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
}

pub enum Req {
    Open { generation: u64, path: PathBuf },
    /// Render the region `origin..origin+size` (pixels) of the page drawn at `scale` px/pt.
    Render { generation: u64, page: usize, scale: f32, origin: [i32; 2], size: [usize; 2], layer: Layer },
    Export { src: PathBuf, out: PathBuf, annots: Vec<Annot>, mode: ExportMode, author: String },
}

pub enum Resp {
    Opened { generation: u64, pages: Vec<Vec2> },
    OpenFailed { generation: u64, error: String },
    Rendered { generation: u64, page: usize, scale: f32, origin: [i32; 2], layer: Layer, image: egui::ColorImage },
    RenderFailed { generation: u64, page: usize, layer: Layer, error: String },
    /// Output path and the number of characters that had no font.
    Exported(Result<(PathBuf, usize), String>),
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

fn find_pdfium() -> anyhow::Result<PathBuf> {
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
    let mut doc: Option<(u64, PdfDocument)> = None;
    let mut queue: VecDeque<Req> = VecDeque::new();
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
        let Some(req) = next_request(&mut queue, &keep) else { continue };
        let resp = match req {
            Req::Open { generation, path } => {
                doc = None;
                match pdfium.load_pdf_from_file(&path, None) {
                    Ok(d) => {
                        let pages = d.pages().iter().map(|p| vec2(p.width().value, p.height().value)).collect();
                        doc = Some((generation, d));
                        Resp::Opened { generation, pages }
                    }
                    Err(e) => Resp::OpenFailed { generation, error: format!("{e:?}") },
                }
            }
            Req::Render { generation, page, scale, origin, size, layer } => {
                let Some((g, d)) = &doc else { continue };
                if *g != generation {
                    continue;
                }
                match render_region(d, page, scale, origin, size) {
                    Ok(image) => Resp::Rendered { generation, page, scale, origin, layer, image },
                    Err(e) => Resp::RenderFailed { generation, page, layer, error: e.to_string() },
                }
            }
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
/// wants, then wanted thumbnails.
fn next_request(queue: &mut VecDeque<Req>, keep: &Mutex<Keep>) -> Option<Req> {
    if let Some(i) = queue.iter().position(|r| !matches!(r, Req::Render { .. })) {
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
    queue.pop_front()
}

fn render_region(doc: &PdfDocument, page: usize, scale: f32, origin: [i32; 2], size: [usize; 2]) -> anyhow::Result<egui::ColorImage> {
    let p = doc.pages().get(page as PdfPageIndex)?;
    let full_w = (p.width().value * scale).round().max(1.0) as Pixels;
    let full_h = (p.height().value * scale).round().max(1.0) as Pixels;
    let cfg = PdfRenderConfig::new()
        .set_fixed_size(full_w, full_h)
        .set_origin(-origin[0] as Pixels, -origin[1] as Pixels)
        .render_annotations(true);
    let mut bmp = PdfBitmap::empty(size[0] as Pixels, size[1] as Pixels, PdfBitmapFormat::BGRA)?;
    p.render_into_bitmap_with_config(&mut bmp, &cfg)?;
    let rgba = bmp.as_rgba_bytes();
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, &rgba))
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
fn load_images(annots: &[Annot]) -> anyhow::Result<HashMap<String, image::RgbaImage>> {
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
    helvetica: PdfFontToken,
    embedded: std::collections::HashMap<usize, PdfFontToken>,
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
    for prim in prims {
        match prim {
            Prim::Stroke { pts, closed, width, color: c } if pts.len() >= 2 => {
                let pts: Vec<Pos2> = pts.iter().map(|p| m.map(*p)).collect();
                let obj = path_object(doc, &pts, closed, Some((c, width * k)), None)?;
                objects.add_path_object(obj)?;
            }
            Prim::Fill { pts, color: c, alpha, outline } if pts.len() >= 3 => {
                let pts: Vec<Pos2> = pts.iter().map(|p| m.map(*p)).collect();
                let outline = outline.map(|(oc, w)| (oc, w * k));
                let obj = path_object(doc, &pts, true, outline, Some(color(c, alpha)))?;
                objects.add_path_object(obj)?;
            }
            Prim::Text { origin, text, size, color: c } => {
                let (runs, lost) = font_runs(&text);
                fonts.missing += lost;
                let mut x = 0.0;
                for (key, run) in runs {
                    let f = match key {
                        None => fonts.helvetica,
                        Some(i) => match fonts.embedded.get(&i) {
                            Some(t) => *t,
                            None => {
                                let t = doc.fonts_mut().load_true_type_from_file(&crate::fonts::faces()[i].path, true)?;
                                fonts.embedded.insert(i, t);
                                t
                            }
                        },
                    };
                    let o = m.map(origin) + ux * (x * k);
                    x += crate::annot::text_width(&run, size);
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
    let helvetica = doc.fonts_mut().helvetica();
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
mod tests {
    use super::*;
    use crate::annot::{Annot, Kind};
    use egui::Rect;

    /// PDFium can be bound only once per process and isn't thread-safe: tests
    /// share one instance and take turns using it.
    pub(super) static PDFIUM_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(super) fn test_pdfium() -> &'static Pdfium {
        static PDFIUM: std::sync::OnceLock<Pdfium> = std::sync::OnceLock::new();
        PDFIUM.get_or_init(|| Pdfium::new(Pdfium::bind_to_library(find_pdfium().unwrap()).unwrap()))
    }

    fn annot(page: usize, kind: Kind) -> Annot {
        Annot { id: 0, page, color: [255, 0, 0], width: 2.0, kind, wrap_w: None, group: None }
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
        let mk = |page, kind| Annot { id: 0, page, color: [228, 0, 0], width: 2.0, kind, wrap_w: None, group: None };
        let annots = vec![
            mk(0, Kind::Callout { anchor: pos2(600.0, 500.0), pos: pos2(800.0, 300.0), box_w: 192.0, text: "Confirm drain locations vs. structural — typ. all roof areas".into(), size: 12.0 }),
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
