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

    /// An id no page here has.
    pub fn next_id(&self) -> u64 {
        self.pages.iter().map(|p| p.id + 1).max().unwrap_or(0).max(ADDED_IDS)
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
