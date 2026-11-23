//! Pages moved, deleted or added (blank, or taken from another PDF) without
//! touching the source PDF: the markups file keeps the arrangement as a list
//! of where each page comes from, and the viewer, the export and the command
//! line all open the document through `open`, which arranges a copy of the
//! source in memory. Markups and scales count pages in that arrangement.

use egui::{vec2, Vec2};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Where an arranged page comes from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "from", rename_all = "lowercase")]
pub enum Src {
    /// A page of the source PDF (from 0).
    Source { page: usize },
    /// A blank page of this size (points).
    Blank { width: f32, height: f32 },
    /// A page of another PDF, kept in `<data dir>/pages/` by content (`file`);
    /// `name` is its file name when it was inserted.
    File { file: String, name: String, page: usize },
}

/// One page of the arrangement. `id` tells two pages from the same place apart
/// (two blank pages), so markups follow their page from one arrangement to the next.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PageRef {
    pub id: u64,
    #[serde(flatten)]
    pub src: Src,
}

/// The pages of a document in their order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Layout {
    /// Pages the source PDF had when arranged: pages it gains later (a reissued
    /// set) are added at the end.
    pub source: usize,
    pub pages: Vec<PageRef>,
}

/// Letter, for a page whose source is gone.
const MISSING: Vec2 = vec2(612.0, 792.0);

/// Ids for added pages start here, clear of the source pages' (their index).
const ADDED_IDS: u64 = 1 << 32;

impl Layout {
    /// The source's `n` pages in their own order.
    pub fn identity(n: usize) -> Self {
        Self { source: n, pages: (0..n).map(|i| PageRef { id: i as u64, src: Src::Source { page: i } }).collect() }
    }

    /// Whether this is just the source's pages in their own order.
    pub fn is_identity(&self) -> bool {
        self.pages.len() == self.source && self.pages.iter().enumerate().all(|(i, p)| p.src == Src::Source { page: i })
    }

    /// An id no page here has, nor any page added before (deleted since, or
    /// undone): ids go up with the clock, so a proposal made for a page that's
    /// gone can't pass for one made for a page added in its place.
    pub fn next_id(&self) -> u64 {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
        self.pages.iter().map(|p| p.id + 1).max().unwrap_or(0).max(ADDED_IDS + now)
    }

    /// Adds pages the source has gained since it was arranged (`n` pages now)
    /// at the end. True if any were.
    pub fn fit(&mut self, n: usize) -> bool {
        if n <= self.source {
            return false;
        }
        for (id, page) in (self.next_id()..).zip(self.source..n) {
            self.pages.push(PageRef { id, src: Src::Source { page } });
        }
        self.source = n;
        true
    }
}

/// Identifies an arrangement, so a proposal made for one isn't accepted into
/// another (0: the source's own order).
pub fn fingerprint(layout: Option<&Layout>) -> u64 {
    match layout {
        None => 0,
        Some(l) if l.is_identity() => 0,
        Some(l) => crate::sidecar::fnv(&serde_json::to_vec(&l.pages).unwrap_or_default()).max(1),
    }
}

/// Where each page of `old` is in `new` (`None`: deleted).
pub fn remap(old: &[PageRef], new: &[PageRef]) -> Vec<Option<usize>> {
    let mut at: HashMap<u64, Vec<(usize, &Src)>> = HashMap::new();
    for (i, p) in new.iter().enumerate() {
        at.entry(p.id).or_default().push((i, &p.src));
    }
    old.iter().map(|p| at.get(&p.id).and_then(|v| v.iter().find(|(_, s)| **s == p.src)).map(|(i, _)| *i)).collect()
}

/// `v` with the items at `moved` (indices, any order) taken out and put, in
/// their order, before the item that was at `before` (`len` for the end).
pub fn moved<T: Clone>(v: &[T], moved: &[usize], before: usize) -> Vec<T> {
    let set: HashSet<usize> = moved.iter().copied().filter(|&i| i < v.len()).collect();
    let mut taken: Vec<usize> = set.iter().copied().collect();
    taken.sort_unstable();
    let mut out = Vec::with_capacity(v.len());
    let mut put = false;
    for (i, p) in v.iter().enumerate() {
        if i == before {
            out.extend(taken.iter().map(|&k| v[k].clone()));
            put = true;
        }
        if !set.contains(&i) {
            out.push(p.clone());
        }
    }
    if !put {
        out.extend(taken.iter().map(|&k| v[k].clone()));
    }
    out
}

/// `v` without the items at `gone`.
pub fn without<T: Clone>(v: &[T], gone: &[usize]) -> Vec<T> {
    let set: HashSet<usize> = gone.iter().copied().collect();
    v.iter().enumerate().filter(|(i, _)| !set.contains(i)).map(|(_, p)| p.clone()).collect()
}

/// `layout` with the pages at `moved` put before the page at `before` (`moved`).
pub fn move_pages(layout: &Layout, moved_: &[usize], before: usize) -> Layout {
    Layout { source: layout.source, pages: moved(&layout.pages, moved_, before) }
}

/// `layout` without the pages at `gone`.
pub fn delete_pages(layout: &Layout, gone: &[usize]) -> Layout {
    Layout { source: layout.source, pages: without(&layout.pages, gone) }
}

/// `layout` with pages from `srcs` put before the page at `at` (`len` for the end).
pub fn insert_pages(layout: &Layout, at: usize, srcs: Vec<Src>) -> Layout {
    let mut id = layout.next_id();
    let new: Vec<PageRef> = srcs
        .into_iter()
        .map(|src| {
            id += 1;
            PageRef { id: id - 1, src }
        })
        .collect();
    let mut pages = layout.pages.clone();
    let at = at.min(pages.len());
    pages.splice(at..at, new);
    Layout { source: layout.source, pages }
}

/// Pages of a 1-based range list ("1-3, 7", or "all"/empty for every page)
/// in a document of `n` pages, from 0, in the order given.
pub fn parse_range(s: &str, n: usize) -> Result<Vec<usize>, String> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("all") {
        return Ok((0..n).collect());
    }
    let num = |t: &str| -> Result<usize, String> {
        let v: usize = t.trim().parse().map_err(|_| format!("\"{}\" isn't a page number", t.trim()))?;
        if v == 0 || v > n {
            return Err(format!("page {v} isn't there: the file has {n} pages"));
        }
        Ok(v - 1)
    };
    let mut out = Vec::new();
    for part in s.split(',').filter(|p| !p.trim().is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b)?);
                if a <= b {
                    out.extend(a..=b);
                } else {
                    out.extend((b..=a).rev());
                }
            }
            None => out.push(num(part)?),
        }
    }
    Ok(out)
}

/// Where PDFs pages were inserted from are kept.
pub fn dir() -> PathBuf {
    crate::sidecar::dir().join("pages")
}

/// Keeps a copy of the PDF at `path` for inserting its pages (by content, so
/// the same file is kept once); its stored name.
pub fn store(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    let name = format!("{:016x}.pdf", crate::sidecar::fnv(&bytes));
    let f = dir().join(&name);
    if !f.exists() {
        std::fs::create_dir_all(dir())?;
        let tmp = f.with_extension(format!("pdf.{}.tmp", std::process::id()));
        std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &f)).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
    }
    Ok(name)
}

/// The source PDF arranged as `layout` (as it is, without one). Pages that
/// stay in their order are left where they are and the rest copied in, so
/// arranging is quick however big the set.
pub fn open<'a>(pdfium: &'a Pdfium, src: &Path, layout: Option<&Layout>) -> anyhow::Result<PdfDocument<'a>> {
    open_in(pdfium, src, layout, &dir())
}

/// `open`, with inserted PDFs kept in `files`.
fn open_in<'a>(pdfium: &'a Pdfium, src: &Path, layout: Option<&Layout>, files_dir: &Path) -> anyhow::Result<PdfDocument<'a>> {
    let load = || pdfium.load_pdf_from_file(src, None).map_err(|e| anyhow::anyhow!("{}: {e:?}", src.display()));
    let doc = load()?;
    let Some(layout) = layout else { return Ok(doc) };
    let n = doc.pages().len() as usize;
    let mut layout = layout.clone();
    layout.fit(n);
    if layout.is_identity() {
        return Ok(doc);
    }
    // Source pages left where they are: the longest run of them already in order.
    let mut seen = HashSet::new();
    let order: Vec<(usize, usize)> = layout
        .pages
        .iter()
        .enumerate()
        .filter_map(|(t, p)| match p.src {
            Src::Source { page } if page < n && seen.insert(page) => Some((t, page)),
            _ => None,
        })
        .collect();
    let stay: HashSet<usize> = increasing(&order.iter().map(|o| o.1).collect::<Vec<_>>()).into_iter().map(|k| order[k].0).collect();
    let kept: HashSet<usize> = stay.iter().filter_map(|&t| match layout.pages[t].src {
        Src::Source { page } => Some(page),
        _ => None,
    }).collect();
    let mut doc = doc;
    for i in (0..n).rev() {
        if !kept.contains(&i) {
            doc.pages().get(i as PdfPageIndex)?.delete()?;
        }
    }
    let mut again: Option<PdfDocument> = None;
    let mut files: HashMap<String, Option<PdfDocument>> = HashMap::new();
    for (t, p) in layout.pages.iter().enumerate() {
        if stay.contains(&t) {
            continue;
        }
        let at = t as PdfPageIndex;
        let blank = |doc: &mut PdfDocument, s: Vec2| doc.pages_mut().create_page_at_index(PdfPagePaperSize::new_custom(PdfPoints::new(s.x), PdfPoints::new(s.y)), at).map(|_| ());
        match &p.src {
            Src::Source { page } if *page < n => {
                if again.is_none() {
                    again = Some(load()?);
                }
                let from = again.as_ref().unwrap();
                doc.pages_mut().copy_page_from_document(from, *page as PdfPageIndex, at)?;
            }
            Src::Source { .. } => blank(&mut doc, MISSING)?,
            Src::Blank { width, height } => blank(&mut doc, vec2(*width, *height))?,
            Src::File { file, page, .. } => {
                let from = files.entry(file.clone()).or_insert_with(|| pdfium.load_pdf_from_file(&files_dir.join(file), None).ok());
                match from {
                    Some(f) if (*page as PdfPageIndex) < f.pages().len() => doc.pages_mut().copy_page_from_document(f, *page as PdfPageIndex, at)?,
                    // Its stored copy is gone: a blank page keeps the rest in place.
                    _ => blank(&mut doc, MISSING)?,
                }
            }
        }
    }
    Ok(doc)
}

/// PDFium's own functions, for arranging a file to write (`arrange_file`):
/// the wrapper has no way to move a page.
static RAW: std::sync::OnceLock<Box<dyn PdfiumLibraryBindings>> = std::sync::OnceLock::new();

/// Binds PDFium at `lib` for this process (it can be only once): the wrapper,
/// and PDFium's functions as they are for `arrange_file`.
pub fn bind(lib: &Path) -> Result<Pdfium, PdfiumError> {
    let raw = Pdfium::bind_to_library(lib)?;
    let bindings = Pdfium::bind_to_library(lib)?;
    let _ = RAW.set(raw);
    Ok(Pdfium::new(bindings))
}

/// The source PDF arranged as `layout`, for writing out (an export, a smaller
/// copy): pages are moved, not copied as `open` does, so the PDF's own links
/// and bookmarks to them still lead there. Arranged, it's read from a file
/// written at `tmp`, returned for removing once the document is let go.
pub fn open_to_write<'a>(pdfium: &'a Pdfium, src: &Path, layout: Option<&Layout>, tmp: &Path) -> anyhow::Result<(PdfDocument<'a>, Option<PathBuf>)> {
    match layout.filter(|l| fingerprint(Some(l)) != 0) {
        None => Ok((open(pdfium, src, None)?, None)),
        Some(l) => {
            arrange_file(src, l, tmp, &dir()).inspect_err(|_| {
                let _ = std::fs::remove_file(tmp);
            })?;
            let doc = pdfium.load_pdf_from_file(tmp, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            Ok((doc, Some(tmp.to_path_buf())))
        }
    }
}

/// A document opened through PDFium's own functions, closed when dropped.
struct RawDoc<'a>(&'a dyn PdfiumLibraryBindings, FPDF_DOCUMENT);

impl Drop for RawDoc<'_> {
    fn drop(&mut self) {
        unsafe { self.0.FPDF_CloseDocument(self.1) }
    }
}

fn raw_open<'a>(b: &'a dyn PdfiumLibraryBindings, path: &Path) -> Option<RawDoc<'a>> {
    let doc = unsafe { b.FPDF_LoadDocument(path.to_str()?, None) };
    (!doc.is_null()).then(|| RawDoc(b, doc))
}

/// Where PDFium writes a document: a file.
#[repr(C)]
struct Sink {
    // First, so PDFium's pointer to it is a pointer to the whole.
    write: FPDF_FILEWRITE,
    out: std::io::BufWriter<std::fs::File>,
    failed: bool,
}

unsafe extern "C" fn write_block(this: *mut FPDF_FILEWRITE, data: *const std::ffi::c_void, size: std::ffi::c_ulong) -> std::ffi::c_int {
    use std::io::Write;
    // SAFETY: `this` is the `write` field at the start of a live `Sink` (see `arrange_file`),
    // and PDFium hands over `size` bytes at `data`.
    let sink = unsafe { &mut *(this as *mut Sink) };
    let bytes = if size == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) } };
    match sink.out.write_all(bytes) {
        Ok(()) => 1,
        Err(_) => {
            sink.failed = true;
            0
        }
    }
}

/// Writes the source arranged as `layout` to `out`: pages added at the end,
/// then all moved into place at once and the rest deleted (inserted PDFs in `files_dir`).
fn arrange_file(src: &Path, layout: &Layout, out: &Path, files_dir: &Path) -> anyhow::Result<()> {
    use std::ffi::{c_int, c_ulong};
    let b: &dyn PdfiumLibraryBindings = RAW.get().map(|b| b.as_ref()).ok_or_else(|| anyhow::anyhow!("PDFium isn't loaded"))?;
    let doc = raw_open(b, src).ok_or_else(|| anyhow::anyhow!("{}: couldn't read it", src.display()))?;
    let n = unsafe { b.FPDF_GetPageCount(doc.1) }.max(0) as usize;
    let mut layout = layout.clone();
    layout.fit(n);
    // Each page's index in the document once everything is added at the end.
    let mut order: Vec<Option<c_int>> = vec![None; layout.pages.len()];
    let mut count = n;
    let mut used = HashSet::new();
    let failed = || anyhow::anyhow!("couldn't arrange the pages");
    // Pages from another document (or the source again), by document in order
    // first asked for: imported together, so resources they share (fonts,
    // images) come in once. (PDFium drops links among imported pages either way.)
    // By document (`None`: the source): the arranged position and page of each.
    type Wanted<'k> = Vec<(Option<&'k str>, Vec<(usize, usize)>)>;
    let mut wanted: Wanted = Vec::new();
    for (t, p) in layout.pages.iter().enumerate() {
        let (key, page) = match &p.src {
            Src::Source { page } if *page < n && used.insert(*page) => {
                order[t] = Some(*page as c_int);
                continue;
            }
            Src::Source { page } if *page < n => (None, *page),
            Src::File { file, page, .. } => (Some(file.as_str()), *page),
            _ => continue,
        };
        match wanted.iter_mut().find(|w| w.0 == key) {
            Some(w) => w.1.push((t, page)),
            None => wanted.push((key, vec![(t, page)])),
        }
    }
    for (key, pages) in &wanted {
        let from = match key {
            None => raw_open(b, src),
            Some(file) => raw_open(b, &files_dir.join(file)),
        };
        let Some(from) = from else { continue };
        let there = unsafe { b.FPDF_GetPageCount(from.1) }.max(0) as usize;
        // Pages it lacks are made blank below.
        let pages: Vec<(usize, usize)> = pages.iter().copied().filter(|(_, pg)| *pg < there).collect();
        if pages.is_empty() {
            continue;
        }
        let idx: Vec<c_int> = pages.iter().map(|(_, pg)| *pg as c_int).collect();
        if !b.is_true(unsafe { b.FPDF_ImportPagesByIndex(doc.1, from.1, idx.as_ptr(), idx.len() as c_ulong, count as c_int) }) {
            return Err(failed());
        }
        for (t, _) in pages {
            order[t] = Some(count as c_int);
            count += 1;
        }
    }
    // Blank pages, and blanks for pages whose source is gone.
    for (t, p) in layout.pages.iter().enumerate() {
        if order[t].is_some() {
            continue;
        }
        let s = match &p.src {
            Src::Blank { width, height } => vec2(*width, *height),
            _ => MISSING,
        };
        let page = unsafe { b.FPDFPage_New(doc.1, count as c_int, s.x as f64, s.y as f64) };
        if page.is_null() {
            return Err(failed());
        }
        unsafe { b.FPDF_ClosePage(page) };
        order[t] = Some(count as c_int);
        count += 1;
    }
    let order: Vec<c_int> = order.into_iter().flatten().collect();
    if !order.is_empty() && !b.is_true(unsafe { b.FPDF_MovePages(doc.1, order.as_ptr(), order.len() as c_ulong, 0) }) {
        return Err(failed());
    }
    for i in (order.len()..count).rev() {
        unsafe { b.FPDFPage_Delete(doc.1, i as c_int) };
    }
    let mut sink = Box::new(Sink { write: FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) }, out: std::io::BufWriter::new(std::fs::File::create(out)?), failed: false });
    let saved = b.is_true(unsafe { b.FPDF_SaveAsCopy(doc.1, &mut sink.write, 0) });
    let flushed = std::io::Write::flush(&mut sink.out);
    if !saved || sink.failed {
        anyhow::bail!("couldn't write {}", out.display());
    }
    flushed?;
    Ok(())
}

/// Indices into `v` of a longest strictly increasing subsequence.
fn increasing(v: &[usize]) -> Vec<usize> {
    // tails[k]: index of the smallest last value of a run of length k+1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; v.len()];
    for i in 0..v.len() {
        let k = tails.partition_point(|&t| v[t] < v[i]);
        prev[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(i);
        cur = prev[i];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(l: &Layout) -> Vec<String> {
        l.pages
            .iter()
            .map(|p| match &p.src {
                Src::Source { page } => page.to_string(),
                Src::Blank { .. } => "b".into(),
                Src::File { page, .. } => format!("f{page}"),
            })
            .collect()
    }

    #[test]
    fn moves_deletes_and_inserts() {
        let l = Layout::identity(5);
        assert_eq!(sources(&move_pages(&l, &[3, 1], 0)), ["1", "3", "0", "2", "4"]);
        assert_eq!(sources(&move_pages(&l, &[0], 5)), ["1", "2", "3", "4", "0"]);
        assert_eq!(sources(&move_pages(&l, &[0, 1], 3)), ["2", "0", "1", "3", "4"]);
        assert_eq!(sources(&delete_pages(&l, &[0, 4])), ["1", "2", "3"]);
        let l2 = insert_pages(&l, 2, vec![Src::Blank { width: 10.0, height: 20.0 }, Src::Blank { width: 10.0, height: 20.0 }]);
        assert_eq!(sources(&l2), ["0", "1", "b", "b", "2", "3", "4"]);
        assert_ne!(l2.pages[2].id, l2.pages[3].id, "two blank pages are told apart");
        let m = remap(&l2.pages, &move_pages(&l2, &[3], 0).pages);
        assert_eq!(m, [Some(1), Some(2), Some(3), Some(0), Some(4), Some(5), Some(6)]);
        assert_eq!(remap(&l.pages, &delete_pages(&l, &[1]).pages), [Some(0), None, Some(1), Some(2), Some(3)]);
        assert!(move_pages(&l, &[2], 2).is_identity());
        assert_eq!(fingerprint(Some(&l)), 0);
        assert_ne!(fingerprint(Some(&l2)), 0);
    }

    #[test]
    fn a_reissued_set_gains_its_new_pages_at_the_end() {
        let mut l = delete_pages(&Layout::identity(3), &[0]);
        assert!(l.fit(4));
        assert_eq!(sources(&l), ["1", "2", "3"]);
        assert!(!l.fit(4));
    }

    /// A PDF of pages of these sizes, each with its number written on it.
    fn numbered(pdfium: &Pdfium, path: &Path, sizes: &[(f32, f32)], tag: &str) {
        let mut doc = pdfium.create_new_pdf().unwrap();
        let font = doc.fonts_mut().helvetica();
        for (i, (w, h)) in sizes.iter().enumerate() {
            let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(*w), PdfPoints::new(*h))).unwrap();
            let mut t = PdfPageTextObject::new(&doc, format!("{tag}{}", i + 1), font, PdfPoints::new(12.0)).unwrap();
            t.translate(PdfPoints::new(10.0), PdfPoints::new(10.0)).unwrap();
            page.objects_mut().add_text_object(t).unwrap();
        }
        doc.save_to_file(path).unwrap();
    }

    #[test]
    fn open_arranges_a_copy_of_the_source() {
        let _turn = crate::render::tests::PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = crate::render::tests::test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("set.pdf");
        numbered(pdfium, &src, &[(100.0, 200.0), (300.0, 400.0), (500.0, 600.0), (700.0, 800.0)], "S");
        numbered(pdfium, &dir.join("other.pdf"), &[(110.0, 120.0), (130.0, 140.0)], "O");
        let before = std::fs::read(&src).unwrap();

        // Page 4 first, a blank, page 1, the other file's page 2, page 2 (page 3 deleted).
        let base = Layout::identity(4);
        let l = move_pages(&delete_pages(&base, &[2]), &[2], 0);
        let l = insert_pages(&l, 1, vec![Src::Blank { width: 50.0, height: 60.0 }]);
        let l = insert_pages(&l, 3, vec![Src::File { file: "other.pdf".into(), name: "other.pdf".into(), page: 1 }]);
        let doc = open_in(pdfium, &src, Some(&l), &dir).unwrap();
        let sizes: Vec<(f32, f32)> = doc.pages().iter().map(|p| (p.width().value, p.height().value)).collect();
        assert_eq!(sizes, [(700.0, 800.0), (50.0, 60.0), (100.0, 200.0), (130.0, 140.0), (300.0, 400.0)]);
        let text: Vec<String> = doc.pages().iter().map(|p| p.text().unwrap().all().trim().to_string()).collect();
        assert_eq!(text, ["S4", "", "S1", "O2", "S2"]);
        drop(doc);
        assert_eq!(std::fs::read(&src).unwrap(), before, "the source PDF is never changed");

        // A stored copy gone, or a source page the PDF no longer has: blank pages keep the rest in place.
        let l = insert_pages(&Layout::identity(4), 0, vec![Src::File { file: "gone.pdf".into(), name: "gone.pdf".into(), page: 0 }]);
        let l = Layout { source: 4, pages: [l.pages, vec![PageRef { id: 99, src: Src::Source { page: 9 } }]].concat() };
        let doc = open_in(pdfium, &src, Some(&l), &dir).unwrap();
        assert_eq!(doc.pages().len(), 6);
        assert_eq!(doc.pages().get(0).unwrap().width().value, 612.0);
        assert_eq!(doc.pages().get(1).unwrap().width().value, 100.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Three pages, the first with a link to the third.
    fn linked(path: &Path) {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [6 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>",
            "<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /Dest [5 0 R /Fit] >>",
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            pdf.extend(format!("{o:010} 00000 n \n").bytes());
        }
        pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        std::fs::write(path, pdf).unwrap();
    }

    #[test]
    fn written_out_moved_pages_keep_their_links() {
        let _turn = crate::render::tests::PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = crate::render::tests::test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("linked.pdf");
        linked(&src);
        let dest = |doc: &PdfDocument, page: PdfPageIndex| -> PdfPageIndex {
            let p = doc.pages().get(page).unwrap();
            let a = p.annotations().iter().next().expect("the link");
            a.as_link_annotation().unwrap().link().unwrap().destination().unwrap().page_index().unwrap()
        };
        assert_eq!(dest(&pdfium.load_pdf_from_file(&src, None).unwrap(), 0), 2);

        // Page 3 first, then 1 and a blank (page 2 deleted): the link still leads to page 3, now first.
        let l = insert_pages(&move_pages(&delete_pages(&Layout::identity(3), &[1]), &[1], 0), 2, vec![Src::Blank { width: 10.0, height: 20.0 }]);
        let (doc, tmp) = open_to_write(pdfium, &src, Some(&l), &dir.join("arranged.pdf")).unwrap();
        let sizes: Vec<f32> = doc.pages().iter().map(|p| p.width().value).collect();
        assert_eq!(sizes, [300.0, 100.0, 10.0]);
        assert_eq!(dest(&doc, 1), 0);
        drop(doc);
        assert!(tmp.is_some());

        // Another PDF inserted whole (here, the same one): the set's own links still hold. (PDFium's
        // import drops links among the pages it brings in, however many come in one import.)
        let srcs = (0..3).map(|page| Src::File { file: "linked.pdf".into(), name: "linked.pdf".into(), page }).collect();
        let l = insert_pages(&Layout::identity(3), 3, srcs);
        arrange_file(&src, &l, &dir.join("inserted.pdf"), &dir).unwrap();
        let doc = pdfium.load_pdf_from_file(&dir.join("inserted.pdf"), None).unwrap();
        assert_eq!(doc.pages().len(), 6);
        assert_eq!(dest(&doc, 0), 2);
        drop(doc);

        // In the PDF's own order: the PDF itself, nothing written.
        let (doc, tmp) = open_to_write(pdfium, &src, Some(&Layout::identity(3)), &dir.join("same.pdf")).unwrap();
        assert_eq!(doc.pages().len(), 3);
        assert!(tmp.is_none() && !dir.join("same.pdf").exists());
        drop(doc);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("", 3).unwrap(), [0, 1, 2]);
        assert_eq!(parse_range("All", 2).unwrap(), [0, 1]);
        assert_eq!(parse_range("1-2, 5", 5).unwrap(), [0, 1, 4]);
        assert_eq!(parse_range("3-1", 3).unwrap(), [2, 1, 0]);
        assert!(parse_range("4", 3).is_err());
        assert!(parse_range("x", 3).is_err());
    }

    #[test]
    fn longest_increasing_run() {
        assert_eq!(increasing(&[0, 1, 2]), [0, 1, 2]);
        let v = [3, 0, 1, 4, 2];
        let r = increasing(&v);
        assert_eq!(r.len(), 3);
        assert!(r.windows(2).all(|w| v[w[0]] < v[w[1]]));
        assert!(increasing(&[]).is_empty());
    }
}
