//! PDFium worker thread. PDFium is not thread-safe, so one thread owns the
//! library and every document: it renders pages for the viewer and writes
//! flattened exports.

use crate::annot::{Annot, Prim, Rgb};
use egui::{pos2, vec2, Pos2, Vec2};
use pdfium_render::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

/// What a render is for: the whole page, a sharp patch of the visible area, or a thumbnail.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Layer {
    Base,
    Patch,
    Thumb,
}

/// Pages the viewer still wants; queued renders for other pages are dropped.
#[derive(Default)]
pub struct Keep {
    pub base: HashSet<usize>,
    pub thumbs: HashSet<usize>,
    /// Of those, the ones the viewer has no thumbnail for yet: a whole-page
    /// render of one is shrunk into its thumbnail.
    pub thumbs_missing: HashSet<usize>,
    /// The search still wanted; page searches for older ones are dropped.
    pub search: u64,
}

pub enum Req {
    Open { generation: u64, path: PathBuf },
    /// A tab closed: let its document go.
    Close { generation: u64 },
    /// Render the region `origin..origin+size` (pixels) of the page drawn at `scale` px/pt.
    Render { generation: u64, page: usize, scale: f32, origin: [i32; 2], size: [usize; 2], layer: Layer },
    Export { src: PathBuf, out: PathBuf, annots: Vec<Annot>, mode: ExportMode, author: String },
    /// A page's text, for the scales written on it.
    PageText { generation: u64, page: usize },
    /// Write a new PDF of `pages` blank pages, `size` points each.
    Create { path: PathBuf, size: Vec2, pages: usize },
    /// Find `query` on `pages` in turn, for search `id`. The worker searches one
    /// page per turn and queues the rest behind other work, so renders stay live.
    Search { generation: u64, id: u64, pages: VecDeque<usize>, query: String },
}

pub enum Resp {
    Opened { generation: u64, pages: Vec<Vec2> },
    OpenFailed { generation: u64, error: String },
    Rendered { generation: u64, page: usize, scale: f32, origin: [i32; 2], layer: Layer, image: Arc<egui::ColorImage> },
    RenderFailed { generation: u64, page: usize, layer: Layer, error: String },
    /// Output path and the number of characters that had no font.
    Exported(Result<(PathBuf, usize), String>),
    PageText { generation: u64, page: usize, text: String },
    /// A blank PDF written (its path), or why not.
    Created(Result<PathBuf, String>),
    /// Matches on one page for search `id`: per match, a box per line it spans;
    /// `more` when the page had more than were reported.
    Found { generation: u64, id: u64, page: usize, hits: Vec<Vec<egui::Rect>>, more: bool },
}

pub struct Worker {
    tx: mpsc::Sender<Req>,
    pub rx: mpsc::Receiver<Resp>,
    pub keep: Arc<Mutex<Keep>>,
}

impl Worker {
    pub fn spawn(ctx: egui::Context) -> anyhow::Result<Self> {
        let lib = find_pdfium()?;
        let (tx, req_rx) = mpsc::channel();
        let (resp_tx, rx) = mpsc::channel();
        let keep = Arc::new(Mutex::new(Keep::default()));
        let keep2 = keep.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new().name("pdfium".into()).spawn(move || {
            let pdfium = match Pdfium::bind_to_library(&lib) {
                Ok(b) => {
                    let _ = ready_tx.send(Ok(()));
                    Pdfium::new(b)
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("loading {}: {e}", lib.display())));
                    return;
                }
            };
            run(&pdfium, req_rx, resp_tx, keep2, ctx);
        })?;
        ready_rx.recv()?.map_err(anyhow::Error::msg)?;
        Ok(Self { tx, rx, keep })
    }

    pub fn send(&self, req: Req) {
        let _ = self.tx.send(req);
    }
}

pub fn find_pdfium() -> anyhow::Result<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
            dirs.push(d.join("../../vendor"));
        }
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor"));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/lib"));
    }
    dirs.push(PathBuf::from("/usr/lib"));
    dirs.into_iter()
        .map(|d| d.join(&name))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("libpdfium.so not found; run scripts/fetch-pdfium.sh"))
}

fn run(
    pdfium: &Pdfium,
    rx: mpsc::Receiver<Req>,
    tx: mpsc::Sender<Resp>,
    keep: Arc<Mutex<Keep>>,
    ctx: egui::Context,
) {
    // One open document per tab, by the generation it was opened as.
    let mut docs: HashMap<u64, PdfDocument> = HashMap::new();
    // Declared after `docs`, so dropped before them: a page must close before its document.
    let mut loaded = Loaded::default();
    let mut thumbs = Thumbs::default();
    let mut queue: VecDeque<Req> = VecDeque::new();
    // Thumbnails and a search take turns.
    let mut thumb_turn = false;
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(r) => queue.push_back(r),
                Err(_) => return,
            }
        }
        while let Ok(r) = rx.try_recv() {
            queue.push_back(r);
        }
        let Some(req) = next_request(&mut queue, &keep, &mut thumb_turn) else { continue };
        let resp = match req {
            Req::Open { generation, path } => {
                match pdfium.load_pdf_from_file(&path, None).and_then(|d| page_sizes(&d).map(|s| (d, s))) {
                    Ok((d, pages)) => {
                        docs.insert(generation, d);
                        Resp::Opened { generation, pages }
                    }
                    Err(e) => Resp::OpenFailed { generation, error: format!("{e:?}") },
                }
            }
            Req::Render { generation, page, scale, origin, size, layer } => {
                let Some(d) = docs.get(&generation) else { continue };
                // A thumbnail made from this page's last whole-page render costs nothing.
                if layer == Layer::Thumb {
                    if let Some(image) = thumbs.get(generation, page, size) {
                        if tx.send(Resp::Rendered { generation, page, scale, origin, layer, image }).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                        continue;
                    }
                }
                let rendered = loaded.get(d, generation, page).and_then(|p| Ok((render_page_region(p, scale, origin, size)?, vec2(p.width().value, p.height().value))));
                match rendered {
                    Ok((image, page_size)) => {
                        let image = Arc::new(image);
                        if tx.send(Resp::Rendered { generation, page, scale, origin, layer, image: image.clone() }).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                        // A thumbnail the sidebar or overview is waiting for, from this
                        // whole-page render (after sending it, so the view isn't kept waiting).
                        if layer == Layer::Base && keep.lock().unwrap().thumbs_missing.contains(&page) {
                            let (thumb_scale, thumb_size) = thumb_size(page_size);
                            if let Some(thumb) = thumbs.add(generation, page, &image, thumb_size) {
                                if tx.send(Resp::Rendered { generation, page, scale: thumb_scale, origin: [0, 0], layer: Layer::Thumb, image: thumb }).is_err() {
                                    return;
                                }
                                ctx.request_repaint();
                            }
                        }
                        continue;
                    }
                    Err(e) => Resp::RenderFailed { generation, page, layer, error: e.to_string() },
                }
            }
            Req::PageText { generation, page } => {
                let Some(d) = docs.get(&generation) else { continue };
                let text = loaded.get(d, generation, page).ok().and_then(|p| p.text().ok().map(|t| t.all())).unwrap_or_default();
                Resp::PageText { generation, page, text }
            }
            Req::Search { generation, id, mut pages, query } => {
                let Some(d) = docs.get(&generation) else { continue };
                let Some(page) = pages.pop_front() else { continue };
                // Searching every page goes past the loaded-page cache: load each one just for this.
                let (hits, more) = d.pages().get(page as PdfPageIndex).ok().and_then(|p| crate::pdf::search_page(&p, &query).ok()).unwrap_or_default();
                if !pages.is_empty() {
                    queue.push_back(Req::Search { generation, id, pages, query });
                }
                Resp::Found { generation, id, page, hits, more }
            }
            Req::Close { generation } => {
                // Its pages before the document they belong to.
                loaded.forget(generation);
                thumbs.forget(generation);
                docs.remove(&generation);
                continue;
            }
            Req::Create { path, size, pages } => Resp::Created(create_blank(pdfium, &path, size, pages).map(|_| path).map_err(|e| e.to_string())),
            Req::Export { src, out, annots, mode, author } => Resp::Exported(
                load_images(&annots)
                    .and_then(|images| export(pdfium, &src, &out, &annots, &images, mode, &author))
                    .map(|n| (out, n))
                    .map_err(|e| e.to_string()),
            ),
        };
        if tx.send(resp).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Open/Export first, then the newest patch, then base renders the viewer still
/// wants, then wanted thumbnails and the current search page by page, in turn.
fn next_request(queue: &mut VecDeque<Req>, keep: &Mutex<Keep>, thumb_turn: &mut bool) -> Option<Req> {
    if let Some(i) = queue.iter().position(|r| !matches!(r, Req::Render { .. } | Req::Search { .. })) {
        return queue.remove(i);
    }
    // Drop renders superseded by a newer request for the same page and layer.
    let mut seen = HashSet::new();
    let mut kept = VecDeque::new();
    while let Some(r) = queue.pop_back() {
        if let Req::Render { page, layer, .. } = &r {
            if !seen.insert((*page, *layer)) {
                continue;
            }
        }
        kept.push_front(r);
    }
    *queue = kept;
    if let Some(i) = queue.iter().rposition(|r| matches!(r, Req::Render { layer: Layer::Patch, .. })) {
        return queue.remove(i);
    }
    let keep = keep.lock().unwrap();
    queue.retain(|r| match r {
        Req::Render { page, layer: Layer::Base, .. } => keep.base.contains(page),
        Req::Render { page, layer: Layer::Thumb, .. } => keep.thumbs.contains(page),
        _ => true,
    });
    if let Some(i) = queue.iter().position(|r| matches!(r, Req::Render { layer: Layer::Base, .. })) {
        return queue.remove(i);
    }
    // A search goes a page at a time between renders, taking turns with
    // thumbnails, so the view and the overview stay live; stale ones are dropped.
    queue.retain(|r| !matches!(r, Req::Search { id, .. } if *id != keep.search));
    let search = queue.iter().position(|r| matches!(r, Req::Search { .. }));
    let thumb = queue.iter().position(|r| matches!(r, Req::Render { layer: Layer::Thumb, .. }));
    *thumb_turn = !*thumb_turn;
    match (search, thumb) {
        (Some(s), Some(t)) => queue.remove(if *thumb_turn { t } else { s }),
        (Some(i), None) | (None, Some(i)) => queue.remove(i),
        (None, None) => queue.pop_front(),
    }
}

/// Writes a new PDF of `pages` blank pages of `size` points. It never replaces
/// a file: `path` is created exclusively (failing if anything is there), and
/// removed again if writing fails.
pub fn create_blank(pdfium: &Pdfium, path: &Path, size: Vec2, pages: usize) -> anyhow::Result<()> {
    let mut doc = pdfium.create_new_pdf()?;
    for _ in 0..pages.max(1) {
        doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(size.x), PdfPoints::new(size.y)))?;
    }
    let bytes = doc.save_to_bytes()?;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists { anyhow::anyhow!("{} already exists", path.display()) } else { anyhow::Error::from(e) }
    })?;
    let written = std::io::Write::write_all(&mut f, &bytes).and_then(|_| f.sync_all());
    if let Err(e) = written {
        drop(f);
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}

/// Every page's displayed size (after /Rotate and the CropBox) in points, read
/// from the page tree without loading the pages: loading one parses all its
/// content, which for a big drawing set took most of the time to open it.
pub fn page_sizes(doc: &PdfDocument) -> Result<Vec<Vec2>, PdfiumError> {
    Ok(doc.pages().page_sizes()?.iter().map(|r| vec2(r.width().value, r.height().value)).collect())
}

pub fn render_region(doc: &PdfDocument, page: usize, scale: f32, origin: [i32; 2], size: [usize; 2]) -> anyhow::Result<egui::ColorImage> {
    render_page_region(&doc.pages().get(page as PdfPageIndex)?, scale, origin, size)
}

/// Renders the region `origin..origin+size` (pixels) of `p` drawn at `scale` px/pt.
pub fn render_page_region(p: &PdfPage, scale: f32, origin: [i32; 2], size: [usize; 2]) -> anyhow::Result<egui::ColorImage> {
    let full_w = (p.width().value * scale).round().max(1.0) as Pixels;
    let full_h = (p.height().value * scale).round().max(1.0) as Pixels;
    // PDFium writes RGBA straight away (no swizzle afterwards), onto white.
    let cfg = PdfRenderConfig::new()
        .set_fixed_size(full_w, full_h)
        .set_origin(-origin[0] as Pixels, -origin[1] as Pixels)
        .set_reverse_byte_order(true)
        .render_annotations(true);
    let mut bmp = PdfBitmap::empty(size[0] as Pixels, size[1] as Pixels, PdfBitmapFormat::BGRA)?;
    p.render_into_bitmap_with_config(&mut bmp, &cfg)?;
    Ok(opaque_image(&bmp.as_raw_bytes(), size))
}

/// A rendered page (RGBA rows, maybe padded) as an image. Pages are opaque, so
/// the pixels need no unpremultiplying: each is taken as it is, fully opaque.
fn opaque_image(raw: &[u8], size: [usize; 2]) -> egui::ColorImage {
    let [w, h] = size;
    let stride = if h == 0 { 0 } else { raw.len() / h };
    let mut pixels = Vec::with_capacity(w * h);
    for row in raw.chunks_exact(stride.max(1)).take(h) {
        pixels.extend(row[..w * 4].chunks_exact(4).map(|c| egui::Color32::from_rgba_premultiplied(c[0], c[1], c[2], 255)));
    }
    egui::ColorImage::new(size, pixels)
}

/// Pages loaded lately, kept so the next render of one (a sharper patch after
/// panning, a re-render after zooming) needn't parse its content again or
/// decode its images again: PDFium keeps decoded images with the page.
#[derive(Default)]
struct Loaded<'a> {
    /// Most recently used last.
    pages: Vec<((u64, usize), PdfPage<'a>)>,
}

/// Loaded pages kept: a scanned sheet's decoded image can be large.
const LOADED_PAGES: usize = 4;

impl<'a> Loaded<'a> {
    fn get(&mut self, doc: &PdfDocument<'a>, generation: u64, page: usize) -> anyhow::Result<&PdfPage<'a>> {
        let key = (generation, page);
        if let Some(i) = self.pages.iter().position(|(k, _)| *k == key) {
            let e = self.pages.remove(i);
            self.pages.push(e);
        } else {
            let p = doc.pages().get(page as PdfPageIndex)?;
            if self.pages.len() >= LOADED_PAGES {
                self.pages.remove(0);
            }
            self.pages.push((key, p));
        }
        Ok(&self.pages.last().unwrap().1)
    }

    fn forget(&mut self, generation: u64) {
        self.pages.retain(|((g, _), _)| *g != generation);
    }
}

/// Thumbnail size: physical pixels on the long side.
const THUMB_PX: f32 = 420.0;

/// The scale (px/pt) and pixel size of the thumbnail of a page `s` points in size.
pub fn thumb_size(s: Vec2) -> (f32, [usize; 2]) {
    let scale = THUMB_PX / s.x.max(s.y);
    (scale, [(s.x * scale).round().max(1.0) as usize, (s.y * scale).round().max(1.0) as usize])
}

/// Thumbnails shrunk from whole-page renders, so a page seen in the main view
/// needn't be rendered again for the sidebar or overview.
#[derive(Default)]
struct Thumbs {
    /// Most recently made last.
    made: VecDeque<((u64, usize), Arc<egui::ColorImage>)>,
}

impl Thumbs {
    /// A thumbnail of `size` from a whole-page render, unless the render is smaller.
    fn add(&mut self, generation: u64, page: usize, image: &egui::ColorImage, size: [usize; 2]) -> Option<Arc<egui::ColorImage>> {
        if image.size[0] < size[0] || image.size[1] < size[1] {
            return None;
        }
        let thumb = Arc::new(shrink(image, size));
        self.made.retain(|(key, _)| *key != (generation, page));
        if self.made.len() >= 64 {
            self.made.pop_front();
        }
        self.made.push_back(((generation, page), thumb.clone()));
        Some(thumb)
    }

    /// The page's thumbnail if one was made at `size`.
    fn get(&self, generation: u64, page: usize, size: [usize; 2]) -> Option<Arc<egui::ColorImage>> {
        self.made.iter().find(|(k, t)| *k == (generation, page) && t.size == size).map(|(_, t)| t.clone())
    }

    fn forget(&mut self, generation: u64) {
        self.made.retain(|((g, _), _)| *g != generation);
    }
}

/// `image` shrunk to `size`. Each pixel covers a block of the original and is
/// halfway between the block's average and its darkest: a thin dark line keeps
/// showing as a line (a plain average would fade it to nearly white, where a
/// direct small render draws it a pixel wide). One pass over the rows, in order.
fn shrink(image: &egui::ColorImage, size: [usize; 2]) -> egui::ColorImage {
    let [sw, sh] = image.size;
    let [w, h] = size;
    debug_assert!(sw >= w && sh >= h, "shrink only");
    let raw = image.as_raw();
    // Source columns where each output column starts (and one past the end).
    let cols: Vec<usize> = (0..=w).map(|x| x * sw / w).collect();
    let mut pixels = Vec::with_capacity(w * h);
    let mut sum = vec![0u32; 3 * w];
    let mut min = vec![255u8; 3 * w];
    for y in 0..h {
        let (y0, y1) = (y * sh / h, (y + 1) * sh / h);
        sum.fill(0);
        min.fill(255);
        for row in raw[y0 * sw * 4..y1 * sw * 4].chunks_exact(sw * 4) {
            for x in 0..w {
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                let (mut mr, mut mg, mut mb) = (255u8, 255u8, 255u8);
                for p in row[cols[x] * 4..cols[x + 1] * 4].chunks_exact(4) {
                    r += p[0] as u32;
                    g += p[1] as u32;
                    b += p[2] as u32;
                    mr = mr.min(p[0]);
                    mg = mg.min(p[1]);
                    mb = mb.min(p[2]);
                }
                let (s, m) = (&mut sum[3 * x..3 * x + 3], &mut min[3 * x..3 * x + 3]);
                s[0] += r;
                s[1] += g;
                s[2] += b;
                m[0] = m[0].min(mr);
                m[1] = m[1].min(mg);
                m[2] = m[2].min(mb);
            }
        }
        for x in 0..w {
            let n = ((y1 - y0) * (cols[x + 1] - cols[x])).max(1) as u32;
            let v = |i: usize| ((sum[3 * x + i] / n + min[3 * x + i] as u32) / 2) as u8;
            pixels.push(egui::Color32::from_rgb(v(0), v(1), v(2)));
        }
    }
    egui::ColorImage::new(size, pixels)
}

// ---------------------------------------------------------------------------
// Flattened export

/// Maps page-local view points (displayed page, y down) to PDF user space,
/// using PDFium's own device<->page transform so /Rotate and CropBox are honoured.
pub struct ViewToUser {
    o: Pos2,
    u: Vec2,
    v: Vec2,
}

impl ViewToUser {
    pub fn for_page(page: &PdfPage) -> anyhow::Result<Self> {
        let (w, h) = (page.width().value, page.height().value);
        let (wp, hp) = ((w * 10.0).round() as Pixels, (h * 10.0).round() as Pixels);
        let cfg = PdfRenderConfig::new().set_fixed_size(wp, hp);
        let pt = |x: Pixels, y: Pixels| -> anyhow::Result<Pos2> {
            let (px, py) = page.pixels_to_points(x, y, &cfg)?;
            Ok(pos2(px.value, py.value))
        };
        let o = pt(0, 0)?;
        let (vw, vh) = (wp as f32 / 10.0, hp as f32 / 10.0);
        Ok(Self { o, u: (pt(wp, 0)? - o) / vw, v: (pt(0, hp)? - o) / vh })
    }

    pub fn map(&self, p: Pos2) -> Pos2 {
        self.o + self.u * p.x + self.v * p.y
    }

    /// PDF user space back to view points.
    pub fn unmap(&self, q: Pos2) -> Pos2 {
        let d = q - self.o;
        let det = self.u.x * self.v.y - self.u.y * self.v.x;
        pos2((d.x * self.v.y - d.y * self.v.x) / det, (self.u.x * d.y - self.u.y * d.x) / det)
    }
}

fn color(c: Rgb, a: u8) -> PdfColor {
    PdfColor::new(c[0], c[1], c[2], a)
}

fn path_object<'a>(doc: &PdfDocument<'a>, pts: &[Pos2], closed: bool, stroke: Option<(Rgb, f32)>, fill: Option<PdfColor>) -> anyhow::Result<PdfPagePathObject<'a>> {
    let p0 = pts[0];
    let mut obj = PdfPagePathObject::new(
        doc,
        PdfPoints::new(p0.x),
        PdfPoints::new(p0.y),
        stroke.map(|(c, _)| color(c, 255)),
        stroke.map(|(_, w)| PdfPoints::new(w)),
        fill,
    )?;
    for p in &pts[1..] {
        obj.line_to(PdfPoints::new(p.x), PdfPoints::new(p.y))?;
    }
    if closed {
        obj.close_path()?;
    }
    obj.set_line_join(PdfPageObjectLineJoin::Round)?;
    obj.set_line_cap(PdfPageObjectLineCap::Round)?;
    Ok(obj)
}

/// Characters the standard Helvetica (WinAnsiEncoding) can show.
fn win_ansi(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7e | 0xa0..=0xff) || "€‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ".contains(c)
}

/// Normalise symbols common in drawing notes to ones our fonts carry.
fn normalise(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '⌀' | '∅' => 'Ø',
            '−' => '-',
            '\u{a0}' => ' ',
            c => c,
        })
        // Soft hyphens are invisible on screen; PDFium's Helvetica mapping garbles them.
        .filter(|&c| c != '\u{ad}')
        .collect()
}

/// Splits a line into runs that share a font: `None` = built-in Helvetica,
/// `Some(i)` = `fonts::faces()[i]`. Characters no font has become '?'; the
/// second value counts them.
fn font_runs(text: &str) -> (Vec<(Option<usize>, String)>, usize) {
    let mut runs: Vec<(Option<usize>, String)> = Vec::new();
    let mut missing = 0;
    for c in normalise(text).chars() {
        let (key, c) = if win_ansi(c) {
            (None, c)
        } else if let Some(i) = crate::fonts::face_for(c).filter(|_| (c as u32) <= 0xffff) {
            // PDFium's text path only handles the Basic Multilingual Plane.
            (Some(i), c)
        } else {
            missing += 1;
            (None, '?')
        };
        match runs.last_mut() {
            Some((k, s)) if *k == key => s.push(c),
            _ => runs.push((key, c.to_string())),
        }
    }
    (runs, missing)
}

/// How markups are written into the exported PDF.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportMode {
    /// Burned into the page content: what you see is what everyone gets.
    Flatten,
    /// One stamp annotation per markup, with the identical appearance and the
    /// note text as its comment, so Bluebeam/Acrobat users can move, reply to or delete them.
    Editable,
}

/// Highest resolution an image is embedded at, for its placed size.
const IMAGE_DPI: f32 = 600.0;

/// Library images used by `annots`, decoded. A missing one fails the export
/// rather than sending out a sheet without its seal.
pub fn load_images(annots: &[Annot]) -> anyhow::Result<HashMap<String, image::RgbaImage>> {
    let mut out = HashMap::new();
    for a in annots {
        if let crate::annot::Kind::Image { image, name, .. } = &a.kind {
            if !out.contains_key(image) {
                let img = crate::stamps::decode(image).map_err(|e| anyhow::anyhow!("stamp \"{name}\" on page {}: {e}", a.page + 1))?;
                out.insert(image.clone(), img);
            }
        }
    }
    Ok(out)
}

/// Images for image prims, downsampled to `IMAGE_DPI` at their placed size
/// (cached, since a stamp copied to every sheet has the same size on each).
struct Images<'i> {
    src: &'i HashMap<String, image::RgbaImage>,
    scaled: HashMap<(String, u32, u32), image::DynamicImage>,
}

impl Images<'_> {
    fn get(&mut self, name: &str, size_pt: Vec2) -> anyhow::Result<&image::DynamicImage> {
        let img = self.src.get(name).ok_or_else(|| anyhow::anyhow!("image {name} not loaded"))?;
        let k = (size_pt.x.max(size_pt.y) / 72.0 * IMAGE_DPI / img.width().max(img.height()) as f32).min(1.0);
        let (w, h) = (((img.width() as f32 * k).round() as u32).max(1), ((img.height() as f32 * k).round() as u32).max(1));
        Ok(self.scaled.entry((name.to_string(), w, h)).or_insert_with(|| {
            let img = if (w, h) == img.dimensions() { img.clone() } else { image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3) };
            image::DynamicImage::ImageRgba8(img)
        }))
    }
}

/// Fonts for text prims: built-in Helvetica plus system faces embedded on demand.
struct TextFonts {
    /// Helvetica, -Bold, -Oblique, -BoldOblique (indexed bold * 1 + italic * 2).
    helvetica: [PdfFontToken; 4],
    /// By system face and whether its bold / italic version was used.
    embedded: std::collections::HashMap<(usize, bool, bool), PdfFontToken>,
    missing: usize,
}

/// Adds `prims` (page view points) to `objects` — a page's content or an
/// annotation's appearance — mapped into PDF user space by `m`.
fn emit_prims<'a>(
    doc: &mut PdfDocument<'a>,
    objects: &mut impl PdfPageObjectsCommon<'a>,
    m: &ViewToUser,
    prims: Vec<Prim>,
    fonts: &mut TextFonts,
    images: &mut Images,
) -> anyhow::Result<()> {
    // Unit vectors for upright text: baseline runs along +u, glyph "up" is -v.
    let (ux, uy) = (m.u.normalized(), (-m.v).normalized());
    let k = m.u.length();
    let mut prims = prims.into_iter().peekable();
    while let Some(prim) = prims.next() {
        match prim {
            Prim::Segments { segs, offset, width, color: c, .. } if !segs.is_empty() => {
                // One path object for the whole hatch: far smaller than one per segment.
                let first = m.map(segs[0][0] + offset);
                let mut obj = PdfPagePathObject::new(doc, PdfPoints::new(first.x), PdfPoints::new(first.y), Some(color(c, 255)), Some(PdfPoints::new(width * k)), None)?;
                for (i, [a, b]) in segs.iter().enumerate() {
                    let (a, b) = (m.map(*a + offset), m.map(*b + offset));
                    if i > 0 {
                        obj.move_to(PdfPoints::new(a.x), PdfPoints::new(a.y))?;
                    }
                    // A dot: a zero-length segment, which round caps draw as a dot.
                    obj.line_to(PdfPoints::new(b.x), PdfPoints::new(b.y))?;
                }
                obj.set_line_cap(PdfPageObjectLineCap::Round)?;
                objects.add_path_object(obj)?;
            }
            Prim::Stroke { pts, closed, width, color: c } if pts.len() >= 2 => {
                // A run of strokes alike (a dashed or patterned line) becomes one path
                // of many subpaths: one object, not hundreds.
                let pts: Vec<Pos2> = pts.iter().map(|p| m.map(*p)).collect();
                let mut obj = path_object(doc, &pts, closed, Some((c, width * k)), None)?;
                while let Some(Prim::Stroke { pts, closed, width: w2, color: c2 }) = prims.peek() {
                    if *w2 != width || *c2 != c || pts.len() < 2 {
                        break;
                    }
                    let first = m.map(pts[0]);
                    obj.move_to(PdfPoints::new(first.x), PdfPoints::new(first.y))?;
                    for p in &pts[1..] {
                        let p = m.map(*p);
                        obj.line_to(PdfPoints::new(p.x), PdfPoints::new(p.y))?;
                    }
                    if *closed {
                        obj.close_path()?;
                    }
                    prims.next();
                }
                objects.add_path_object(obj)?;
            }
            Prim::Fill { pts, color: c, alpha, outline } if pts.len() >= 3 => {
                let pts: Vec<Pos2> = pts.iter().map(|p| m.map(*p)).collect();
                let outline = outline.map(|(oc, w)| (oc, w * k));
                let obj = path_object(doc, &pts, true, outline, Some(color(c, alpha)))?;
                objects.add_path_object(obj)?;
            }
            Prim::Text { origin, text, size, color: c, bold, italic } => {
                let (runs, lost) = font_runs(&text);
                fonts.missing += lost;
                let mut x = 0.0;
                for (key, run) in runs {
                    let f = match key {
                        None => fonts.helvetica[bold as usize + 2 * italic as usize],
                        Some(i) => {
                            // Characters beyond WinAnsi in a bold or italic note: the styled
                            // face of the Helvetica-metric font where the system has one.
                            let styled = (i == 0 && (bold || italic)).then(|| crate::fonts::styled(bold, italic)).flatten().filter(|f| run.chars().all(|c| f.covers(c)));
                            let key = (i, styled.is_some() && bold, styled.is_some() && italic);
                            match fonts.embedded.get(&key) {
                                Some(t) => *t,
                                None => {
                                    let path = styled.map_or(&crate::fonts::faces()[i].path, |f| &f.path);
                                    let t = doc.fonts_mut().load_true_type_from_file(path, true)?;
                                    fonts.embedded.insert(key, t);
                                    t
                                }
                            }
                        }
                    };
                    let o = m.map(origin) + ux * (x * k);
                    x += crate::annot::text_width_b(&run, size, bold);
                    let mut obj = PdfPageTextObject::new(doc, &run, f, PdfPoints::new(size * k))?;
                    obj.set_fill_color(color(c, 255))?;
                    obj.apply_matrix(PdfMatrix::new(ux.x, ux.y, uy.x, uy.y, o.x, o.y))?;
                    objects.add_text_object(obj)?;
                }
            }
            Prim::Image { rect, image } => {
                // Image space is the unit square with the picture's top edge at y = 1.
                let (bl, br, tl) = (m.map(rect.left_bottom()), m.map(rect.right_bottom()), m.map(rect.left_top()));
                let img = images.get(&image, rect.size() * k)?;
                let mut obj = PdfPageImageObject::new(doc, img)?;
                obj.apply_matrix(PdfMatrix::new(br.x - bl.x, br.y - bl.y, tl.x - bl.x, tl.y - bl.y, bl.x, bl.y))?;
                objects.add_image_object(obj)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// A flattened copy of `src` with `annots` drawn in, for the command line's renders.
pub fn flatten(pdfium: &Pdfium, src: &Path, out: &Path, annots: &[Annot]) -> anyhow::Result<usize> {
    let images = load_images(annots)?;
    export(pdfium, src, out, annots, &images, ExportMode::Flatten, "Greenline")
}

/// Writes `annots` onto a copy of `src` and saves it to `out`. Returns how many
/// characters had no font. Annotations already in the PDF are left as they are
/// (PDFium's flatten can resize pages and drop comments).
pub fn export(
    pdfium: &Pdfium,
    src: &Path,
    out: &Path,
    annots: &[Annot],
    images: &HashMap<String, image::RgbaImage>,
    mode: ExportMode,
    author: &str,
) -> anyhow::Result<usize> {
    let mut doc = pdfium.load_pdf_from_file(src, None)?;
    let f = doc.fonts_mut();
    let helvetica = [f.helvetica(), f.helvetica_bold(), f.helvetica_oblique(), f.helvetica_bold_oblique()];
    let mut fonts = TextFonts { helvetica, embedded: Default::default(), missing: 0 };
    let mut images = Images { src: images, scaled: HashMap::new() };
    let n = doc.pages().len();

    let mut pages: Vec<usize> = annots.iter().map(|a| a.page).collect();
    pages.sort();
    pages.dedup();
    for pi in pages {
        if pi >= n as usize {
            continue;
        }
        let mut page = doc.pages().get(pi as PdfPageIndex)?;
        // Regenerating the content stream after every object is quadratic: once at the end.
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let m = ViewToUser::for_page(&page)?;
        for a in annots.iter().filter(|a| a.page == pi) {
            match mode {
                ExportMode::Flatten => emit_prims(&mut doc, page.objects_mut(), &m, a.prims(), &mut fonts, &mut images)?,
                ExportMode::Editable => {
                    let b = a.bounds().expand(a.width + 2.0);
                    let corners = [b.left_top(), b.right_top(), b.right_bottom(), b.left_bottom()].map(|p| m.map(p));
                    let (mut lo, mut hi) = (corners[0], corners[0]);
                    for c in &corners[1..] {
                        lo = lo.min(*c);
                        hi = hi.max(*c);
                    }
                    let mut stamp = page.annotations_mut().create_stamp_annotation()?;
                    stamp.set_bounds(PdfRect::new(PdfPoints::new(lo.y), PdfPoints::new(lo.x), PdfPoints::new(hi.y), PdfPoints::new(hi.x)))?;
                    stamp.set_creator(author)?;
                    stamp.set_is_printed(true)?;
                    stamp.set_contents(a.label().trim())?;
                    emit_prims(&mut doc, stamp.objects_mut(), &m, a.prims(), &mut fonts, &mut images)?;
                }
            }
        }
        page.regenerate_content()?;
    }

    // Unique per process and export so concurrent exports can't share a temp file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = out.with_extension(format!("pdf.{}-{seq}.part", std::process::id()));
    let saved = doc.save_to_file(&tmp).map_err(anyhow::Error::from).and_then(|_| Ok(std::fs::rename(&tmp, out)?));
    if saved.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    saved?;
    Ok(fonts.missing)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::annot::{Annot, Kind};
    use egui::Rect;

    /// PDFium can be bound only once per process and isn't thread-safe: tests
    /// share one instance and take turns using it.
    pub(crate) static PDFIUM_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) fn test_pdfium() -> &'static Pdfium {
        crate::pdf::pdfium().unwrap()
    }

    fn annot(page: usize, kind: Kind) -> Annot {
        Annot { id: 0, page, color: [255, 0, 0], width: 2.0, kind, wrap_w: None, group: None, style: Default::default() }
    }

    /// Exports onto pages with every /Rotate value and an offset CropBox, then
    /// re-renders the result and checks the marks land where the viewer drew them.
    #[test]
    fn export_lands_in_view_coordinates() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.pdf");
        let out = dir.join("out.pdf");

        let mut doc = pdfium.create_new_pdf().unwrap();
        let rots = [
            PdfPageRenderRotation::None,
            PdfPageRenderRotation::Degrees90,
            PdfPageRenderRotation::Degrees180,
            PdfPageRenderRotation::Degrees270,
        ];
        for (i, r) in rots.iter().enumerate() {
            let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0), PdfPoints::new(600.0)))
                .unwrap();
            page.set_rotation(*r);
            if i == 3 {
                page.boundaries_mut()
                    .set_crop(PdfRect::new(PdfPoints::new(50.0), PdfPoints::new(30.0), PdfPoints::new(560.0), PdfPoints::new(380.0)))
                    .unwrap();
            }
        }
        doc.save_to_file(&src).unwrap();
        drop(doc);

        let probe = Rect::from_min_max(pos2(40.0, 50.0), pos2(140.0, 110.0));
        let mut annots = Vec::new();
        for p in 0..4 {
            annots.push(annot(p, Kind::Highlight { rect: probe }));
            annots.push(annot(p, Kind::Text { pos: pos2(40.0, 200.0), box_w: 300.0, text: "WWWWWWWWWWWWWWWWWW".into(), size: 14.0 }));
        }
        for mode in [ExportMode::Flatten, ExportMode::Editable] {
        export(pdfium, &src, &out, &annots, &HashMap::new(), mode, "test").unwrap();

        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        for p in 0..4 {
            let page = doc.pages().get(p as PdfPageIndex).unwrap();
            let expected = if mode == ExportMode::Editable { 2 } else { 0 };
            assert_eq!(page.annotations().len(), expected, "{mode:?}");
            for a in page.annotations().iter() {
                assert!(a.is_printed(), "editable markups must print");
                assert_eq!(a.creator().as_deref(), Some("test"));
            }
            let (w, h) = (page.width().value.round() as usize, page.height().value.round() as usize);
            let img = render_region(&doc, p, 1.0, [0, 0], [w, h]).unwrap();
            let px = |x: usize, y: usize| img.pixels[y * w + x];
            let c = px(90, 80);
            assert!(c.r() > 200 && c.g() < 230 && c.g() > 100, "{mode:?} page {p}: highlight centre {c:?}");
            let outside = px(200, 80);
            assert!(outside.g() > 245, "page {p}: outside highlight {outside:?}");
            // Text runs left-to-right as displayed: ink along the line, little above/below.
            let ink = |x0: usize, x1: usize, y0: usize, y1: usize| {
                (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| px(x, y).g() < 120).count()
            };
            let line = ink(50, 250, 205, 222);
            assert!(line > 300, "{mode:?} page {p}: text ink {line}");
            assert!(ink(50, 250, 230, 280) < 10, "page {p}: ink below text line");
        }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Stamps land in place on rotated pages, keep their transparency (the
    /// highlight shows through), stay upright, and the file stays small.
    #[test]
    fn export_images_with_alpha_and_orientation() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("src.pdf"), dir.join("out.pdf"));
        let mut doc = pdfium.create_new_pdf().unwrap();
        for r in [PdfPageRenderRotation::None, PdfPageRenderRotation::Degrees90] {
            let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0), PdfPoints::new(600.0))).unwrap();
            page.set_rotation(r);
        }
        doc.save_to_file(&src).unwrap();
        drop(doc);

        // 200x100 px: top half opaque blue, bottom half transparent.
        let img = image::RgbaImage::from_fn(200, 100, |_, y| if y < 50 { image::Rgba([0, 0, 255, 255]) } else { image::Rgba([0, 0, 0, 0]) });
        let images = HashMap::from([("ab.png".to_string(), img)]);
        let rect = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let mut annots = Vec::new();
        for p in 0..2 {
            annots.push(annot(p, Kind::Highlight { rect }));
            annots.push(annot(p, Kind::Image { rect, image: "ab.png".into(), name: "Seal".into() }));
        }
        for mode in [ExportMode::Flatten, ExportMode::Editable] {
            export(pdfium, &src, &out, &annots, &images, mode, "test").unwrap();
            let size = std::fs::metadata(&out).unwrap().len();
            assert!(size < 40_000, "{mode:?}: image stored uncompressed? {size} bytes");
            let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
            for p in 0..2 {
                let page = doc.pages().get(p as PdfPageIndex).unwrap();
                if mode == ExportMode::Editable {
                    let contents: Vec<_> = page.annotations().iter().filter_map(|a| a.contents()).collect();
                    assert!(contents.iter().any(|c| c == "Seal"), "{contents:?}");
                }
                let (w, h) = (page.width().value.round() as usize, page.height().value.round() as usize);
                let img = render_region(&doc, p, 1.0, [0, 0], [w, h]).unwrap();
                let px = |x: usize, y: usize| img.pixels[y * w + x];
                let top = px(200, 125);
                assert!(top.b() > 200 && top.r() < 60 && top.g() < 60, "{mode:?} page {p}: opaque top half {top:?}");
                let bottom = px(200, 175);
                assert!(bottom.r() > 200 && bottom.g() < 230 && bottom.b() < 230, "{mode:?} page {p}: highlight shows through {bottom:?}");
                assert!(px(200, 250).g() > 245, "{mode:?} page {p}: nothing outside");
            }
        }
        let missing = annot(0, Kind::Image { rect, image: "ab.png".into(), name: "Seal".into() });
        assert!(export(pdfium, &src, &out, &[missing], &HashMap::new(), ExportMode::Flatten, "test").is_err(), "missing image must fail");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An L-shaped area exports filled inside the L only, with its value text,
    /// and a length with its ticks: measurements go through prims like any markup.
    #[test]
    fn export_measurements() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("src.pdf"), dir.join("out.pdf"));
        let mut doc = pdfium.create_new_pdf().unwrap();
        doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0), PdfPoints::new(600.0))).unwrap();
        doc.save_to_file(&src).unwrap();
        drop(doc);
        let l = vec![pos2(50.0, 50.0), pos2(350.0, 50.0), pos2(350.0, 150.0), pos2(150.0, 150.0), pos2(150.0, 350.0), pos2(50.0, 350.0)];
        let mut annots = vec![
            annot(0, Kind::Measure { pts: l, area: true, name: "Roof A".into(), value: String::new(), size: 10.0 }),
            annot(0, Kind::Measure { pts: vec![pos2(50.0, 500.0), pos2(350.0, 500.0)], area: false, name: String::new(), value: String::new(), size: 10.0 }),
        ];
        let scales = vec![crate::measure::Scale::from_ratio(0, None, 48.0, "1/4\" = 1'-0\"")];
        crate::measure::relabel(&mut annots, &scales, crate::measure::Units::Imperial);
        export(pdfium, &src, &out, &annots, &HashMap::new(), ExportMode::Flatten, "test").unwrap();
        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        let text = page.text().unwrap().all();
        assert!(text.contains("Roof A: 154.32 sf") && text.contains("16'-8\""), "{text:?}");
        let img = render_region(&doc, 0, 1.0, [0, 0], [400, 600]).unwrap();
        let px = |x: usize, y: usize| img.pixels[y * 400 + x];
        assert!(px(300, 100).g() < 245, "inside the L's arm is tinted: {:?}", px(300, 100));
        assert!(px(300, 300).r() > 250 && px(300, 300).g() > 250, "the notch stays white: {:?}", px(300, 300));
        assert!(px(50, 492).g() < 200, "end tick: {:?}", px(50, 492));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A filled cloud exports its fill out into the scallops; an open polyline has none.
    #[test]
    fn export_fills_and_polylines() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("src.pdf"), dir.join("out.pdf"));
        let mut doc = pdfium.create_new_pdf().unwrap();
        doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0), PdfPoints::new(600.0))).unwrap();
        doc.save_to_file(&src).unwrap();
        drop(doc);
        let mut cloud = annot(0, Kind::Cloud { rect: egui::Rect::from_min_max(pos2(50.0, 50.0), pos2(350.0, 250.0)) });
        cloud.style.fill = Some(crate::annot::Fill { color: [0, 0, 255], alpha: 255 });
        let poly = annot(0, Kind::Poly { pts: vec![pos2(50.0, 400.0), pos2(350.0, 400.0), pos2(200.0, 550.0)], closed: false });
        let mut hatched = annot(0, Kind::Rect { rect: egui::Rect::from_min_max(pos2(20.0, 280.0), pos2(380.0, 380.0)) });
        hatched.style.hatch = Some(crate::patterns::HatchFill { pattern: std::sync::Arc::new(crate::patterns::builtin_hatches()[2].clone()), scale: 1.0 });
        export(pdfium, &src, &out, &[cloud, poly, hatched], &HashMap::new(), ExportMode::Flatten, "test").unwrap();
        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        let img = render_region(&doc, 0, 1.0, [0, 0], [400, 600]).unwrap();
        let px = |x: usize, y: usize| img.pixels[y * 400 + x];
        assert!(px(200, 150).b() > 200 && px(200, 150).r() < 50, "inside is blue: {:?}", px(200, 150));
        assert!(px(200, 410).r() > 250 && px(200, 410).b() > 250, "an open polyline isn't filled: {:?}", px(200, 410));
        assert!(px(200, 400).g() < 200, "its first segment is drawn: {:?}", px(200, 400));
        // Horizontal hatch lines every 8 points from the top of the rectangle: dark at 284, light at 288.
        assert!(px(200, 284).g() < 200 && px(200, 288).g() > 230, "{:?} {:?}", px(200, 284), px(200, 288));
        let page = doc.pages().get(0).unwrap();
        assert!(page.objects().len() < 20, "the hatch is one path, not hundreds: {}", page.objects().len());
        drop(page);
        drop(doc);
        // A long dotted line: one path object too, and quick.
        let mut dotted = annot(0, Kind::Poly { pts: (0..400).map(|i| pos2(10.0 + (i % 20) as f32 * 19.0, 10.0 + (i / 20) as f32 * 29.0)).collect(), closed: false });
        dotted.style.line = Some(std::sync::Arc::new(crate::patterns::builtin_line_types()[1].clone()));
        assert!(dotted.prims().len() > 2000, "{}", dotted.prims().len());
        let t = std::time::Instant::now();
        export(pdfium, &src, &out, &[dotted], &HashMap::new(), ExportMode::Flatten, "test").unwrap();
        assert!(t.elapsed().as_secs_f32() < 2.0, "{:?}", t.elapsed());
        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        assert!(doc.pages().get(0).unwrap().objects().len() < 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Sizes read from the page tree match the loaded pages', /Rotate and CropBox included.
    #[test]
    fn page_sizes_match_loaded_pages() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.pdf");
        let mut doc = pdfium.create_new_pdf().unwrap();
        for (i, r) in [PdfPageRenderRotation::None, PdfPageRenderRotation::Degrees90, PdfPageRenderRotation::Degrees270].into_iter().enumerate() {
            let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0 + i as f32), PdfPoints::new(600.0))).unwrap();
            page.set_rotation(r);
            if i == 2 {
                page.boundaries_mut().set_crop(PdfRect::new(PdfPoints::new(50.0), PdfPoints::new(30.0), PdfPoints::new(560.0), PdfPoints::new(380.0))).unwrap();
            }
        }
        doc.save_to_file(&src).unwrap();
        drop(doc);
        let doc = pdfium.load_pdf_from_file(&src, None).unwrap();
        let loaded: Vec<Vec2> = doc.pages().iter().map(|p| vec2(p.width().value, p.height().value)).collect();
        assert_eq!(page_sizes(&doc).unwrap(), loaded);
        assert_eq!(loaded[1], vec2(600.0, 401.0), "rotated a quarter turn: width and height swap");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Rendered rows (padded or not) become opaque pixels as they were; a
    /// thumbnail shrunk from a render averages what it covers.
    #[test]
    fn rendered_pixels_and_shrunk_thumbnails() {
        // 2x2 pixels in rows padded to 12 bytes.
        let raw = [10, 20, 30, 0, 40, 50, 60, 128, 9, 9, 9, 9, 70, 80, 90, 255, 1, 2, 3, 4, 9, 9, 9, 9];
        let img = opaque_image(&raw, [2, 2]);
        assert_eq!(img.pixels, [egui::Color32::from_rgb(10, 20, 30), egui::Color32::from_rgb(40, 50, 60), egui::Color32::from_rgb(70, 80, 90), egui::Color32::from_rgb(1, 2, 3)]);
        // A one-pixel line on white shrinks to a clear grey line, not to nothing;
        // black and white stripes to a dark grey; a white page stays white.
        let line = egui::ColorImage::new([80, 80], (0..80 * 80).map(|i| if i % 80 == 33 { egui::Color32::BLACK } else { egui::Color32::WHITE }).collect());
        let t = shrink(&line, [8, 8]);
        assert!(t.pixels[3].r() < 140 && t.pixels[2].r() == 255 && t.pixels[4].r() == 255, "{:?}", &t.pixels[..8]);
        let stripes = egui::ColorImage::new([64, 64], (0..64 * 64).map(|i| if i % 2 == 0 { egui::Color32::BLACK } else { egui::Color32::WHITE }).collect());
        let t = shrink(&stripes, [8, 8]);
        assert!(t.pixels.iter().all(|c| (55..=70).contains(&c.r())), "{:?}", t.pixels[0]);
        let white = shrink(&egui::ColorImage::filled([300, 200], egui::Color32::WHITE), [42, 28]);
        assert!(white.pixels.iter().all(|c| *c == egui::Color32::WHITE) && white.size == [42, 28]);
        assert_eq!(thumb_size(vec2(2592.0, 1728.0)).1, [420, 280]);
    }

    /// A search takes turns with thumbnails, behind base renders; a stale one is dropped.
    #[test]
    fn searches_share_the_worker() {
        let keep = Mutex::new(Keep { base: HashSet::from([0]), thumbs: HashSet::from([1, 2]), search: 7, ..Default::default() });
        let thumb = |page| Req::Render { generation: 1, page, scale: 0.1, origin: [0, 0], size: [10, 10], layer: Layer::Thumb };
        let search = |id| Req::Search { generation: 1, id, pages: VecDeque::from([0, 1, 2]), query: "x".into() };
        let mut q = VecDeque::from([search(6), thumb(1), thumb(2), search(7), Req::Render { generation: 1, page: 0, scale: 1.0, origin: [0, 0], size: [10, 10], layer: Layer::Base }]);
        let mut turn = false;
        let mut order = Vec::new();
        while let Some(r) = next_request(&mut q, &keep, &mut turn) {
            order.push(match r {
                Req::Render { layer: Layer::Base, .. } => "base".to_string(),
                Req::Render { page, .. } => format!("thumb{page}"),
                Req::Search { id, .. } => format!("search{id}"),
                _ => "?".into(),
            });
        }
        assert_eq!(order, ["base", "thumb1", "search7", "thumb2"]);
    }

    /// Bold and italic notes export in Helvetica-Bold / -Oblique.
    #[test]
    fn export_bold_and_italic_fonts() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test6-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("src.pdf"), dir.join("out.pdf"));
        let mut doc = pdfium.create_new_pdf().unwrap();
        doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0), PdfPoints::new(600.0))).unwrap();
        doc.save_to_file(&src).unwrap();
        drop(doc);
        let mut b = annot(0, Kind::Text { pos: pos2(40.0, 40.0), box_w: 300.0, text: "Bold note".into(), size: 14.0 });
        b.style.bold = true;
        let mut i = annot(0, Kind::Text { pos: pos2(40.0, 140.0), box_w: 300.0, text: "Italic note".into(), size: 14.0 });
        i.style.italic = true;
        export(pdfium, &src, &out, &[b, i], &HashMap::new(), ExportMode::Flatten, "test").unwrap();
        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        let fonts: Vec<String> = page.objects().iter().filter_map(|o| o.as_text_object().map(|t| t.font().name())).collect();
        assert!(fonts.iter().any(|f| f.contains("Bold")) && fonts.iter().any(|f| f.contains("Oblique")), "{fonts:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A new blank document has the pages asked for, at the size asked for.
    #[test]
    fn creates_blank_documents() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test7-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blank.pdf");
        create_blank(pdfium, &path, vec2(1224.0, 792.0), 3).unwrap();
        let doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        assert_eq!(doc.pages().len(), 3);
        let p = doc.pages().get(2).unwrap();
        assert_eq!((p.width().value, p.height().value), (1224.0, 792.0));
        assert!(std::fs::read_dir(&dir).unwrap().count() == 1, "no temporary file left");
        assert!(create_blank(pdfium, &path, vec2(612.0, 792.0), 1).unwrap_err().to_string().contains("already exists"), "never replaces a file");
        assert_eq!(pdfium.load_pdf_from_file(&path, None).unwrap().pages().len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Search finds text case-insensitively and reports its box in view points.
    #[test]
    fn search_finds_text_in_view_coordinates() {
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let mut doc = pdfium.create_new_pdf().unwrap();
        let font = doc.fonts_mut().helvetica();
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(400.0), PdfPoints::new(600.0))).unwrap();
        // Baseline 100 points up from the bottom: 500 down in view points.
        page.objects_mut().create_text_object(PdfPoints::new(50.0), PdfPoints::new(100.0), "Check the SILL flashing; sill pan", font, PdfPoints::new(12.0)).unwrap();
        let (hits, more) = crate::pdf::search_page(&page, "sill").unwrap();
        assert!(!more);
        assert_eq!(hits.len(), 2);
        let r = hits[0][0];
        assert!(r.min.y > 485.0 && r.max.y < 502.0 && r.min.x > 50.0, "{r:?}");
        assert!(crate::pdf::search_page(&page, "  ").unwrap().0.is_empty());
    }

    /// Page with an inherited MediaBox and a visible, non-printing annotation:
    /// export must keep the page size and leave the annotation visible.
    #[test]
    fn export_preserves_inherited_boxes_and_visible_annotations() {
        let stream = b"1 0 0 rg 0 0 100 50 re f";
        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1000 1400] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Resources << >> /Annots [4 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Square /Rect [40 50 140 100] /F 0 /AP << /N 5 0 R >> >>".to_vec(),
            [format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 50] /Resources << >> /Length {} >>\nstream\n", stream.len()).as_bytes(), stream, b"\nendstream"].concat(),
        ];
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(data.len());
            data.extend(format!("{} 0 obj\n", i + 1).as_bytes());
            data.extend(o);
            data.extend(b"\nendobj\n");
        }
        let xref = data.len();
        data.extend(b"xref\n0 6\n0000000000 65535 f \n");
        for o in offsets {
            data.extend(format!("{o:010} 00000 n \n").as_bytes());
        }
        data.extend(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF").as_bytes());

        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let dir = std::env::temp_dir().join(format!("greenline-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("src.pdf"), dir.join("out.pdf"));
        std::fs::write(&src, &data).unwrap();
        let note = annot(0, Kind::Text { pos: pos2(300.0, 300.0), box_w: 400.0, text: "Δ ≥ 5\u{a0}mm, ⌀ 25 ☐ ok".into(), size: 20.0 });
        assert_eq!(export(pdfium, &src, &out, &[note], &HashMap::new(), ExportMode::Flatten, "test").unwrap(), 0);

        let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        assert_eq!((page.width().value, page.height().value), (1000.0, 1400.0));
        assert_eq!(page.annotations().len(), 1);
        let text = page.text().unwrap().all();
        assert!(["Δ", "≥", "5 mm", "Ø", "☐", "ok"].iter().all(|t| text.contains(t)), "{text:?}");
        let img = render_region(&doc, 0, 1.0, [0, 0], [1000, 1400]).unwrap();
        // Square appearance: user (40..140, 50..100) => view y = 1400 - user y.
        let c = img.pixels[1325 * 1000 + 90];
        assert!(c.r() > 200 && c.g() < 50, "existing annotation missing: {c:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod sample {
    use super::tests::{test_pdfium, PDFIUM_TESTS};
    use super::*;
    use crate::annot::{Annot, Kind};

    /// GREENLINE_SAMPLE=/path/to.pdf cargo test --release -- --ignored --nocapture
    #[test]
    #[ignore]
    fn export_real_document() {
        let src = PathBuf::from(std::env::var("GREENLINE_SAMPLE").unwrap());
        let out = src.with_file_name("sample_markup.pdf");
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let mk = |page, kind| Annot { id: 0, page, color: [228, 0, 0], width: 2.0, kind, wrap_w: None, group: None, style: Default::default() };
        let annots = vec![
            mk(0, Kind::Callout { anchor: pos2(600.0, 500.0), pos: pos2(800.0, 300.0), box_w: 192.0, text: "Confirm drain locations vs. structural — typ. all roof areas".into(), size: 12.0, leaders: vec![pos2(300.0, 620.0)] }),
            mk(0, Kind::Cloud { rect: egui::Rect::from_min_max(pos2(500.0, 420.0), pos2(700.0, 560.0)) }),
            mk(3, Kind::Highlight { rect: egui::Rect::from_min_max(pos2(100.0, 100.0), pos2(400.0, 160.0)) }),
        ];
        for (mode, out) in [(ExportMode::Flatten, out.clone()), (ExportMode::Editable, src.with_file_name("sample_markup_editable.pdf"))] {
            let t = std::time::Instant::now();
            export(pdfium, &src, &out, &annots, &HashMap::new(), mode, "test").unwrap();
            eprintln!("{mode:?}: exported {} in {:?}", out.display(), t.elapsed());
        }
    }
}

#[cfg(test)]
mod bench {
    use super::tests::{test_pdfium, PDFIUM_TESTS};
    use super::*;
    use std::time::Instant;

    /// GREENLINE_SAMPLE=/path/to.pdf cargo test --release bench_ -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_worker_steps() {
        let src = PathBuf::from(std::env::var("GREENLINE_SAMPLE").unwrap());
        let _guard = PDFIUM_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = test_pdfium();
        let t = Instant::now();
        let doc = pdfium.load_pdf_from_file(&src, None).unwrap();
        eprintln!("load: {:?}", t.elapsed());
        let t = Instant::now();
        let a: Vec<Vec2> = doc.pages().iter().map(|p| vec2(p.width().value, p.height().value)).collect();
        eprintln!("sizes by loading every page ({}): {:?}", a.len(), t.elapsed());
        let t = Instant::now();
        let b: Vec<Vec2> = doc.pages().page_sizes().unwrap().iter().map(|r| vec2(r.width().value, r.height().value)).collect();
        eprintln!("sizes by index: {:?} same={}", t.elapsed(), a == b);
        for pi in [0usize, 5, 10] {
            let t = Instant::now();
            let p = doc.pages().get(pi as PdfPageIndex).unwrap();
            let load = t.elapsed();
            let s = vec2(p.width().value, p.height().value);
            let scale = 4096.0 / s.x.max(s.y);
            let size = [(s.x * scale).round() as usize, (s.y * scale).round() as usize];
            let cfg = PdfRenderConfig::new().set_fixed_size(size[0] as Pixels, size[1] as Pixels).render_annotations(true);
            let mut bmp = PdfBitmap::empty(size[0] as Pixels, size[1] as Pixels, PdfBitmapFormat::BGRA).unwrap();
            let t = Instant::now();
            p.render_into_bitmap_with_config(&mut bmp, &cfg).unwrap();
            let r1 = t.elapsed();
            let t = Instant::now();
            p.render_into_bitmap_with_config(&mut bmp, &cfg).unwrap();
            let r2 = t.elapsed();
            let t = Instant::now();
            let rgba = bmp.as_rgba_bytes();
            let img = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
            let conv = t.elapsed();
            drop(img);
            drop(p);
            let t = Instant::now();
            let image = render_region(&doc, pi, scale, [0, 0], size).unwrap();
            let full = t.elapsed();
            let t = Instant::now();
            let _ = shrink(&image, thumb_size(s).1);
            eprintln!("  thumbnail shrunk from it: {:?}", t.elapsed());
            eprintln!("page {pi} {size:?}: load {load:?}, render {r1:?}, again on loaded page {r2:?}, convert {conv:?}; render_region {full:?}");
            let p = doc.pages().get(pi as PdfPageIndex).unwrap();
            for px in [512.0f32, 1024.0, 1448.0, 2048.0, 2896.0, 4096.0] {
                let scale = px / s.x.max(s.y);
                let size = [(s.x * scale).round() as usize, (s.y * scale).round() as usize];
                let cfg = PdfRenderConfig::new().set_fixed_size(size[0] as Pixels, size[1] as Pixels).render_annotations(true);
                let mut bmp = PdfBitmap::empty(size[0] as Pixels, size[1] as Pixels, PdfBitmapFormat::BGRA).unwrap();
                let t = Instant::now();
                p.render_into_bitmap_with_config(&mut bmp, &cfg).unwrap();
                eprintln!("   {px}px: {:?}", t.elapsed());
            }
        }
    }
}
