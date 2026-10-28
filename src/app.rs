use crate::annot::{self, Annot, Kind, Prim, Rgb};
use crate::render::{ExportMode, Layer, Req, Resp, Worker};
use crate::sidecar;
use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontFamily, FontId, Key, Painter, PointerButton, Pos2, Rect, Sense, Shape, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const GAP: f32 = 12.0;
const MAX_BASE_PX: f32 = 4096.0;
const HIT_PX: f32 = 7.0;
/// Thumbnail render size (physical px on the long side) and cache cap.
const THUMB_PX: f32 = 420.0;
const MAX_THUMBS: usize = 400;
const SCROLL_SPEED: f32 = 1.8;
/// Scroll acceleration: after SCROLL_ACCEL_DELAY s of continuous scrolling in one
/// direction, speed ramps up by SCROLL_ACCEL_RATE x/s to at most SCROLL_ACCEL_MAX x.
const SCROLL_ACCEL_DELAY: f64 = 0.35;
const SCROLL_ACCEL_RATE: f64 = 4.0;
const SCROLL_ACCEL_MAX: f64 = 8.0;
/// A pause longer than this (or reversing direction) resets acceleration.
const SCROLL_STREAK_GAP: f64 = 0.25;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tool {
    Select,
    Callout,
    Text,
    Cloud,
    Rect,
    Highlight,
    Arrow,
    Line,
    Pen,
}

const TOOLS: [(Tool, &str, Key); 9] = [
    (Tool::Select, "Select", Key::V),
    (Tool::Callout, "Callout", Key::C),
    (Tool::Text, "Text", Key::T),
    (Tool::Cloud, "Cloud", Key::K),
    (Tool::Rect, "Rect", Key::R),
    (Tool::Highlight, "Highlight", Key::H),
    (Tool::Arrow, "Arrow", Key::A),
    (Tool::Line, "Line", Key::L),
    (Tool::Pen, "Pen", Key::P),
];

const COLORS: [(Rgb, &str); 6] = [
    ([228, 0, 0], "Red"),
    ([0, 88, 220], "Blue"),
    ([0, 140, 60], "Green"),
    ([20, 20, 20], "Black"),
    ([240, 120, 0], "Orange"),
    ([255, 214, 0], "Yellow"),
];
const COLOR_KEYS: [Key; 6] = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6];
const TEXT_SIZES: [f32; 6] = [8.0, 10.0, 12.0, 14.0, 18.0, 24.0];

struct PageTex {
    tex: TextureHandle,
    scale: f32,
    origin: [i32; 2],
    size: [usize; 2],
}

#[derive(Clone, Copy, Debug)]
enum Handle {
    /// Rect corner; holds the fixed opposite corner.
    Corner(Pos2),
    BoxWidth,
    Anchor,
    A,
    B,
}

enum Drag {
    None,
    Pan,
    Create { page: usize, start: Pos2, cur: Pos2 },
    Pen { page: usize, pts: Vec<Pos2> },
    Move { id: u64, last: Pos2, before: Vec<Annot>, moved: bool },
    Handle { id: u64, handle: Handle, before: Vec<Annot> },
    /// Swallow the rest of a gesture (e.g. the click that committed a text edit).
    Ignore,
}

pub struct App {
    worker: Option<Worker>,
    status: String,
    status_until: f64,

    path: Option<PathBuf>,
    generation: u64,
    pages: Vec<Vec2>,
    page_y: Vec<f32>,
    doc_w: f32,
    doc_h: f32,

    zoom: f32,
    offset: Vec2,
    canvas: Rect,
    fit_pending: bool,
    last_view_change: f64,
    /// (streak start time, last scroll time, direction) for scroll acceleration.
    scroll_streak: (f64, f64, f32),
    view_key: (f32, Vec2),

    base: HashMap<usize, PageTex>,
    patch: HashMap<usize, PageTex>,
    req_base: HashMap<usize, f32>,
    req_patch: HashMap<usize, (f32, [i32; 2], [usize; 2])>,
    thumbs: HashMap<usize, PageTex>,
    req_thumb: HashSet<usize>,
    /// Thumbnails drawn this frame (sidebar or overview); requested after layout.
    thumbs_wanted: Vec<usize>,
    show_sidebar: bool,
    show_markups: bool,
    overview: bool,
    overview_tile: f32,
    overview_cursor: usize,
    overview_cols: usize,
    /// Scroll the overview to the cursor on the next frame.
    overview_scroll: bool,
    /// Page the sidebar last scrolled to, so it follows the view without fighting the user.
    sidebar_synced: Option<usize>,

    annots: Vec<Annot>,
    next_id: u64,
    undo: Vec<Vec<Annot>>,
    redo: Vec<Vec<Annot>>,
    dirty: bool,

    tool: Tool,
    tool_color: HashMap<Tool, usize>,
    width: f32,
    text_size: f32,
    selected: Option<u64>,
    drag: Drag,
    editing: Option<u64>,
    edit_needs_focus: bool,
    goto: Option<String>,
    show_help: bool,

    /// Screen text scale correction so Liberation Sans matches Helvetica widths,
    /// and its baseline offset as a fraction of the font size.
    text_k: Option<(f32, f32)>,

    lock: Option<std::fs::File>,
    read_only: bool,
    /// Shown while `read_only`: why markups aren't being autosaved.
    read_only_why: String,
    now: f64,
    save_retry_at: f64,
    failed: HashSet<usize>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, path: Option<PathBuf>) -> Self {
        install_fonts(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (worker, status) = match Worker::spawn(cc.egui_ctx.clone()) {
            Ok(w) => (Some(w), String::from("Ctrl+O to open a PDF (or drop one here)")),
            Err(e) => (None, format!("PDFium failed to load: {e}")),
        };
        let mut app = Self {
            worker,
            status,
            status_until: f64::INFINITY,
            path: None,
            generation: 0,
            pages: Vec::new(),
            page_y: Vec::new(),
            doc_w: 0.0,
            doc_h: 0.0,
            zoom: 1.0,
            offset: Vec2::ZERO,
            canvas: Rect::NOTHING,
            fit_pending: false,
            last_view_change: 0.0,
            scroll_streak: (0.0, f64::NEG_INFINITY, 0.0),
            view_key: (0.0, Vec2::ZERO),
            base: HashMap::new(),
            patch: HashMap::new(),
            req_base: HashMap::new(),
            req_patch: HashMap::new(),
            thumbs: HashMap::new(),
            req_thumb: HashSet::new(),
            thumbs_wanted: Vec::new(),
            show_sidebar: true,
            show_markups: false,
            overview: false,
            overview_tile: 240.0,
            overview_cursor: 0,
            overview_cols: 1,
            overview_scroll: false,
            sidebar_synced: None,
            annots: Vec::new(),
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            tool: Tool::Callout,
            tool_color: HashMap::from([(Tool::Highlight, 5)]),
            width: 2.0,
            text_size: 12.0,
            selected: None,
            drag: Drag::None,
            editing: None,
            edit_needs_focus: false,
            goto: None,
            show_help: false,
            text_k: None,
            lock: None,
            read_only: false,
            read_only_why: String::new(),
            now: 0.0,
            save_retry_at: 0.0,
            failed: HashSet::new(),
        };
        if let Some(p) = path {
            app.open(p);
        }
        app
    }

    // -----------------------------------------------------------------------
    // Document

    fn open(&mut self, path: PathBuf) {
        if self.worker.is_none() {
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        self.commit_edit();
        self.save_sidecar();
        if self.dirty && !self.read_only {
            self.toast("Autosave failed; not switching documents so markups aren't lost".into());
            return;
        }
        let w = self.worker.as_ref().unwrap();
        self.generation += 1;
        w.send(Req::Open { generation: self.generation, path: path.clone() });
        self.pages.clear();
        self.base.clear();
        self.patch.clear();
        self.req_base.clear();
        self.req_patch.clear();
        self.thumbs.clear();
        self.req_thumb.clear();
        self.thumbs_wanted.clear();
        self.overview = false;
        self.sidebar_synced = None;
        self.failed.clear();
        self.undo.clear();
        self.redo.clear();
        self.selected = None;
        self.editing = None;
        self.drag = Drag::None;
        self.annots = Vec::new();
        self.next_id = 1;
        self.lock = None;
        self.read_only = false;
        let mut why = None;
        match sidecar::lock(&path) {
            Ok(Some(l)) => self.lock = Some(l),
            Ok(None) => why = Some("open in another Redline window".to_string()),
            Err(e) => why = Some(format!("could not lock markups file ({e})")),
        }
        match sidecar::load(&path) {
            Ok(Some(s)) => {
                self.next_id = s.annots.iter().map(|a| a.id + 1).max().unwrap_or(1);
                self.annots = s.annots;
            }
            Ok(None) => {}
            // Never autosave over a markups file we couldn't read.
            Err(e) => why = Some(format!("saved markups unreadable ({e})")),
        }
        self.read_only = why.is_some();
        self.read_only_why = why.unwrap_or_default();
        self.path = Some(path);
        self.status = "Opening…".into();
    }

    fn on_opened(&mut self, ctx: &egui::Context, pages: Vec<Vec2>) {
        self.pages = pages;
        self.page_y.clear();
        let mut y = GAP;
        self.doc_w = 0.0;
        for s in &self.pages {
            self.page_y.push(y);
            y += s.y + GAP;
            self.doc_w = self.doc_w.max(s.x);
        }
        self.doc_h = y;
        self.fit_pending = true;
        let name = self.file_name();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — Redline")));
        let n = self.annots.len();
        if self.read_only {
            self.toast(format!("{name}: {} pages", self.pages.len()));
            return;
        }
        self.toast(if n > 0 { format!("{name}: {} pages, restored {n} markups", self.pages.len()) } else { format!("{name}: {} pages", self.pages.len()) });
    }

    fn file_name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn export(&mut self, mode: ExportMode, ask: bool) {
        self.commit_edit();
        let (Some(w), Some(src)) = (&self.worker, &self.path) else { return };
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let suffix = if mode == ExportMode::Editable { "_markup_editable" } else { "_markup" };
        let default = src.with_file_name(format!("{stem}{suffix}.pdf"));
        let out = if ask {
            let mut d = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(default.file_name().unwrap().to_string_lossy());
            if let Some(dir) = src.parent() {
                d = d.set_directory(dir);
            }
            match d.save_file() {
                Some(p) => p,
                None => return,
            }
        } else {
            default
        };
        // Compare real paths so a symlinked folder can't alias the original.
        let real_out = out.parent().and_then(|d| d.canonicalize().ok()).zip(out.file_name()).map(|(d, f)| d.join(f)).unwrap_or_else(|| out.clone());
        if real_out == *src || out.canonicalize().is_ok_and(|o| o == *src) {
            self.toast("Refusing to overwrite the original PDF".into());
            return;
        }
        let annots: Vec<Annot> = self.annots.iter().filter(|a| a.text_size().is_none() || !text_of(a).trim().is_empty()).cloned().collect();
        w.send(Req::Export { src: src.clone(), out, annots, mode, author: author() });
        self.status = "Exporting…".into();
        self.status_until = f64::INFINITY;
    }

    fn save_sidecar(&mut self) {
        if !self.dirty || self.read_only {
            return;
        }
        let Some(p) = &self.path else {
            self.dirty = false;
            return;
        };
        match sidecar::save(p, &self.annots) {
            Ok(()) => self.dirty = false,
            Err(e) => {
                // Stays dirty; retried on the next change or document switch.
                self.status = format!("AUTOSAVE FAILED: {e}");
                self.status_until = f64::INFINITY;
                self.save_retry_at = self.now + 5.0;
            }
        }
    }

    fn toast(&mut self, s: String) {
        self.status = s;
        self.status_until = f64::NAN; // set to now+4s on next frame
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let Some(w) = &self.worker else { return };
        let msgs: Vec<Resp> = w.rx.try_iter().collect();
        for m in msgs {
            match m {
                Resp::Opened { generation, pages } if generation == self.generation => self.on_opened(ctx, pages),
                Resp::OpenFailed { generation, error } if generation == self.generation => {
                    self.status = format!("Could not open: {error}");
                    self.path = None;
                }
                Resp::Rendered { generation, page, scale, origin, layer, image } if generation == self.generation => {
                    let size = image.size;
                    let mip = TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR };
                    let opts = if layer == Layer::Patch { TextureOptions::LINEAR } else { mip };
                    let tex = ctx.load_texture(format!("p{page}{layer:?}"), image, opts);
                    let t = PageTex { tex, scale, origin, size };
                    match layer {
                        Layer::Patch => {
                            self.req_patch.remove(&page);
                            self.patch.insert(page, t);
                        }
                        Layer::Base => {
                            self.req_base.remove(&page);
                            self.base.insert(page, t);
                        }
                        Layer::Thumb => {
                            self.req_thumb.remove(&page);
                            self.thumbs.insert(page, t);
                        }
                    }
                }
                Resp::RenderFailed { generation, page, layer, error } if generation == self.generation => {
                    match layer {
                        Layer::Patch => self.req_patch.remove(&page).is_some(),
                        Layer::Base => self.req_base.remove(&page).is_some(),
                        Layer::Thumb => self.req_thumb.remove(&page),
                    };
                    self.failed.insert(page);
                    self.toast(format!("Page {} failed to render: {error}", page + 1));
                }
                Resp::Exported(Ok((p, 0))) => self.toast(format!("Exported → {}", p.display())),
                Resp::Exported(Ok((p, missing))) => {
                    self.toast(format!("Exported → {}  ({missing} character(s) had no font and became '?')", p.display()))
                }
                Resp::Exported(Err(e)) => self.toast(format!("Export failed: {e}")),
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // View geometry

    fn page_doc_rect(&self, i: usize) -> Rect {
        let s = self.pages[i];
        Rect::from_min_size(pos2((self.doc_w - s.x) / 2.0, self.page_y[i]), s)
    }

    fn to_screen(&self, doc: Pos2) -> Pos2 {
        self.canvas.min + (doc.to_vec2() - self.offset) * self.zoom
    }

    fn to_doc(&self, screen: Pos2) -> Pos2 {
        (self.offset + (screen - self.canvas.min) / self.zoom).to_pos2()
    }

    fn page_to_screen(&self, page: usize, p: Pos2) -> Pos2 {
        self.to_screen(self.page_doc_rect(page).min + p.to_vec2())
    }

    fn screen_to_page(&self, page: usize, s: Pos2) -> Pos2 {
        (self.to_doc(s) - self.page_doc_rect(page).min).to_pos2()
    }

    fn page_at(&self, screen: Pos2) -> Option<usize> {
        let d = self.to_doc(screen);
        (0..self.pages.len()).find(|&i| self.page_doc_rect(i).contains(d))
    }

    fn visible_pages(&self) -> Vec<usize> {
        let top = self.offset.y;
        let bottom = top + self.canvas.height() / self.zoom;
        (0..self.pages.len()).filter(|&i| self.page_y[i] < bottom && self.page_y[i] + self.pages[i].y > top).collect()
    }

    fn current_page(&self) -> usize {
        let mid = self.offset.y + self.canvas.height() / (2.0 * self.zoom);
        (0..self.pages.len()).rev().find(|&i| self.page_y[i] - GAP <= mid).unwrap_or(0)
    }

    fn zoom_at(&mut self, factor: f32, screen: Pos2) {
        let before = self.to_doc(screen);
        self.zoom = (self.zoom * factor).clamp(0.02, 40.0);
        self.offset = before.to_vec2() - (screen - self.canvas.min) / self.zoom;
    }

    fn fit_width(&mut self) {
        if self.pages.is_empty() {
            return;
        }
        let i = self.current_page();
        let r = self.page_doc_rect(i);
        self.zoom = (self.canvas.width() - 24.0) / r.width();
        self.offset.x = r.min.x - 12.0 / self.zoom;
        self.offset.y = r.min.y - GAP / 2.0;
    }

    fn fit_page(&mut self) {
        if self.pages.is_empty() {
            return;
        }
        let i = self.current_page();
        self.show_page(i);
    }

    fn show_page(&mut self, i: usize) {
        let r = self.page_doc_rect(i);
        self.zoom = ((self.canvas.width() - 24.0) / r.width()).min((self.canvas.height() - 24.0) / r.height());
        let view = self.canvas.size() / self.zoom;
        self.offset = r.center().to_vec2() - view / 2.0;
    }

    fn goto_page(&mut self, i: usize) {
        if i < self.pages.len() {
            self.offset.y = self.page_y[i] - GAP / 2.0;
            let r = self.page_doc_rect(i);
            if r.width() * self.zoom < self.canvas.width() {
                self.offset.x = r.center().x - self.canvas.width() / (2.0 * self.zoom);
            }
        }
    }

    fn clamp_offset(&mut self) {
        let view = self.canvas.size() / self.zoom;
        let clamp = |o: f32, doc: f32, view: f32| {
            if doc <= view { (doc - view) / 2.0 } else { o.clamp(-view * 0.5, doc - view * 0.5) }
        };
        self.offset.x = clamp(self.offset.x, self.doc_w, view.x);
        self.offset.y = self.offset.y.clamp(-view.y * 0.5, (self.doc_h - view.y * 0.5).max(0.0));
    }

    // -----------------------------------------------------------------------
    // Rendering requests

    fn request_renders(&mut self, ctx: &egui::Context, now: f64) {
        let Some(w) = &self.worker else { return };
        if self.pages.is_empty() {
            return;
        }
        let ppp = ctx.pixels_per_point();
        let need = self.zoom * ppp;
        let vis = self.visible_pages();
        let cur = self.current_page();
        let mut want: Vec<usize> = vis.clone();
        for i in [vis.first().copied().unwrap_or(cur).checked_sub(1), vis.last().map(|v| v + 1)].into_iter().flatten() {
            if i < self.pages.len() {
                want.push(i);
            }
        }
        want.sort_by_key(|&i| (i as i64 - cur as i64).abs());
        if self.overview {
            want.clear();
        }
        let mut thumbs_wanted = std::mem::take(&mut self.thumbs_wanted);
        thumbs_wanted.retain(|&i| i < self.pages.len());
        {
            let mut keep = w.keep.lock().unwrap();
            keep.base = want.iter().copied().collect();
            keep.thumbs = thumbs_wanted.iter().copied().collect();
        }
        self.req_thumb.retain(|k| thumbs_wanted.contains(k));
        for &i in &thumbs_wanted {
            if self.thumbs.contains_key(&i) || self.req_thumb.contains(&i) || self.failed.contains(&i) {
                continue;
            }
            let s = self.pages[i];
            let scale = THUMB_PX / s.x.max(s.y);
            let size = [(s.x * scale).round().max(1.0) as usize, (s.y * scale).round().max(1.0) as usize];
            w.send(Req::Render { generation: self.generation, page: i, scale, origin: [0, 0], size, layer: Layer::Thumb });
            self.req_thumb.insert(i);
        }
        if self.thumbs.len() > MAX_THUMBS {
            self.thumbs.retain(|k, _| thumbs_wanted.contains(k));
        }
        let keep_tex: HashSet<usize> = want.iter().flat_map(|&i| i.saturating_sub(2)..=i + 2).collect();
        if !self.overview {
            self.base.retain(|k, _| keep_tex.contains(k));
        }
        self.patch.retain(|k, _| vis.contains(k));
        // The worker drops requests for pages that left `keep`; forget them so they are re-sent.
        self.req_base.retain(|k, _| want.contains(k));
        self.req_patch.retain(|k, _| vis.contains(k));

        for &i in &want {
            let s = self.pages[i];
            let cap = MAX_BASE_PX / s.x.max(s.y);
            let bucket = 2f32.powf(need.log2().ceil()).max(0.125);
            let scale = bucket.min(cap);
            if self.failed.contains(&i) {
                continue;
            }
            let have = self.base.get(&i).map(|t| t.scale);
            if have != Some(scale) && self.req_base.get(&i) != Some(&scale) {
                let size = [(s.x * scale).round().max(1.0) as usize, (s.y * scale).round().max(1.0) as usize];
                w.send(Req::Render { generation: self.generation, page: i, scale, origin: [0, 0], size, layer: Layer::Base });
                self.req_base.insert(i, scale);
            }
        }

        // Sharp patch of the visible region once the view settles.
        if self.overview {
            return;
        }
        if now - self.last_view_change < 0.12 {
            ctx.request_repaint_after(std::time::Duration::from_millis(130));
            return;
        }
        let view = Rect::from_min_size(self.offset.to_pos2(), self.canvas.size() / self.zoom);
        for &i in &vis {
            let base_scale = self.base.get(&i).map(|t| t.scale).unwrap_or(0.0);
            if base_scale >= need * 0.98 || self.failed.contains(&i) {
                self.patch.remove(&i);
                continue;
            }
            let pr = self.page_doc_rect(i);
            let local = view.intersect(pr).translate(-pr.min.to_vec2());
            if local.width() <= 0.0 || local.height() <= 0.0 {
                continue;
            }
            let full = [(pr.width() * need).round() as i32, (pr.height() * need).round() as i32];
            let x0 = ((local.min.x * need).floor() as i32).clamp(0, full[0]);
            let y0 = ((local.min.y * need).floor() as i32).clamp(0, full[1]);
            let x1 = ((local.max.x * need).ceil() as i32).clamp(0, full[0]);
            let y1 = ((local.max.y * need).ceil() as i32).clamp(0, full[1]);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let key = (need, [x0, y0], [(x1 - x0) as usize, (y1 - y0) as usize]);
            let have = self.patch.get(&i).map(|t| (t.scale, t.origin, t.size));
            if have != Some(key) && self.req_patch.get(&i) != Some(&key) {
                w.send(Req::Render { generation: self.generation, page: i, scale: need, origin: key.1, size: key.2, layer: Layer::Patch });
                self.req_patch.insert(i, key);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Annotation editing

    fn snapshot(&mut self) {
        self.undo.push(self.annots.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self) {
        self.dirty = true;
    }

    fn color(&self) -> Rgb {
        COLORS[self.color_idx(self.tool)].0
    }

    fn color_idx(&self, tool: Tool) -> usize {
        *self.tool_color.get(&tool).unwrap_or(&0)
    }

    fn add(&mut self, page: usize, kind: Kind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.annots.push(Annot { id, page, color: self.color(), width: self.width, kind, wrap_w: None });
        self.changed();
        id
    }

    fn find(&self, id: u64) -> Option<&Annot> {
        self.annots.iter().find(|a| a.id == id)
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut Annot> {
        self.annots.iter_mut().find(|a| a.id == id)
    }

    fn start_edit(&mut self, id: u64) {
        if let Some(a) = self.find_mut(id) {
            a.widen_for_edit();
        }
        self.editing = Some(id);
        self.edit_needs_focus = true;
        self.selected = Some(id);
    }

    fn commit_edit(&mut self) {
        if let Some(id) = self.editing.take() {
            if self.find(id).is_some_and(|a| text_of(a).trim().is_empty()) {
                self.annots.retain(|a| a.id != id);
                self.selected = None;
            } else if let Some(a) = self.find_mut(id) {
                a.fit_box_w();
            }
            self.changed();
        }
    }

    fn hit(&self, page: usize, p: Pos2) -> Option<u64> {
        let tol = HIT_PX / self.zoom;
        // Prefer the selected one, then topmost.
        if let Some(sel) = self.selected.and_then(|id| self.find(id)) {
            if sel.page == page && sel.distance(p) <= tol {
                return Some(sel.id);
            }
        }
        self.annots.iter().rev().filter(|a| a.page == page).find(|a| a.distance(p) <= tol).map(|a| a.id)
    }

    fn handles(a: &Annot) -> Vec<(Handle, Pos2)> {
        match &a.kind {
            Kind::Cloud { rect } | Kind::Rect { rect } | Kind::Highlight { rect } => {
                let c = annot::rect_pts(*rect);
                (0..4).map(|i| (Handle::Corner(c[(i + 2) % 4]), c[i])).collect()
            },
            Kind::Callout { anchor, .. } => {
                let r = a.text_box().unwrap();
                vec![(Handle::Anchor, *anchor), (Handle::BoxWidth, pos2(r.max.x, r.center().y))]
            }
            Kind::Text { .. } => {
                let r = a.text_box().unwrap();
                vec![(Handle::BoxWidth, pos2(r.max.x, r.center().y))]
            }
            Kind::Arrow { a: p, b } | Kind::Line { a: p, b } => vec![(Handle::A, *p), (Handle::B, *b)],
            Kind::Pen { .. } => vec![],
        }
    }

    fn hit_handle(&self, page: usize, p: Pos2) -> Option<(u64, Handle)> {
        let a = self.selected.and_then(|id| self.find(id))?;
        if a.page != page {
            return None;
        }
        let tol = HIT_PX / self.zoom;
        Self::handles(a).into_iter().find(|(_, hp)| (*hp - p).length() <= tol).map(|(h, _)| (a.id, h))
    }

    fn apply_handle(a: &mut Annot, h: Handle, p: Pos2) {
        match (&mut a.kind, h) {
            (Kind::Cloud { rect } | Kind::Rect { rect } | Kind::Highlight { rect }, Handle::Corner(opposite)) => {
                *rect = Rect::from_two_pos(opposite, p);
            }
            (Kind::Callout { anchor, .. }, Handle::Anchor) => *anchor = p,
            (Kind::Callout { pos, box_w, size, .. } | Kind::Text { pos, box_w, size, .. }, Handle::BoxWidth) => {
                *box_w = (p.x - pos.x).max(4.0 * *size);
                a.wrap_w = Some(*box_w);
            }
            (Kind::Arrow { a, .. } | Kind::Line { a, .. }, Handle::A) => *a = p,
            (Kind::Arrow { b, .. } | Kind::Line { b, .. }, Handle::B) => *b = p,
            _ => {}
        }
    }

    fn delete_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            self.snapshot();
            self.annots.retain(|a| a.id != id);
            self.changed();
        }
    }

    fn undo(&mut self) {
        self.commit_edit();
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.annots, prev));
            self.selected = None;
            self.changed();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.annots, next));
            self.selected = None;
            self.changed();
        }
    }

    fn set_tool(&mut self, t: Tool) {
        self.commit_edit();
        self.tool = t;
        if t != Tool::Select {
            self.selected = None;
        }
    }

    fn set_color(&mut self, idx: usize) {
        self.tool_color.insert(self.tool, idx);
        if let Some(id) = self.selected {
            self.snapshot();
            if let Some(a) = self.find_mut(id) {
                a.color = COLORS[idx].0;
            }
            self.changed();
        }
    }

    fn set_width(&mut self, w: f32) {
        self.width = w.clamp(0.5, 12.0);
        if let Some(id) = self.selected {
            self.snapshot();
            let w = self.width;
            if let Some(a) = self.find_mut(id) {
                a.width = w;
            }
            self.changed();
        }
    }

    fn step_text_size(&mut self, dir: i32) {
        let cur = self.selected.and_then(|id| self.find(id)).and_then(|a| a.text_size()).unwrap_or(self.text_size);
        let i = TEXT_SIZES.iter().position(|&s| s >= cur).unwrap_or(TEXT_SIZES.len() - 1) as i32;
        let next = TEXT_SIZES[(i + dir).clamp(0, TEXT_SIZES.len() as i32 - 1) as usize];
        self.set_text_size(next);
    }

    fn set_text_size(&mut self, s: f32) {
        self.text_size = s;
        if let Some(id) = self.selected {
            if self.find(id).and_then(|a| a.text_size()).is_some() {
                self.snapshot();
                if let Some(a) = self.find_mut(id) {
                    if let Kind::Callout { size, .. } | Kind::Text { size, .. } = &mut a.kind {
                        *size = s;
                    }
                }
                self.changed();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Input

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.goto.is_some() {
            self.goto_keys(ctx);
            return;
        }
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        if cmd {
            if pressed(Key::O) {
                if let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                    self.open(p);
                }
            }
            if pressed(Key::E) {
                let alt = ctx.input(|i| i.modifiers.alt);
                self.export(if alt { ExportMode::Editable } else { ExportMode::Flatten }, shift);
            }
            if pressed(Key::Z) {
                if shift { self.redo() } else { self.undo() }
            }
            if pressed(Key::Y) {
                self.redo();
            }
            if pressed(Key::D) {
                if let Some(a) = self.selected.and_then(|id| self.find(id)).cloned() {
                    self.snapshot();
                    let id = self.next_id;
                    self.next_id += 1;
                    let mut b = Annot { id, ..a };
                    b.translate(vec2(12.0, 12.0) / self.zoom.max(0.1));
                    self.annots.push(b);
                    self.selected = Some(id);
                    self.changed();
                }
            }
            if pressed(Key::Num0) {
                self.fit_width();
            }
            return;
        }
        if pressed(Key::O) && !self.pages.is_empty() {
            self.toggle_overview();
            return;
        }
        if self.overview {
            self.overview_keys(ctx);
            return;
        }
        if pressed(Key::S) {
            self.show_sidebar = !self.show_sidebar;
        }
        if pressed(Key::M) {
            self.show_markups = !self.show_markups;
        }
        for (t, _, k) in TOOLS {
            if pressed(k) {
                self.set_tool(t);
            }
        }
        if pressed(Key::Escape) {
            // Cancel any gesture in progress (the rest of it is swallowed until release).
            if let Drag::Move { before, .. } | Drag::Handle { before, .. } = std::mem::replace(&mut self.drag, Drag::Ignore) {
                self.annots = before;
            }
            if self.selected.is_some() { self.selected = None } else { self.set_tool(Tool::Select) }
        }
        for (i, k) in COLOR_KEYS.iter().enumerate() {
            if pressed(*k) {
                self.set_color(i);
            }
        }
        if pressed(Key::OpenBracket) {
            if shift { self.step_text_size(-1) } else { self.set_width(self.width - 0.5) }
        }
        if pressed(Key::CloseBracket) {
            if shift { self.step_text_size(1) } else { self.set_width(self.width + 0.5) }
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selected();
        }
        if pressed(Key::Enter) {
            if let Some(id) = self.selected.filter(|id| self.find(*id).is_some_and(|a| a.text_size().is_some())) {
                self.snapshot();
                self.start_edit(id);
            }
        }
        if pressed(Key::W) {
            self.fit_width();
        }
        if pressed(Key::F) {
            self.fit_page();
        }
        if pressed(Key::G) {
            self.goto = Some(String::new());
        }
        if pressed(Key::F1) || pressed(Key::Questionmark) {
            self.show_help = !self.show_help;
        }
        let center = self.canvas.center();
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.zoom_at(1.25, center);
        }
        if pressed(Key::Minus) {
            self.zoom_at(0.8, center);
        }
        let n = self.pages.len();
        if n > 0 {
            let cur = self.current_page();
            if pressed(Key::PageDown) || pressed(Key::ArrowRight) {
                self.goto_page((cur + 1).min(n - 1));
            }
            if pressed(Key::PageUp) || pressed(Key::ArrowLeft) {
                self.goto_page(cur.saturating_sub(1));
            }
            if pressed(Key::Home) {
                self.goto_page(0);
            }
            if pressed(Key::End) {
                self.goto_page(n - 1);
            }
        }
        let step = 60.0 / self.zoom;
        if pressed(Key::ArrowDown) {
            self.offset.y += step;
        }
        if pressed(Key::ArrowUp) {
            self.offset.y -= step;
        }
    }

    fn canvas_input(&mut self, ui: &Ui, resp: &egui::Response) {
        let ctx = ui.ctx().clone();
        let (hover, scroll, zoom_delta, space, press_origin, latest) = ctx.input(|i| {
            (i.pointer.hover_pos(), i.smooth_scroll_delta, i.zoom_delta(), i.key_down(Key::Space), i.pointer.press_origin(), i.pointer.latest_pos())
        });

        if resp.hovered() {
            if zoom_delta != 1.0 {
                if let Some(h) = hover {
                    self.zoom_at(zoom_delta, h);
                }
            } else if scroll != Vec2::ZERO {
                let now = ui.input(|i| i.time);
                let dir = if scroll.y.abs() >= scroll.x.abs() { scroll.y.signum() * 2.0 } else { scroll.x.signum() };
                let (start, last, last_dir) = self.scroll_streak;
                let start = if now - last > SCROLL_STREAK_GAP || dir != last_dir { now } else { start };
                self.scroll_streak = (start, now, dir);
                let boost = (1.0 + (now - start - SCROLL_ACCEL_DELAY).max(0.0) * SCROLL_ACCEL_RATE).min(SCROLL_ACCEL_MAX) as f32;
                self.offset -= scroll * SCROLL_SPEED * boost / self.zoom;
            }
        }

        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
            self.offset -= resp.drag_delta() / self.zoom;
            return;
        }

        if self.pages.is_empty() {
            return;
        }

        // Clicking the drawing takes keyboard focus back from toolbar widgets
        // (a focused DragValue/ComboBox would otherwise swallow the shortcuts).
        if resp.is_pointer_button_down_on() && self.editing.is_none() {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }

        // Primary button.
        if resp.drag_started_by(PointerButton::Primary) {
            let start = press_origin.unwrap_or_default();
            self.begin_drag(start, space);
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = latest {
                self.update_drag(p, resp.drag_delta());
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            if let Some(p) = latest {
                self.update_drag(p, resp.drag_delta());
            }
            self.end_drag();
        }
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.click(p, resp.double_clicked());
            }
        }

        // Cursor.
        if space || matches!(self.drag, Drag::Pan) {
            ctx.set_cursor_icon(if matches!(self.drag, Drag::Pan) { CursorIcon::Grabbing } else { CursorIcon::Grab });
        } else if resp.hovered() {
            let icon = match self.tool {
                Tool::Select => {
                    let over = hover.and_then(|h| self.page_at(h).map(|pg| (pg, self.screen_to_page(pg, h))));
                    match over {
                        Some((pg, p)) if self.hit_handle(pg, p).is_some() => CursorIcon::Crosshair,
                        Some((pg, p)) if self.hit(pg, p).is_some() => CursorIcon::Move,
                        _ => CursorIcon::Default,
                    }
                }
                Tool::Text => CursorIcon::Text,
                _ => CursorIcon::Crosshair,
            };
            ctx.set_cursor_icon(icon);
        }
    }

    fn begin_drag(&mut self, start: Pos2, space: bool) {
        if self.editing.is_some() {
            self.commit_edit();
            self.drag = Drag::Ignore;
            return;
        }
        let Some(page) = self.page_at(start) else {
            self.drag = Drag::Pan;
            return;
        };
        if space {
            self.drag = Drag::Pan;
            return;
        }
        let p = self.screen_to_page(page, start);
        self.drag = match self.tool {
            Tool::Select => {
                if let Some((id, handle)) = self.hit_handle(page, p) {
                    Drag::Handle { id, handle, before: self.annots.clone() }
                } else if let Some(id) = self.hit(page, p) {
                    self.selected = Some(id);
                    Drag::Move { id, last: p, before: self.annots.clone(), moved: false }
                } else {
                    self.selected = None;
                    Drag::Pan
                }
            }
            Tool::Pen => Drag::Pen { page, pts: vec![p] },
            _ => Drag::Create { page, start: p, cur: p },
        };
    }

    fn update_drag(&mut self, screen: Pos2, delta: Vec2) {
        let page = match &self.drag {
            Drag::Create { page, .. } | Drag::Pen { page, .. } => Some(*page),
            Drag::Move { id, .. } | Drag::Handle { id, .. } => self.find(*id).map(|a| a.page),
            _ => None,
        };
        let p = page.map(|pg| self.screen_to_page(pg, screen)).unwrap_or_default();
        let min_step = 1.0 / self.zoom;
        match &mut self.drag {
            Drag::Pan => self.offset -= delta / self.zoom,
            Drag::Create { cur, .. } => *cur = p,
            Drag::Pen { pts, .. } => {
                if pts.last().is_none_or(|l| (*l - p).length() >= min_step) {
                    pts.push(p);
                }
            }
            Drag::Move { id, last, moved, .. } => {
                let (id, d) = (*id, p - *last);
                *last = p;
                *moved = true;
                if let Some(a) = self.annots.iter_mut().find(|a| a.id == id) {
                    a.translate(d);
                }
            }
            Drag::Handle { id, handle, .. } => {
                let (id, h) = (*id, *handle);
                if let Some(a) = self.annots.iter_mut().find(|a| a.id == id) {
                    Self::apply_handle(a, h, p);
                }
            }
            Drag::None | Drag::Ignore => {}
        }
    }

    fn end_drag(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Create { page, start, cur } => self.create(page, start, cur, true),
            Drag::Pen { page, pts } => {
                if pts.len() >= 2 {
                    self.snapshot();
                    let pts = annot::simplify(&pts, 0.6 / self.zoom);
                    self.add(page, Kind::Pen { pts });
                }
            }
            Drag::Move { before, moved, .. } => {
                if moved {
                    self.undo.push(before);
                    self.redo.clear();
                    self.changed();
                }
            }
            Drag::Handle { before, .. } => {
                self.undo.push(before);
                self.redo.clear();
                self.changed();
            }
            _ => {}
        }
    }

    fn click(&mut self, screen: Pos2, double: bool) {
        if self.editing.is_some() {
            self.commit_edit();
            return;
        }
        let Some(page) = self.page_at(screen) else {
            self.selected = None;
            return;
        };
        let p = self.screen_to_page(page, screen);
        match self.tool {
            Tool::Select => {
                self.selected = self.hit(page, p);
                if double {
                    if let Some(id) = self.selected.filter(|id| self.find(*id).is_some_and(|a| a.text_size().is_some())) {
                        self.snapshot();
                        self.start_edit(id);
                    }
                }
            }
            Tool::Callout | Tool::Text => self.create(page, p, p, false),
            _ => {
                // Clicking an existing markup with a drawing tool selects it for quick edits.
                if let Some(id) = self.hit(page, p) {
                    self.tool = Tool::Select;
                    self.selected = Some(id);
                }
            }
        }
    }

    fn create(&mut self, page: usize, start: Pos2, end: Pos2, dragged: bool) {
        let bounds = Rect::from_min_size(Pos2::ZERO, self.pages[page]);
        let end = bounds.clamp(end);
        let rect = Rect::from_two_pos(start, end);
        let size = self.text_size;
        let tiny = rect.width() < 3.0 / self.zoom.min(1.0) && rect.height() < 3.0 / self.zoom.min(1.0);
        let kind = match self.tool {
            Tool::Callout => {
                let box_w = annot::default_box_w(size);
                let h = annot::box_rect(Pos2::ZERO, box_w, "", size).height();
                let target = if dragged && (end - start).length() * self.zoom > 12.0 {
                    end
                } else {
                    // Quick note: up and to the right, flipped inward near page edges.
                    let off = 50.0 / self.zoom.max(0.05);
                    let dx = if start.x + off + box_w <= bounds.max.x { off } else { -off };
                    let dy = if start.y - off - h / 2.0 >= 0.0 { -off } else { off };
                    start + vec2(dx, dy)
                };
                let x = if target.x >= start.x { target.x } else { target.x - box_w };
                let pos = pos2(
                    x.clamp(0.0, (bounds.max.x - box_w).max(0.0)),
                    (target.y - h / 2.0).clamp(0.0, (bounds.max.y - h).max(0.0)),
                );
                Kind::Callout { anchor: start, pos, box_w, text: String::new(), size }
            }
            Tool::Text => {
                let box_w = if dragged && rect.width() > 4.0 * size { rect.width() } else { annot::default_box_w(size) };
                let x = rect.min.x.clamp(0.0, (bounds.max.x - box_w).max(0.0));
                Kind::Text { pos: pos2(x, rect.min.y), box_w, text: String::new(), size }
            }
            _ if tiny => return,
            Tool::Cloud => Kind::Cloud { rect },
            Tool::Rect => Kind::Rect { rect },
            Tool::Highlight => Kind::Highlight { rect },
            Tool::Arrow => Kind::Arrow { a: start, b: end },
            Tool::Line => Kind::Line { a: start, b: end },
            Tool::Select | Tool::Pen => return,
        };
        self.snapshot();
        let is_text = matches!(kind, Kind::Callout { .. } | Kind::Text { .. });
        let explicit_w = match &kind {
            Kind::Text { box_w, .. } if dragged && rect.width() > 4.0 * size => Some(*box_w),
            _ => None,
        };
        let id = self.add(page, kind);
        if let Some(a) = self.find_mut(id) {
            a.wrap_w = explicit_w;
        }
        if is_text {
            self.start_edit(id);
        }
    }

    // -----------------------------------------------------------------------
    // Painting

    fn text_metrics(&mut self, painter: &Painter) -> (f32, f32) {
        if let Some(k) = self.text_k {
            return k;
        }
        let sample = "Hamburgefonstiv Quick review 0123";
        let g = painter.layout_no_wrap(sample.into(), FontId::new(100.0, helv()), Color32::BLACK);
        let k = annot::text_width(sample, 100.0) / g.size().x.max(1.0);
        let g = painter.layout_no_wrap("H".into(), FontId::new(100.0 * k, helv()), Color32::BLACK);
        let baseline = g.rows.first().and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y)).unwrap_or(80.0);
        self.text_k = Some((k, baseline / 100.0));
        self.text_k.unwrap()
    }

    fn paint(&mut self, ui: &mut Ui) {
        let painter = ui.painter_at(self.canvas);
        painter.rect_filled(self.canvas, 0.0, Color32::from_gray(64));
        let k = self.text_metrics(&painter);
        let shadow = Color32::from_black_alpha(90);
        for i in self.visible_pages() {
            let pr = self.page_doc_rect(i);
            let sr = Rect::from_min_max(self.to_screen(pr.min), self.to_screen(pr.max));
            painter.rect_filled(sr.translate(vec2(2.0, 3.0)), 0.0, shadow);
            painter.rect_filled(sr, 0.0, Color32::WHITE);
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            let base_scale = self.base.get(&i).map(|t| t.scale).unwrap_or(0.0);
            if let Some(t) = self.base.get(&i) {
                painter.image(t.tex.id(), sr, uv, Color32::WHITE);
            }
            if let Some(t) = self.patch.get(&i).filter(|t| t.scale > base_scale) {
                let min = pr.min + vec2(t.origin[0] as f32, t.origin[1] as f32) / t.scale;
                let max = min + vec2(t.size[0] as f32, t.size[1] as f32) / t.scale;
                painter.image(t.tex.id(), Rect::from_min_max(self.to_screen(min), self.to_screen(max)), uv, Color32::WHITE);
            }

            let pp = painter.with_clip_rect(sr.intersect(self.canvas));
            for a in self.annots.iter().filter(|a| a.page == i) {
                let mut prims = a.prims();
                if Some(a.id) == self.editing {
                    prims.retain(|p| !matches!(p, Prim::Text { .. }));
                    if let Kind::Text { .. } = a.kind {
                        let r = a.text_box().unwrap();
                        let r = Rect::from_min_max(self.page_to_screen(i, r.min), self.page_to_screen(i, r.max));
                        pp.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(150)), egui::StrokeKind::Outside);
                    }
                }
                paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &prims, k, 1.0);
            }
            // Live preview.
            match &self.drag {
                Drag::Create { page, start, cur } if *page == i => {
                    let preview = self.preview_kind(*start, *cur);
                    if let Some(kind) = preview {
                        let a = Annot { id: 0, page: i, color: self.color(), width: self.width, kind, wrap_w: None };
                        paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.7);
                    }
                }
                Drag::Pen { page, pts } if *page == i => {
                    let a = Annot { id: 0, page: i, color: self.color(), width: self.width, kind: Kind::Pen { pts: pts.clone() }, wrap_w: None };
                    paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 1.0);
                }
                _ => {}
            }
            // Selection.
            if let Some(a) = self.selected.and_then(|id| self.find(id)).filter(|a| a.page == i) {
                let b = a.bounds();
                let r = Rect::from_min_max(self.page_to_screen(i, b.min), self.page_to_screen(i, b.max)).expand(3.0);
                let sel = Color32::from_rgb(0, 150, 255);
                pp.rect_stroke(r, 0.0, Stroke::new(1.0, sel), egui::StrokeKind::Outside);
                for (_, hp) in Self::handles(a) {
                    let c = self.page_to_screen(i, hp);
                    painter.rect(Rect::from_center_size(c, Vec2::splat(8.0)), 1.0, Color32::WHITE, Stroke::new(1.5, sel), egui::StrokeKind::Middle);
                }
            }
        }
    }

    fn preview_kind(&self, start: Pos2, cur: Pos2) -> Option<Kind> {
        let rect = Rect::from_two_pos(start, cur);
        let size = self.text_size;
        Some(match self.tool {
            Tool::Callout => {
                let box_w = annot::default_box_w(size);
                let h = annot::box_rect(Pos2::ZERO, box_w, "", size).height();
                let x = if cur.x >= start.x { cur.x } else { cur.x - box_w };
                Kind::Callout { anchor: start, pos: pos2(x, cur.y - h / 2.0), box_w, text: String::new(), size }
            }
            Tool::Text => Kind::Rect { rect },
            Tool::Cloud => Kind::Cloud { rect },
            Tool::Rect => Kind::Rect { rect },
            Tool::Highlight => Kind::Highlight { rect },
            Tool::Arrow => Kind::Arrow { a: start, b: cur },
            Tool::Line => Kind::Line { a: start, b: cur },
            Tool::Select | Tool::Pen => return None,
        })
    }

    fn text_editor(&mut self, ctx: &egui::Context) {
        let Some(id) = self.editing else { return };
        let Some(a) = self.find(id) else {
            self.editing = None;
            return;
        };
        let (page, size, color) = (a.page, a.text_size().unwrap(), a.color);
        let r = a.text_box().unwrap();
        let k = self.text_k.unwrap_or((1.0, 0.8));
        let z = self.zoom;
        let pad = annot::pad(size);
        let pos = self.page_to_screen(page, r.min + vec2(pad, pad));
        let width = (r.width() - 2.0 * pad) * z;
        let font = FontId::new((size * z * k.0).max(6.0), helv());
        let mut text = text_of(a).to_string();
        let needs_focus = std::mem::take(&mut self.edit_needs_focus);
        
        let resp = egui::Area::new(egui::Id::new("redline-edit"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                // Areas default to a narrow max width; wrap exactly at the box's text width.
                let w = width.max(20.0);
                ui.set_min_width(w);
                ui.set_max_width(w);
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .id(egui::Id::new(("redline-edit", self.generation, id)))
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .desired_width(width.max(20.0))
                        .desired_rows(1)
                        .font(font)
                        .text_color(Color32::from_rgb(color[0], color[1], color[2])),
                )
            })
            .inner;
        if needs_focus {
            resp.request_focus();
        }
        if let Some(a) = self.find_mut(id) {
            if let Some(t) = a.text_mut() {
                if *t != text {
                    *t = text;
                    self.dirty = true;
                }
            }
        }
        let done = ctx.input(|i| i.key_pressed(Key::Escape) || (i.modifiers.command && i.key_pressed(Key::Enter)));
        if done || (!needs_focus && resp.lost_focus()) {
            self.commit_edit();
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open").on_hover_text("Ctrl+O").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                    self.open(p);
                }
            }
            ui.add_enabled_ui(self.path.is_some(), |ui| {
                ui.menu_button("Export", |ui| {
                    if ui.button("Flattened PDF   (Ctrl+E)").on_hover_text("Markups burned in → <name>_markup.pdf").clicked() {
                        self.export(ExportMode::Flatten, false);
                    }
                    if ui.button("Flattened PDF as…   (Ctrl+Shift+E)").clicked() {
                        self.export(ExportMode::Flatten, true);
                    }
                    if ui.button("Editable PDF   (Ctrl+Alt+E)").on_hover_text("Each markup a movable comment in Bluebeam/Acrobat → <name>_markup_editable.pdf").clicked() {
                        self.export(ExportMode::Editable, false);
                    }
                    if ui.button("Comment list (CSV)").on_hover_text("<name>_comments.csv: page, type, comment").clicked() {
                        self.export_csv();
                    }
                });
            });
            ui.separator();
            if ui.selectable_label(self.show_sidebar, "Pages S").on_hover_text("Thumbnail sidebar").clicked() {
                self.show_sidebar = !self.show_sidebar;
            }
            if ui.selectable_label(self.overview, "Overview O").on_hover_text("All pages at a glance").clicked() && !self.pages.is_empty() {
                self.toggle_overview();
            }
            if ui.selectable_label(self.show_markups, "Markups M").on_hover_text("List of markups").clicked() {
                self.show_markups = !self.show_markups;
            }
            ui.separator();
            for (t, label, k) in TOOLS {
                let text = format!("{label} {}", k.name());
                if ui.selectable_label(self.tool == t, text).clicked() {
                    self.set_tool(t);
                }
            }
            ui.separator();
            let cur = self.selected.and_then(|id| self.find(id)).map(|a| a.color).unwrap_or(self.color());
            for (i, (c, name)) in COLORS.iter().enumerate() {
                let col = Color32::from_rgb(c[0], c[1], c[2]);
                let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                ui.painter().rect_filled(rect.shrink(2.0), 3.0, col);
                if *c == cur {
                    ui.painter().rect_stroke(rect, 3.0, Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
                }
                if resp.on_hover_text(format!("{name} ({})", i + 1)).clicked() {
                    self.set_color(i);
                }
            }
            ui.separator();
            ui.label("Line");
            let mut w = self.width;
            if ui.add(egui::DragValue::new(&mut w).range(0.5..=12.0).speed(0.1).suffix(" pt")).on_hover_text("[ / ]").changed() {
                self.set_width(w);
            }
            ui.label("Text");
            let cur_size = self.selected.and_then(|id| self.find(id)).and_then(|a| a.text_size()).unwrap_or(self.text_size);
            egui::ComboBox::from_id_salt("tsize").width(56.0).selected_text(format!("{cur_size} pt")).show_ui(ui, |ui| {
                for s in TEXT_SIZES {
                    if ui.selectable_label(s == cur_size, format!("{s} pt")).clicked() {
                        self.set_text_size(s);
                    }
                }
            });
            ui.separator();
            if ui.button("?").on_hover_text("Shortcuts (F1)").clicked() {
                self.show_help = !self.show_help;
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, now: f64) {
        if self.status_until.is_nan() {
            self.status_until = now + 5.0;
        }
        ui.horizontal(|ui| {
            if !self.pages.is_empty() {
                ui.label(format!("Page {} / {}", self.current_page() + 1, self.pages.len()));
                ui.separator();
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                ui.separator();
                ui.label(format!("{} markups", self.annots.len()));
                ui.separator();
            }
            if self.read_only && self.path.is_some() {
                ui.colored_label(Color32::from_rgb(230, 80, 60), format!("NOT AUTOSAVING: {}", self.read_only_why));
                ui.separator();
            }
            if now < self.status_until {
                ui.label(&self.status);
            } else if let Some(p) = &self.path {
                ui.weak(p.display().to_string());
            }
        });
    }

    /// Page-number entry after `G`: digits, Backspace, Enter to jump, Esc to cancel.
    /// Reads key events directly (no text box), so the `G` itself is never typed in.
    fn goto_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let Some(s) = self.goto.as_mut() else { return };
        let mut jump = None;
        let mut close = false;
        for e in events {
            match e {
                egui::Event::Text(t) => s.extend(t.chars().filter(|c| c.is_ascii_digit())),
                egui::Event::Key { key, pressed: true, .. } => match key {
                    Key::Backspace => {
                        s.pop();
                    }
                    Key::Enter => {
                        jump = s.parse::<usize>().ok();
                        close = true;
                    }
                    Key::Escape => close = true,
                    _ => {}
                },
                _ => {}
            }
        }
        if s.len() > 5 {
            s.truncate(5);
        }
        if let Some(n) = jump {
            self.goto_page(n.clamp(1, self.pages.len().max(1)) - 1);
        }
        if close {
            self.goto = None;
        }
    }

    fn goto_overlay(&self, ctx: &egui::Context) {
        let Some(s) = &self.goto else { return };
        egui::Area::new(egui::Id::new("redline-goto"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 70.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Go to page").strong());
                        let shown = if s.is_empty() { "_".to_string() } else { format!("{s}_") };
                        ui.label(egui::RichText::new(shown).monospace().size(18.0));
                        ui.weak(format!("of {}   ·   Enter / Esc", self.pages.len()));
                    });
                });
            });
    }

    // -----------------------------------------------------------------------
    // Thumbnails, overview, markup list

    fn markup_count(&self, page: usize) -> usize {
        self.annots.iter().filter(|a| a.page == page).count()
    }

    /// Height/width of the tallest page, so thumbnail slots are uniform.
    fn max_aspect(&self) -> f32 {
        self.pages.iter().map(|s| s.y / s.x.max(1.0)).fold(0.2, f32::max).min(1.6)
    }

    /// Draws page `i` fitted inside `slot`, with its markups and a count badge.
    fn paint_thumb(&mut self, ui: &Ui, slot: Rect, i: usize, current: bool) {
        self.thumbs_wanted.push(i);
        let s = self.pages[i];
        let z = (slot.width() / s.x).min(slot.height() / s.y);
        let r = Rect::from_center_size(slot.center(), s * z);
        let painter = ui.painter_at(slot.expand(4.0));
        painter.rect_filled(r.translate(vec2(1.5, 2.0)), 0.0, Color32::from_black_alpha(80));
        painter.rect_filled(r, 0.0, Color32::WHITE);
        if let Some(t) = self.thumbs.get(&i) {
            painter.image(t.tex.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }
        let k = self.text_k.unwrap_or((1.0, 0.8));
        let clip = painter.with_clip_rect(r);
        for a in self.annots.iter().filter(|a| a.page == i) {
            paint_prims(&clip, r.min, z, &a.prims(), k, 1.0);
        }
        if current {
            painter.rect_stroke(r.expand(2.0), 2.0, Stroke::new(2.5, ui.visuals().selection.stroke.color), egui::StrokeKind::Outside);
        }
        let n = self.markup_count(i);
        if n > 0 {
            let c = pos2(r.max.x - 9.0, r.min.y + 9.0);
            painter.circle_filled(c, 9.0, Color32::from_rgb(220, 30, 30));
            painter.text(c, Align2::CENTER_CENTER, n.to_string(), FontId::proportional(11.0), Color32::WHITE);
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let w = (ui.available_width() - 8.0).max(40.0);
        let slot_h = w * self.max_aspect();
        let row_h = slot_h + 24.0;
        let cur = self.current_page();
        let n = self.pages.len();
        // Follow the main view: scroll the strip when the current page changes.
        let follow = self.sidebar_synced != Some(cur);
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if follow {
            let top = (cur as f32 * (row_h + ui.spacing().item_spacing.y) - ui.available_height() / 2.0 + row_h / 2.0).max(0.0);
            area = area.vertical_scroll_offset(top);
            self.sidebar_synced = Some(cur);
        }
        area.show_rows(ui, row_h, n, |ui, range| {
            for i in range {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
                let slot = Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(w, slot_h));
                self.paint_thumb(ui, slot, i, i == cur);
                ui.painter().text(pos2(rect.center().x, rect.max.y - 9.0), Align2::CENTER_CENTER, (i + 1).to_string(), FontId::proportional(12.0), ui.visuals().text_color());
                if resp.clicked() {
                    self.goto_page(i);
                    self.sidebar_synced = Some(i);
                }
            }
        });
    }

    fn toggle_overview(&mut self) {
        self.commit_edit();
        // Its release will land in the other view: keep a move/resize already
        // made (recorded for undo and autosave), drop a half-drawn new markup.
        if matches!(self.drag, Drag::Move { .. } | Drag::Handle { .. }) {
            self.end_drag();
        }
        if !matches!(self.drag, Drag::None) {
            self.drag = Drag::Ignore;
        }
        self.overview = !self.overview;
        if self.overview {
            self.overview_cursor = self.current_page();
            self.overview_scroll = true;
        }
    }

    fn overview_keys(&mut self, ctx: &egui::Context) {
        let n = self.pages.len();
        if n == 0 {
            return;
        }
        let cols = self.overview_cols.max(1);
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let mut c = self.overview_cursor as i64;
        if pressed(Key::ArrowRight) {
            c += 1;
        }
        if pressed(Key::ArrowLeft) {
            c -= 1;
        }
        if pressed(Key::ArrowDown) || pressed(Key::PageDown) {
            c += cols as i64;
        }
        if pressed(Key::ArrowUp) || pressed(Key::PageUp) {
            c -= cols as i64;
        }
        if pressed(Key::Home) {
            c = 0;
        }
        if pressed(Key::End) {
            c = n as i64 - 1;
        }
        let c = c.clamp(0, n as i64 - 1) as usize;
        if c != self.overview_cursor {
            self.overview_cursor = c;
            self.overview_scroll = true;
        }
        if pressed(Key::Enter) || pressed(Key::Space) {
            self.overview = false;
            self.goto_page(c);
        } else if pressed(Key::Escape) {
            self.overview = false;
        }
    }

    fn overview_grid(&mut self, ui: &mut Ui) {
        if let Some(h) = ui.input(|i| i.pointer.hover_pos()) {
            let z = ui.input(|i| i.zoom_delta());
            if z != 1.0 && ui.max_rect().contains(h) {
                self.overview_tile = (self.overview_tile * z).clamp(100.0, 900.0);
            }
        }
        let gap = 18.0;
        let tile = self.overview_tile;
        let slot_h = tile * self.max_aspect();
        let row_h = slot_h + 26.0;
        let cols = (((ui.available_width() + gap) / (tile + gap)).floor() as usize).max(1);
        self.overview_cols = cols;
        let n = self.pages.len();
        let rows = n.div_ceil(cols);
        let cur = self.overview_cursor;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if std::mem::take(&mut self.overview_scroll) {
            let row = cur / cols;
            let top = (row as f32 * (row_h + ui.spacing().item_spacing.y) - ui.available_height() / 2.0 + row_h / 2.0).max(0.0);
            area = area.vertical_scroll_offset(top);
        }
        let mut open = None;
        area.show_rows(ui, row_h, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for i in row * cols..((row + 1) * cols).min(n) {
                        let (rect, resp) = ui.allocate_exact_size(vec2(tile, row_h), Sense::click());
                        let slot = Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(tile, slot_h));
                        self.paint_thumb(ui, slot, i, i == cur);
                        let label = match self.markup_count(i) {
                            0 => format!("{}", i + 1),
                            m => format!("{}  ·  {m} markup{}", i + 1, if m == 1 { "" } else { "s" }),
                        };
                        ui.painter().text(pos2(rect.center().x, rect.max.y - 10.0), Align2::CENTER_CENTER, label, FontId::proportional(13.0), ui.visuals().text_color());
                        if resp.hovered() {
                            ui.painter().rect_stroke(slot.expand(3.0), 3.0, Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Outside);
                        }
                        if resp.clicked() {
                            open = Some(i);
                        }
                    }
                });
            }
        });
        if let Some(i) = open {
            self.overview = false;
            self.goto_page(i);
        }
    }

    /// Selects a markup and scrolls it into the middle of the view.
    fn focus_annot(&mut self, id: u64) {
        let Some(a) = self.find(id) else { return };
        if a.page >= self.pages.len() {
            self.toast(format!("That markup is on page {}, which this PDF doesn't have", a.page + 1));
            return;
        }
        let c = self.page_doc_rect(a.page).min + a.bounds().center().to_vec2();
        self.commit_edit();
        self.overview = false;
        self.set_tool(Tool::Select);
        self.selected = Some(id);
        self.offset = c.to_vec2() - self.canvas.size() / (2.0 * self.zoom);
    }

    fn markup_list(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.strong(format!("Markups ({})", self.annots.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(self.path.is_some(), egui::Button::new("CSV")).on_hover_text("Save comment list → <name>_comments.csv").clicked() {
                    self.export_csv();
                }
            });
        });
        ui.separator();
        let mut order: Vec<&Annot> = self.annots.iter().collect();
        order.sort_by(|a, b| (a.page, a.bounds().min.y as i64, a.bounds().min.x as i64).cmp(&(b.page, b.bounds().min.y as i64, b.bounds().min.x as i64)));
        let items: Vec<(u64, usize, Rgb, &'static str, String)> = order.iter().map(|a| (a.id, a.page, a.color, a.kind_name(), a.text().to_string())).collect();
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last_page = None;
            for (id, page, color, kind, text) in &items {
                if last_page != Some(*page) {
                    last_page = Some(*page);
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(format!("Page {}", page + 1)).strong());
                }
                let first = text.lines().next().unwrap_or("").trim();
                let label = if first.is_empty() { kind.to_string() } else { first.to_string() };
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, Color32::from_rgb(color[0], color[1], color[2]));
                    let resp = ui.selectable_label(self.selected == Some(*id), { let t = egui::RichText::new(label); if first.is_empty() { t.weak() } else { t } });
                    let resp = if text.trim().is_empty() { resp } else { resp.on_hover_text(text.as_str()) };
                    if resp.clicked() {
                        clicked = Some(*id);
                    }
                });
            }
            if items.is_empty() {
                ui.weak("No markups yet.");
            }
        });
        if let Some(id) = clicked {
            self.focus_annot(id);
        }
    }

    /// Writes `<name>_comments.csv` next to the PDF: one row per markup, in page order.
    fn export_csv(&mut self) {
        self.commit_edit();
        let Some(src) = &self.path else { return };
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let out = src.with_file_name(format!("{stem}_comments.csv"));
        let mut order: Vec<&Annot> = self.annots.iter().collect();
        order.sort_by(|a, b| (a.page, a.bounds().min.y as i64, a.bounds().min.x as i64).cmp(&(b.page, b.bounds().min.y as i64, b.bounds().min.x as i64)));
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut csv = String::from("No,Page,Type,Colour,Comment\n");
        for (n, a) in order.iter().enumerate() {
            let colour = COLORS.iter().find(|(c, _)| *c == a.color).map(|(_, name)| *name).unwrap_or("Custom");
            csv += &format!("{},{},{},{},{}\n", n + 1, a.page + 1, a.kind_name(), colour, q(a.text().trim()));
        }
        match std::fs::write(&out, csv) {
            Ok(()) => self.toast(format!("Comment list → {}", out.display())),
            Err(e) => self.toast(format!("Could not write {}: {e}", out.display())),
        }
    }

    fn help(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = true;
        egui::Window::new("Shortcuts").open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("keys").striped(true).show(ui, |ui| {
                let rows = [
                    ("C", "Callout: press on the target, drag to where the note goes (click = quick note)"),
                    ("T", "Text box (click, or drag for width)"),
                    ("K / R / H", "Revision cloud / rectangle / highlight (drag)"),
                    ("A / L / P", "Arrow / line / pen"),
                    ("V or Esc", "Select: drag to move, handles to resize, double-click or Enter to edit text"),
                    ("Esc / Ctrl+Enter", "Finish typing"),
                    ("1–6", "Colour (applies to selection)"),
                    ("[ ]", "Line width   ·   { } text size"),
                    ("Del", "Delete selection   ·   Ctrl+D duplicate"),
                    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
                    ("Ctrl+E", "Export flattened PDF next to original (Ctrl+Shift+E: Save As)"),
                    ("Ctrl+Alt+E", "Export editable PDF (markups as movable comments for Bluebeam/Acrobat)"),
                    ("S / O / M", "Page thumbnails / overview of all pages / markup list"),
                    ("In overview", "Arrows move, Enter opens, Ctrl+wheel resizes, O or Esc closes"),
                    ("Wheel / Ctrl+wheel", "Scroll / zoom at cursor"),
                    ("Middle or right drag, Space+drag", "Pan"),
                    ("W / F / G", "Fit width / fit page / go to page"),
                    ("PgUp PgDn ← →  Home End", "Previous / next page"),
                ];
                for (k, d) in rows {
                    ui.strong(k);
                    ui.label(d);
                    ui.end_row();
                }
            });
            ui.weak("Markups autosave as you go; the original PDF is never modified.");
        });
        if !open {
            self.show_help = false;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.poll_worker(&ctx);

        if let Some(p) = ctx.input(|i| i.raw.dropped_files.iter().next().map(|f| f.path().to_path_buf())) {
            self.open(p);
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, now));
        let has_doc = !self.pages.is_empty();
        if has_doc && self.show_sidebar && !self.overview {
            egui::Panel::left("thumbs").resizable(true).default_size(170.0).size_range(90.0..=420.0).show(ui, |ui| self.sidebar(ui));
        }
        if self.show_markups {
            egui::Panel::right("markups").resizable(true).default_size(280.0).size_range(180.0..=600.0).show(ui, |ui| self.markup_list(ui));
        }
        if self.overview && has_doc {
            self.shortcuts(&ctx);
            egui::CentralPanel::default().show(ui, |ui| self.overview_grid(ui));
        } else {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.max_rect();
            if rect != self.canvas {
                self.canvas = rect;
            }
            if self.fit_pending && !self.pages.is_empty() && rect.width() > 50.0 {
                self.fit_pending = false;
                self.fit_width();
            }
            let resp = ui.allocate_rect(rect, Sense::click_and_drag());
            self.shortcuts(&ctx);
            self.canvas_input(ui, &resp);
            self.clamp_offset();
            if (self.zoom, self.offset) != self.view_key {
                self.view_key = (self.zoom, self.offset);
                self.last_view_change = now;
            }
            self.paint(ui);
            if self.pages.is_empty() {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, &self.status, FontId::proportional(18.0), Color32::from_gray(200));
            }
        });
        }
        self.text_editor(&ctx);
        self.goto_overlay(&ctx);
        self.help(&ctx);
        self.request_renders(&ctx, now);

        self.now = now;
        if !matches!(self.drag, Drag::Move { .. } | Drag::Handle { .. }) && self.editing.is_none() && now >= self.save_retry_at {
            self.save_sidecar();
        }
        if now < self.status_until && self.status_until.is_finite() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(self.status_until - now));
        }
    }

    fn on_exit(&mut self) {
        self.commit_edit();
        self.save_sidecar();
    }
}

/// Paints prims for a page whose top-left is at `origin` on screen, drawn at
/// `z` screen points per PDF point. `k` is the text calibration from `text_metrics`.
fn paint_prims(painter: &Painter, origin: Pos2, z: f32, prims: &[Prim], k: (f32, f32), alpha_mul: f32) {
    let col = |c: Rgb, a: u8| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (a as f32 * alpha_mul) as u8);
    let to = |p: Pos2| origin + p.to_vec2() * z;
    let map = |pts: &[Pos2]| pts.iter().map(|p| to(*p)).collect::<Vec<_>>();
    let min_w = if z < 0.3 { 0.7 } else { 1.0 };
    for prim in prims {
        match prim {
            Prim::Stroke { pts, closed, width, color } => {
                let s = Stroke::new((width * z).max(min_w), col(*color, 255));
                let pts = map(pts);
                if *closed {
                    painter.add(Shape::closed_line(pts, s));
                } else {
                    painter.add(Shape::line(pts, s));
                }
            }
            Prim::Fill { pts, color, alpha, outline } => {
                let stroke = outline.map(|(c, w)| Stroke::new((w * z).max(min_w), col(c, 255))).unwrap_or(Stroke::NONE);
                painter.add(Shape::convex_polygon(map(pts), col(*color, *alpha), stroke));
            }
            Prim::Text { origin: o, text, size, color } => {
                let fs = size * z * k.0;
                let o = to(*o);
                if fs < 3.0 {
                    // Too small to read: a grey bar where the line of text is.
                    let w = annot::text_width(text, *size) * z;
                    painter.line_segment([o - vec2(0.0, fs * 0.3), o + vec2(w, -fs * 0.3)], Stroke::new(fs.max(0.7), col(*color, 120)));
                    continue;
                }
                painter.text(o - vec2(0.0, k.1 * size * z), Align2::LEFT_TOP, text, FontId::new(fs, helv()), col(*color, 255));
            }
        }
    }
}

/// Name stamped on editable-export comments: $REDLINE_AUTHOR, else the account's
/// full name from /etc/passwd, else the login name.
fn author() -> String {
    if let Ok(a) = std::env::var("REDLINE_AUTHOR") {
        return a;
    }
    let user = std::env::var("USER").unwrap_or_default();
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| {
            p.lines().find(|l| l.split(':').next() == Some(user.as_str())).and_then(|l| l.split(':').nth(4)).map(|g| g.split(',').next().unwrap_or("").to_string())
        })
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(user)
}

fn text_of(a: &Annot) -> &str {
    match &a.kind {
        Kind::Callout { text, .. } | Kind::Text { text, .. } => text,
        _ => "",
    }
}

fn helv() -> FontFamily {
    FontFamily::Name("helv".into())
}

/// Screen fonts in the same order export uses (Liberation Sans, then the Noto
/// fallbacks in `fonts`), so what's on screen is what gets embedded.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut family = Vec::new();
    for (i, face) in crate::fonts::faces().iter().enumerate() {
        let name = format!("sys{i}");
        fonts.font_data.insert(name.clone(), std::sync::Arc::new(egui::FontData::from_owned(face.data().to_vec())));
        family.push(name);
    }
    family.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(helv(), family);
    ctx.set_fonts(fonts);
}
