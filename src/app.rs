use crate::annot::{self, Annot, Kind, Prim, Rgb};
use crate::render::{ExportMode, Layer, Req, Resp, Worker};
use crate::sidecar;
use crate::stamps;
use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontFamily, FontId, Key, Painter, PointerButton, Pos2, Rect, Sense, Shape, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

const GAP: f32 = 12.0;
const MAX_BASE_PX: f32 = 4096.0;
const HIT_PX: f32 = 7.0;
/// Selection outlines, handles and marquee.
const SEL: Color32 = Color32::from_rgb(0, 150, 255);
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

/// A file dialog to show. Dialogs run on their own thread so the window keeps
/// answering the compositor (Wayland flags a blocked one as not responding).
enum Pick {
    Open,
    /// Save-as for an export of this source PDF.
    Export(ExportMode, PathBuf),
    Images(stamps::Use),
}

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
/// Colour of placed dates.
const DATE_COLOR: Rgb = COLORS[3].0;

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
    /// Moves the selected markups on `page` together.
    Move { page: usize, ids: Vec<u64>, last: Pos2, before: Vec<Annot>, moved: bool },
    Handle { id: u64, handle: Handle, before: Vec<Annot> },
    /// Rubber-band selection; adds whatever it crosses on release.
    Marquee { page: usize, start: Pos2, cur: Pos2 },
    /// Sizing a stamp being placed (see `App::placing`).
    Place { page: usize, start: Pos2, cur: Pos2 },
    /// Swallow the rest of a gesture (e.g. the click that committed a text edit).
    Ignore,
}

/// What the next click on a page puts down, picked in the stamp panel.
#[derive(Clone)]
enum Place {
    Image { image: String, name: String, aspect: f32, width: f32 },
    /// The panel's date text.
    Date,
}

/// Selection modifiers for a click or drag.
#[derive(Clone, Copy, Default)]
struct Mods {
    /// Ctrl or Shift: add to / toggle the selection.
    add: bool,
    /// Alt: just the markup under the pointer, not the rest of its group.
    single: bool,
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
    show_stamps: bool,
    overview: bool,
    overview_tile: f32,
    overview_cursor: usize,
    prefs: sidecar::Prefs,
    prefs_dirty: bool,
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
    /// Selected markup ids; the last one is the lead whose colour/size the toolbar shows.
    selected: Vec<u64>,
    drag: Drag,
    editing: Option<u64>,
    edit_needs_focus: bool,
    goto: Option<String>,
    show_help: bool,

    library: stamps::Library,
    /// Library images as textures, by stored name; `tex_failed` ones won't load.
    textures: HashMap<String, TextureHandle>,
    tex_failed: HashSet<String>,
    placing: Option<Place>,
    /// Pointer over the canvas, for the placement preview.
    hover: Option<Pos2>,
    date_text: String,
    copy_all: bool,
    copy_pages: String,
    /// Library entry being renamed, and the name so far.
    renaming: Option<(usize, String)>,

    /// Screen text scale correction so Liberation Sans matches Helvetica widths,
    /// and its baseline offset as a fraction of the font size.
    text_k: Option<(f32, f32)>,

    lock: Option<std::fs::File>,
    read_only: bool,
    /// Shown while `read_only`: why markups aren't being autosaved.
    read_only_why: String,
    now: f64,
    save_retry_at: f64,
    /// When and on what the last arrow-key nudge happened, so a run of them is one undo step.
    last_nudge: (f64, Vec<u64>),
    failed: HashSet<usize>,
    /// Dialog asked for this frame; opened at the end of `ui()`, which has the window handle.
    ask: Option<Pick>,
    /// The open dialog's answer (no paths if cancelled).
    dialog: Option<mpsc::Receiver<(Pick, Vec<PathBuf>)>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, path: Option<PathBuf>) -> Self {
        install_fonts(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let prefs = sidecar::load_prefs();
        let (worker, status) = match Worker::spawn(cc.egui_ctx.clone()) {
            Ok(w) => (Some(w), String::from("Ctrl+O to open a PDF (or drop one here)")),
            Err(e) => (None, format!("PDFium failed to load: {e}")),
        };
        let mut app = Self::with(worker, status, prefs);
        app.library = stamps::Library::load();
        if let Some(p) = path {
            app.open(p);
        }
        app
    }

    fn with(worker: Option<Worker>, status: String, prefs: sidecar::Prefs) -> Self {
        Self {
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
            show_sidebar: prefs.show_sidebar,
            show_markups: prefs.show_markups && !prefs.show_stamps,
            show_stamps: prefs.show_stamps,
            overview: false,
            overview_tile: 240.0,
            overview_cursor: 0,
            date_text: stamps::today(&prefs.date_format),
            prefs,
            prefs_dirty: false,
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
            selected: Vec::new(),
            drag: Drag::None,
            editing: None,
            edit_needs_focus: false,
            goto: None,
            show_help: false,
            library: stamps::Library::default(),
            textures: HashMap::new(),
            tex_failed: HashSet::new(),
            placing: None,
            hover: None,
            copy_all: true,
            copy_pages: String::new(),
            renaming: None,
            text_k: None,
            lock: None,
            read_only: false,
            read_only_why: String::new(),
            now: 0.0,
            save_retry_at: 0.0,
            last_nudge: (f64::NEG_INFINITY, Vec::new()),
            failed: HashSet::new(),
            ask: None,
            dialog: None,
        }
    }

    // -----------------------------------------------------------------------
    // Document

    fn open(&mut self, path: PathBuf) {
        if self.worker.is_none() {
            return;
        }
        let path = dunce::canonicalize(&path).unwrap_or(path);
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
        self.selected.clear();
        self.editing = None;
        self.drag = Drag::None;
        self.placing = None;
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
        let Some(src) = &self.path else { return };
        if ask {
            self.ask = Some(Pick::Export(mode, src.clone()));
        } else {
            self.export_to(mode, default_export(src, mode));
        }
    }

    fn export_to(&mut self, mode: ExportMode, out: PathBuf) {
        let (Some(w), Some(src)) = (&self.worker, &self.path) else { return };
        // Compare real paths so a symlinked folder can't alias the original.
        let real_out = out.parent().and_then(|d| dunce::canonicalize(d).ok()).zip(out.file_name()).map(|(d, f)| d.join(f)).unwrap_or_else(|| out.clone());
        if real_out == *src || dunce::canonicalize(&out).is_ok_and(|o| o == *src) {
            self.toast("Refusing to overwrite the original PDF".into());
            return;
        }
        let annots: Vec<Annot> = self.annots.iter().filter(|a| a.text_size().is_none() || !text_of(a).trim().is_empty()).cloned().collect();
        let author = if self.prefs.author.trim().is_empty() { default_author() } else { self.prefs.author.trim().to_string() };
        w.send(Req::Export { src: src.clone(), out, annots, mode, author });
        self.status = "Exporting…".into();
        self.status_until = f64::INFINITY;
    }

    /// Opens the dialog asked for this frame, unless one is already up.
    fn show_dialog(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(pick) = self.ask.take() else { return };
        if self.dialog.is_some() {
            return;
        }
        let d = rfd::FileDialog::new().set_parent(frame);
        let d = match &pick {
            Pick::Open => d.add_filter("PDF", &["pdf"]),
            Pick::Export(mode, src) => {
                let name = default_export(src, *mode).file_name().unwrap().to_string_lossy().into_owned();
                let d = d.add_filter("PDF", &["pdf"]).set_file_name(name);
                match src.parent() {
                    Some(dir) => d.set_directory(dir),
                    None => d,
                }
            }
            Pick::Images(_) => d.add_filter("PNG or JPEG image", &["png", "jpg", "jpeg"]),
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let paths = match pick {
                Pick::Open => d.pick_file().into_iter().collect(),
                Pick::Export(..) => d.save_file().into_iter().collect(),
                Pick::Images(_) => d.pick_files().unwrap_or_default(),
            };
            tx.send((pick, paths)).ok();
            ctx.request_repaint();
        });
        self.dialog = Some(rx);
    }

    fn poll_dialog(&mut self) {
        let Some(rx) = &self.dialog else { return };
        let (pick, paths) = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => (Pick::Open, Vec::new()),
        };
        self.dialog = None;
        match pick {
            Pick::Open => {
                if let Some(p) = paths.into_iter().next() {
                    self.open(p);
                }
            }
            // Only if the same PDF is still open.
            Pick::Export(mode, src) => {
                if let Some(out) = paths.into_iter().next().filter(|_| self.path.as_ref() == Some(&src)) {
                    self.export_to(mode, out);
                }
            }
            Pick::Images(kind) => {
                if !paths.is_empty() {
                    self.import_images(kind, paths);
                }
            }
        }
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
        self.annots.push(Annot { id, page, color: self.color(), width: self.width, kind, wrap_w: None, group: None });
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
        self.selected = vec![id];
    }

    fn commit_edit(&mut self) {
        if let Some(id) = self.editing.take() {
            if self.find(id).is_some_and(|a| text_of(a).trim().is_empty()) {
                self.annots.retain(|a| a.id != id);
                self.selected.retain(|&s| s != id);
            } else if let Some(a) = self.find_mut(id) {
                a.fit_box_w();
            }
            self.changed();
        }
    }

    fn hit(&self, page: usize, p: Pos2) -> Option<u64> {
        let tol = HIT_PX / self.zoom;
        let mut hits = self.annots.iter().rev().filter(|a| a.page == page && a.distance(p) <= tol);
        // Prefer a selected one, then topmost.
        hits.clone().find(|a| self.selected.contains(&a.id)).or_else(|| hits.next()).map(|a| a.id)
    }

    /// The selected markup when exactly one is selected (handles, text editing).
    fn sole(&self) -> Option<&Annot> {
        match self.selected[..] {
            [id] => self.find(id),
            _ => None,
        }
    }

    /// The most recently selected markup that still exists.
    fn lead(&self) -> Option<&Annot> {
        self.selected.iter().rev().find_map(|&id| self.find(id))
    }

    fn lead_text_size(&self) -> Option<f32> {
        self.selected.iter().rev().filter_map(|&id| self.find(id)).find_map(|a| a.text_size())
    }

    fn toggle_selected(&mut self, id: u64) {
        if self.selected.contains(&id) {
            self.selected.retain(|&s| s != id);
        } else {
            self.selected.push(id);
        }
    }

    /// `id` plus, unless `single`, the rest of its group on its page.
    fn with_group(&self, id: u64, single: bool) -> Vec<u64> {
        match self.find(id) {
            Some(a) if !single && a.group.is_some() => {
                self.annots.iter().filter(|b| b.page == a.page && b.group == a.group).map(|b| b.id).collect()
            }
            Some(_) => vec![id],
            None => vec![],
        }
    }

    fn select_more(&mut self, ids: Vec<u64>) {
        for id in ids {
            if !self.selected.contains(&id) {
                self.selected.push(id);
            }
        }
    }

    fn next_group(&self) -> u64 {
        self.annots.iter().filter_map(|a| a.group).max().unwrap_or(0) + 1
    }

    /// Groups the selection, one group per page it spans.
    fn group_selected(&mut self) {
        let mut pages: Vec<usize> = self.annots.iter().filter(|a| self.selected.contains(&a.id)).map(|a| a.page).collect();
        pages.sort();
        pages.dedup();
        pages.retain(|&p| self.annots.iter().filter(|a| a.page == p && self.selected.contains(&a.id)).count() > 1);
        if pages.is_empty() {
            self.toast("Select two or more markups to group".into());
            return;
        }
        self.snapshot();
        for (g, p) in (self.next_group()..).zip(pages) {
            for a in self.annots.iter_mut().filter(|a| a.page == p && self.selected.contains(&a.id)) {
                a.group = Some(g);
            }
        }
        self.changed();
        self.toast("Grouped: click any part to select the set, Alt+click for one piece".into());
    }

    fn ungroup_selected(&mut self) {
        if self.annots.iter().any(|a| self.selected.contains(&a.id) && a.group.is_some()) {
            self.edit_selected(|a| a.group = None);
            self.toast("Ungrouped".into());
        }
    }

    fn handles(a: &Annot) -> Vec<(Handle, Pos2)> {
        match &a.kind {
            Kind::Cloud { rect } | Kind::Rect { rect } | Kind::Highlight { rect } | Kind::Image { rect, .. } => {
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
        let a = self.sole()?;
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
            (Kind::Image { rect, .. }, Handle::Corner(opposite)) => {
                *rect = annot::aspect_rect(opposite, p, rect.width() / rect.height().max(0.01));
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
        if !self.selected.is_empty() {
            self.snapshot();
            let sel = std::mem::take(&mut self.selected);
            self.annots.retain(|a| !sel.contains(&a.id));
            self.changed();
        }
    }

    /// Applies `f` to every selected markup as one undo step.
    fn edit_selected(&mut self, mut f: impl FnMut(&mut Annot)) {
        if self.selected.is_empty() {
            return;
        }
        self.snapshot();
        for a in self.annots.iter_mut().filter(|a| self.selected.contains(&a.id)) {
            f(a);
        }
        self.changed();
    }

    fn undo(&mut self) {
        self.commit_edit();
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.annots, prev));
            self.selected.clear();
            self.changed();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.annots, next));
            self.selected.clear();
            self.changed();
        }
    }

    fn set_tool(&mut self, t: Tool) {
        self.commit_edit();
        self.placing = None;
        self.tool = t;
        if t != Tool::Select {
            self.selected.clear();
        }
    }

    fn set_color(&mut self, idx: usize) {
        self.tool_color.insert(self.tool, idx);
        self.edit_selected(|a| a.color = COLORS[idx].0);
    }

    fn set_width(&mut self, w: f32) {
        self.width = w.clamp(0.5, 12.0);
        let w = self.width;
        self.edit_selected(|a| a.width = w);
    }

    fn step_text_size(&mut self, dir: i32) {
        let cur = self.lead_text_size().unwrap_or(self.text_size);
        let i = TEXT_SIZES.iter().position(|&s| s >= cur).unwrap_or(TEXT_SIZES.len() - 1) as i32;
        let next = TEXT_SIZES[(i + dir).clamp(0, TEXT_SIZES.len() as i32 - 1) as usize];
        self.set_text_size(next);
    }

    fn set_text_size(&mut self, s: f32) {
        self.text_size = s;
        if self.lead_text_size().is_some() {
            self.edit_selected(|a| {
                if let Kind::Callout { size, .. } | Kind::Text { size, .. } = &mut a.kind {
                    *size = s;
                }
            });
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
                self.ask = Some(Pick::Open);
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
            if pressed(Key::G) {
                if shift { self.ungroup_selected() } else { self.group_selected() }
            }
            if pressed(Key::D) && !self.selected.is_empty() {
                self.snapshot();
                let d = vec2(12.0, 12.0) / self.zoom.max(0.1);
                let copies: Vec<Annot> = self.annots.iter().filter(|a| self.selected.contains(&a.id)).cloned().collect();
                self.selected.clear();
                // Copies of a group form a new group.
                let mut groups = HashMap::new();
                let mut next_group = self.next_group();
                for a in copies {
                    let id = self.next_id;
                    self.next_id += 1;
                    let group = a.group.map(|g| {
                        *groups.entry(g).or_insert_with(|| {
                            next_group += 1;
                            next_group - 1
                        })
                    });
                    let mut b = Annot { id, group, ..a };
                    b.translate(d);
                    self.annots.push(b);
                    self.selected.push(id);
                }
                self.changed();
            }
            if pressed(Key::A) && !self.pages.is_empty() && !self.overview {
                let page = self.current_page();
                self.set_tool(Tool::Select);
                self.selected = self.annots.iter().filter(|a| a.page == page).map(|a| a.id).collect();
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
            self.toggle_right_panel(false);
        }
        if pressed(Key::I) {
            self.toggle_right_panel(true);
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
            if self.placing.take().is_some() {
                self.toast("Placing cancelled".into());
            } else if !self.selected.is_empty() {
                self.selected.clear()
            } else {
                self.set_tool(Tool::Select)
            }
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
            if let Some(id) = self.sole().filter(|a| a.text_size().is_some()).map(|a| a.id) {
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
        // With a selection the arrows nudge it (1 screen pixel, Shift 10); otherwise they navigate.
        let nudging = !self.selected.is_empty();
        if nudging && matches!(self.drag, Drag::None) {
            let dirs = [(Key::ArrowLeft, vec2(-1.0, 0.0)), (Key::ArrowRight, vec2(1.0, 0.0)), (Key::ArrowUp, vec2(0.0, -1.0)), (Key::ArrowDown, vec2(0.0, 1.0))];
            let d = dirs.iter().filter(|(k, _)| pressed(*k)).fold(Vec2::ZERO, |s, (_, v)| s + *v);
            if d != Vec2::ZERO {
                let px = if shift { 10.0 } else { 1.0 };
                self.nudge(d * px / self.zoom, ctx.input(|i| i.time));
            }
        }
        let n = self.pages.len();
        if n > 0 {
            let cur = self.current_page();
            if pressed(Key::PageDown) || (!nudging && pressed(Key::ArrowRight)) {
                self.goto_page((cur + 1).min(n - 1));
            }
            if pressed(Key::PageUp) || (!nudging && pressed(Key::ArrowLeft)) {
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
        if !nudging && pressed(Key::ArrowDown) {
            self.offset.y += step;
        }
        if !nudging && pressed(Key::ArrowUp) {
            self.offset.y -= step;
        }
    }

    /// Moves the selection by `d` page points. Nudges of the same selection
    /// less than a second apart (a held key) share one undo step.
    fn nudge(&mut self, d: Vec2, now: f64) {
        if now - self.last_nudge.0 > 1.0 || self.last_nudge.1 != self.selected {
            self.snapshot();
        }
        self.last_nudge = (now, self.selected.clone());
        for a in self.annots.iter_mut().filter(|a| self.selected.contains(&a.id)) {
            a.translate(d);
        }
        self.changed();
    }

    fn canvas_input(&mut self, ui: &Ui, resp: &egui::Response) {
        let ctx = ui.ctx().clone();
        let (hover, scroll, zoom_delta, space, press_origin, latest) = ctx.input(|i| {
            (i.pointer.hover_pos(), i.smooth_scroll_delta, i.zoom_delta(), i.key_down(Key::Space), i.pointer.press_origin(), i.pointer.latest_pos())
        });
        let mods = ctx.input(|i| Mods { add: i.modifiers.command || i.modifiers.shift, single: i.modifiers.alt });
        self.hover = hover.filter(|_| resp.hovered());

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
            self.begin_drag(start, space, mods);
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
                self.click(p, resp.double_clicked(), mods);
            }
        }

        // Cursor.
        if space || matches!(self.drag, Drag::Pan) {
            ctx.set_cursor_icon(if matches!(self.drag, Drag::Pan) { CursorIcon::Grabbing } else { CursorIcon::Grab });
        } else if resp.hovered() && self.placing.is_some() {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
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

    fn begin_drag(&mut self, start: Pos2, space: bool, mods: Mods) {
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
        if self.placing.is_some() {
            self.drag = Drag::Place { page, start: p, cur: p };
            return;
        }
        self.drag = match self.tool {
            Tool::Select => {
                if let Some((id, handle)) = self.hit_handle(page, p) {
                    Drag::Handle { id, handle, before: self.annots.clone() }
                } else if let Some(id) = self.hit(page, p) {
                    if !self.selected.contains(&id) {
                        if !mods.add {
                            self.selected.clear();
                        }
                        self.select_more(self.with_group(id, mods.single));
                    } else if mods.single {
                        // Alt-drag one piece out of a selected group.
                        self.selected = vec![id];
                    }
                    // Markups selected on other pages stay put.
                    let ids = self.annots.iter().filter(|a| a.page == page && self.selected.contains(&a.id)).map(|a| a.id).collect();
                    Drag::Move { page, ids, last: p, before: self.annots.clone(), moved: false }
                } else {
                    if !mods.add {
                        self.selected.clear();
                    }
                    Drag::Marquee { page, start: p, cur: p }
                }
            }
            Tool::Pen => Drag::Pen { page, pts: vec![p] },
            _ => Drag::Create { page, start: p, cur: p },
        };
    }

    fn update_drag(&mut self, screen: Pos2, delta: Vec2) {
        let page = match &self.drag {
            Drag::Create { page, .. } | Drag::Pen { page, .. } | Drag::Move { page, .. } | Drag::Marquee { page, .. } | Drag::Place { page, .. } => {
                Some(*page)
            }
            Drag::Handle { id, .. } => self.find(*id).map(|a| a.page),
            _ => None,
        };
        let p = page.map(|pg| self.screen_to_page(pg, screen)).unwrap_or_default();
        let min_step = 1.0 / self.zoom;
        match &mut self.drag {
            Drag::Pan => self.offset -= delta / self.zoom,
            Drag::Create { cur, .. } | Drag::Marquee { cur, .. } | Drag::Place { cur, .. } => *cur = p,
            Drag::Pen { pts, .. } => {
                if pts.last().is_none_or(|l| (*l - p).length() >= min_step) {
                    pts.push(p);
                }
            }
            Drag::Move { ids, last, moved, .. } => {
                let d = p - *last;
                *last = p;
                *moved = true;
                for a in self.annots.iter_mut().filter(|a| ids.contains(&a.id)) {
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
            Drag::Handle { id, before, .. } => {
                self.undo.push(before);
                self.redo.clear();
                self.changed();
                // The next placement of this stamp uses the size just chosen.
                if let Some(Kind::Image { rect, image, .. }) = self.find(id).map(|a| a.kind.clone()) {
                    self.remember_width(&image, rect.width());
                }
            }
            Drag::Marquee { page, start, cur } => {
                let r = Rect::from_two_pos(start, cur);
                let hits: Vec<u64> = self.annots.iter().filter(|a| a.page == page && a.touches(r)).map(|a| a.id).collect();
                for id in hits {
                    self.select_more(self.with_group(id, false));
                }
            }
            Drag::Place { page, start, cur } => self.place(page, start, cur),
            _ => {}
        }
    }

    fn click(&mut self, screen: Pos2, double: bool, mods: Mods) {
        if self.editing.is_some() {
            self.commit_edit();
            return;
        }
        let Some(page) = self.page_at(screen) else {
            if !mods.add {
                self.selected.clear();
            }
            return;
        };
        let p = self.screen_to_page(page, screen);
        if self.placing.is_some() {
            self.place(page, p, p);
            return;
        }
        match self.tool {
            Tool::Select if mods.add => {
                if let Some(id) = self.hit(page, p) {
                    let ids = self.with_group(id, mods.single);
                    if self.selected.contains(&id) {
                        self.selected.retain(|s| !ids.contains(s));
                    } else {
                        self.select_more(ids);
                    }
                }
            }
            Tool::Select => {
                let hit = self.hit(page, p);
                self.selected = hit.map(|id| self.with_group(id, mods.single)).unwrap_or_default();
                // Double-click edits the text under the pointer, even inside a group.
                if let Some(id) = hit.filter(|&id| double && self.find(id).is_some_and(|a| a.text_size().is_some())) {
                    self.snapshot();
                    self.start_edit(id);
                }
            }
            Tool::Callout | Tool::Text => self.create(page, p, p, false),
            _ => {
                // Clicking an existing markup with a drawing tool selects it for quick edits.
                if let Some(id) = self.hit(page, p) {
                    self.tool = Tool::Select;
                    self.selected = self.with_group(id, mods.single);
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
    // Stamps

    fn start_placing(&mut self, place: Place) {
        self.commit_edit();
        let what = match &place {
            Place::Image { name, .. } => format!("\"{name}\""),
            Place::Date => "the date".into(),
        };
        self.placing = Some(place);
        self.selected.clear();
        self.toast(format!("Click on the sheet to place {what} (drag to size it) · Esc cancels"));
    }

    /// The markup `place` puts down for a press at `start` released at `end`:
    /// centred on a click, or (images) sized by the drag.
    fn placed_kind(&self, place: &Place, start: Pos2, end: Pos2) -> Kind {
        let dragged = (end - start).length() * self.zoom > 6.0;
        match place {
            Place::Image { image, name, aspect, width } => {
                let rect = if dragged { annot::aspect_rect(start, end, *aspect) } else { Rect::from_center_size(start, vec2(*width, *width / aspect)) };
                Kind::Image { rect, image: image.clone(), name: name.clone() }
            }
            Place::Date => {
                let size = self.text_size;
                let text = self.date_text.trim().to_string();
                let box_w = (annot::text_width(&text, size) + 2.0 * annot::pad(size) + 0.5).max(4.0 * size);
                let h = annot::box_rect(Pos2::ZERO, box_w, &text, size).height();
                Kind::Text { pos: start - vec2(box_w, h) / 2.0, box_w, text, size }
            }
        }
    }

    fn place(&mut self, page: usize, start: Pos2, end: Pos2) {
        let Some(place) = self.placing.take() else { return };
        if matches!(place, Place::Date) && self.date_text.trim().is_empty() {
            self.toast("Type a date in the stamp panel first".into());
            return;
        }
        let kind = self.placed_kind(&place, start, end);
        if let (Place::Image { image, .. }, Kind::Image { rect, .. }) = (&place, &kind) {
            if (end - start).length() * self.zoom > 6.0 {
                self.remember_width(image, rect.width());
            }
        }
        self.snapshot();
        let id = self.add(page, kind);
        if matches!(place, Place::Date) {
            if let Some(a) = self.find_mut(id) {
                a.color = DATE_COLOR;
            }
        }
        // Ready to nudge it into the box.
        self.tool = Tool::Select;
        self.selected = vec![id];
    }

    fn remember_width(&mut self, image: &str, width: f32) {
        for e in self.library.entries.iter_mut().filter(|e| e.image == image) {
            e.width = width;
        }
        if let Err(e) = self.library.save() {
            self.toast(format!("Could not save the stamp library: {e}"));
        }
    }

    fn import_images(&mut self, kind: stamps::Use, paths: Vec<PathBuf>) {
        let mut msgs = Vec::new();
        for p in paths {
            let file = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            match self.library.import(&p, kind) {
                Ok((_, true)) => msgs.push(format!("Added {file} (white background made transparent)")),
                Ok((_, false)) => msgs.push(format!("Added {file}")),
                Err(e) => msgs.push(format!("Could not add {file}: {e}")),
            }
        }
        self.toast(msgs.join(" · "));
    }

    /// Copies the selection on the lead's page to `targets` at the same spot
    /// (measured from the nearest page corner), one group per page. Pages that
    /// already carry one of the selected images are skipped. One undo step.
    fn copy_selection_to(&mut self, targets: &[usize]) {
        let Some(src_page) = self.lead().map(|a| a.page) else { return };
        let src: Vec<Annot> = self.annots.iter().filter(|a| a.page == src_page && self.selected.contains(&a.id)).cloned().collect();
        let bounds = src.iter().fold(Rect::NOTHING, |r, a| r.union(a.bounds()));
        let images: Vec<&String> = src.iter().filter_map(|a| if let Kind::Image { image, .. } = &a.kind { Some(image) } else { None }).collect();
        let stamped = |p: usize| self.annots.iter().any(|a| a.page == p && matches!(&a.kind, Kind::Image { image, .. } if images.contains(&image)));
        let (pages, skipped): (Vec<usize>, Vec<usize>) = targets.iter().copied().filter(|&p| p != src_page && p < self.pages.len()).partition(|&p| !stamped(p));
        if pages.is_empty() {
            let why = if skipped.is_empty() { "no other pages chosen" } else { "every chosen page already has this stamp" };
            self.toast(format!("Nothing copied: {why}"));
            return;
        }
        self.snapshot();
        let mut g = self.next_group();
        let grouped = src.len() > 1;
        // The original set becomes a group too, so it moves as one like the copies.
        if grouped {
            for a in self.annots.iter_mut().filter(|a| a.page == src_page && self.selected.contains(&a.id)) {
                a.group = Some(g);
            }
            g += 1;
        }
        for &p in &pages {
            let d = stamps::corner_offset(bounds, self.pages[src_page], self.pages[p]);
            for a in &src {
                let mut b = Annot { id: self.next_id, page: p, group: grouped.then_some(g), ..a.clone() };
                self.next_id += 1;
                b.translate(d);
                self.annots.push(b);
            }
            g += 1;
        }
        self.changed();
        let list = |v: &[usize]| v.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", ");
        let mut msg = format!("Copied to {} page{}", pages.len(), if pages.len() == 1 { "" } else { "s" });
        if !skipped.is_empty() {
            msg += &format!(" · skipped page {} (already stamped)", list(&skipped));
        }
        self.toast(msg + " · check them in the overview (O)");
    }

    fn toggle_right_panel(&mut self, stamps: bool) {
        if stamps {
            self.show_stamps = !self.show_stamps;
            self.show_markups &= !self.show_stamps;
        } else {
            self.show_markups = !self.show_markups;
            self.show_stamps &= !self.show_markups;
        }
    }

    /// Uploads library images used by markups or listed in the panel.
    fn load_textures(&mut self, ctx: &egui::Context) {
        let names: HashSet<String> = self
            .annots
            .iter()
            .filter_map(|a| if let Kind::Image { image, .. } = &a.kind { Some(image) } else { None })
            .chain(self.library.entries.iter().map(|e| &e.image))
            .filter(|n| !self.textures.contains_key(*n) && !self.tex_failed.contains(*n))
            .cloned()
            .collect();
        for name in names {
            match stamps::decode(&name) {
                Ok(img) => {
                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    let opts = TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR };
                    self.textures.insert(name.clone(), ctx.load_texture(format!("img-{name}"), ci, opts));
                }
                Err(e) => {
                    self.tex_failed.insert(name);
                    self.toast(format!("Stamp image missing: {e}"));
                }
            }
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
                paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &prims, k, 1.0, &self.textures);
            }
            // Live preview.
            match &self.drag {
                Drag::Create { page, start, cur } if *page == i => {
                    let preview = self.preview_kind(*start, *cur);
                    if let Some(kind) = preview {
                        let a = Annot { id: 0, page: i, color: self.color(), width: self.width, kind, wrap_w: None, group: None };
                        paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.7, &self.textures);
                    }
                }
                Drag::Pen { page, pts } if *page == i => {
                    let a = Annot { id: 0, page: i, color: self.color(), width: self.width, kind: Kind::Pen { pts: pts.clone() }, wrap_w: None, group: None };
                    paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 1.0, &self.textures);
                }
                Drag::Marquee { page, start, cur } if *page == i => {
                    let r = Rect::from_two_pos(self.page_to_screen(i, *start), self.page_to_screen(i, *cur));
                    pp.rect(r, 0.0, SEL.gamma_multiply(0.12), Stroke::new(1.0, SEL), egui::StrokeKind::Middle);
                }
                _ => {}
            }
            // Where a stamp being placed will land: the drag so far, or centred on the pointer.
            if let Some(place) = &self.placing {
                let at = match &self.drag {
                    Drag::Place { page, start, cur } => (*page == i).then_some((*start, *cur)),
                    _ => self.hover.filter(|h| self.page_at(*h) == Some(i)).map(|h| self.screen_to_page(i, h)).map(|p| (p, p)),
                };
                if let Some((start, cur)) = at {
                    let color = if matches!(place, Place::Date) { DATE_COLOR } else { self.color() };
                    let a = Annot { id: 0, page: i, color, width: self.width, kind: self.placed_kind(place, start, cur), wrap_w: None, group: None };
                    paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.6, &self.textures);
                    let b = a.bounds();
                    pp.rect_stroke(Rect::from_min_max(self.page_to_screen(i, b.min), self.page_to_screen(i, b.max)), 0.0, Stroke::new(1.0, SEL), egui::StrokeKind::Outside);
                }
            }
            // Selection, plus what a marquee in progress would add.
            let marquee = match &self.drag {
                Drag::Marquee { page, start, cur } if *page == i => Some(Rect::from_two_pos(*start, *cur)),
                _ => None,
            };
            let sole = self.sole().map(|a| a.id);
            for a in self.annots.iter().filter(|a| a.page == i && (self.selected.contains(&a.id) || marquee.is_some_and(|r| a.touches(r)))) {
                let b = a.bounds();
                let r = Rect::from_min_max(self.page_to_screen(i, b.min), self.page_to_screen(i, b.max)).expand(3.0);
                pp.rect_stroke(r, 0.0, Stroke::new(1.0, SEL), egui::StrokeKind::Outside);
                if sole == Some(a.id) {
                    for (_, hp) in Self::handles(a) {
                        let c = self.page_to_screen(i, hp);
                        painter.rect(Rect::from_center_size(c, Vec2::splat(8.0)), 1.0, Color32::WHITE, Stroke::new(1.5, SEL), egui::StrokeKind::Middle);
                    }
                }
            }
            // One outline round each wholly selected group.
            let mut groups: HashMap<u64, (Rect, bool)> = HashMap::new();
            for a in self.annots.iter().filter(|a| a.page == i) {
                if let Some(g) = a.group {
                    let e = groups.entry(g).or_insert((Rect::NOTHING, true));
                    e.0 = e.0.union(a.bounds());
                    e.1 &= self.selected.contains(&a.id);
                }
            }
            for (b, all) in groups.into_values() {
                if all {
                    let r = Rect::from_min_max(self.page_to_screen(i, b.min), self.page_to_screen(i, b.max)).expand(8.0);
                    pp.rect_stroke(r, 3.0, Stroke::new(1.0, SEL.gamma_multiply(0.7)), egui::StrokeKind::Outside);
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
                self.ask = Some(Pick::Open);
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
                self.toggle_right_panel(false);
            }
            if ui.selectable_label(self.show_stamps, "Stamps I").on_hover_text("Seals, signatures and dates").clicked() {
                self.toggle_right_panel(true);
            }
            ui.separator();
            for (t, label, k) in TOOLS {
                let text = format!("{label} {}", k.name());
                if ui.selectable_label(self.tool == t, text).clicked() {
                    self.set_tool(t);
                }
            }
            ui.separator();
            let cur = self.lead().map(|a| a.color).unwrap_or(self.color());
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
            let cur_size = self.lead_text_size().unwrap_or(self.text_size);
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
                if self.selected.len() > 1 {
                    ui.label(format!("{} selected", self.selected.len()));
                    ui.separator();
                }
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
            paint_prims(&clip, r.min, z, &a.prims(), k, 1.0, &self.textures);
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
        self.selected = vec![id];
        self.offset = c.to_vec2() - self.canvas.size() / (2.0 * self.zoom);
    }

    fn stamp_panel(&mut self, ui: &mut Ui) {
        ui.strong("Stamps & signatures");
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut remove = None;
            for kind in [stamps::Use::Stamp, stamps::Use::Signature] {
                ui.horizontal(|ui| {
                    ui.strong(if kind == stamps::Use::Stamp { "Seals" } else { "Signatures" });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Add…").on_hover_text("PNG or JPEG. A scan's white background is made transparent.").clicked() {
                            self.ask = Some(Pick::Images(kind));
                        }
                    });
                });
                let rows: Vec<usize> = (0..self.library.entries.len()).filter(|&i| self.library.entries[i].kind == kind).collect();
                if rows.is_empty() {
                    ui.weak(if kind == stamps::Use::Stamp { "Add a scan of each state's seal." } else { "Add a scan of your signature." });
                }
                for i in rows {
                    if self.library_row(ui, i) {
                        remove = Some(i);
                    }
                }
                ui.add_space(10.0);
            }
            if let Some(i) = remove {
                let e = self.library.entries.remove(i);
                self.renaming = None;
                let saved = self.library.save().map_err(|err| format!(" (could not save the library: {err})")).err().unwrap_or_default();
                self.toast(format!("Removed \"{}\" from the library; copies already placed keep their image{saved}", e.name));
            }

            ui.strong("Date");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.date_text).desired_width(110.0));
                let cur = self.prefs.date_format.clone();
                egui::ComboBox::from_id_salt("datefmt").selected_text("Format").show_ui(ui, |ui| {
                    for f in stamps::DATE_FORMATS {
                        if ui.selectable_label(f == cur, stamps::today(f)).clicked() {
                            self.prefs.date_format = f.to_string();
                            self.prefs_dirty = true;
                            self.date_text = stamps::today(f);
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                let active = matches!(self.placing, Some(Place::Date));
                if ui.add(egui::Button::new("Place date").selected(active)).on_hover_text("Then click on the sheet; uses the current text size").clicked() {
                    if self.pages.is_empty() { self.toast("Open a PDF first".into()) } else { self.start_placing(Place::Date) }
                }
                if ui.button("Today").clicked() {
                    self.date_text = stamps::today(&self.prefs.date_format);
                }
            });
            ui.add_space(10.0);
            ui.separator();
            self.copy_controls(ui);
        });
    }

    /// One library image: click to place, right-click to rename or remove.
    /// Returns true when removal was asked for.
    fn library_row(&mut self, ui: &mut Ui, i: usize) -> bool {
        let e = self.library.entries[i].clone();
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 52.0), Sense::click());
        let active = matches!(&self.placing, Some(Place::Image { image, .. }) if *image == e.image);
        if active || resp.hovered() {
            let bg = if active { ui.visuals().selection.bg_fill } else { ui.visuals().widgets.hovered.weak_bg_fill };
            ui.painter().rect_filled(rect, 3.0, bg);
        }
        let slot = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(64.0, 44.0));
        ui.painter().rect_filled(slot, 2.0, Color32::WHITE);
        let inner = slot.shrink(2.0);
        let size = if e.aspect > inner.aspect_ratio() { vec2(inner.width(), inner.width() / e.aspect) } else { vec2(inner.height() * e.aspect, inner.height()) };
        if let Some(t) = self.textures.get(&e.image) {
            ui.painter().image(t.id(), Rect::from_center_size(inner.center(), size), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }
        let name_rect = Rect::from_min_max(pos2(slot.max.x + 8.0, rect.min.y + 4.0), pos2(rect.max.x - 4.0, rect.max.y - 4.0));
        if let Some((_, buf)) = self.renaming.as_mut().filter(|(r, _)| *r == i) {
            let edit = ui.put(Rect::from_center_size(name_rect.center(), vec2(name_rect.width(), 20.0)), egui::TextEdit::singleline(buf));
            if edit.lost_focus() {
                let name = buf.trim().to_string();
                self.renaming = None;
                if !name.is_empty() {
                    self.library.entries[i].name = name;
                    if let Err(err) = self.library.save() {
                        self.toast(format!("Could not save the stamp library: {err}"));
                    }
                }
            } else if !edit.has_focus() {
                edit.request_focus();
            }
        } else {
            let colour = ui.visuals().text_color();
            let galley = ui.painter().layout(e.name.clone(), FontId::proportional(13.0), colour, name_rect.width());
            ui.painter().galley(pos2(name_rect.min.x, rect.center().y - galley.size().y / 2.0), galley, colour);
        }
        let resp = resp.on_hover_text("Click, then click on the sheet to place it (or drag to size it). Right-click to rename or remove.");
        if resp.clicked() {
            if self.pages.is_empty() {
                self.toast("Open a PDF first".into());
            } else {
                self.start_placing(Place::Image { image: e.image.clone(), name: e.name.clone(), aspect: e.aspect, width: e.width });
            }
        }
        let mut remove = false;
        resp.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                self.renaming = Some((i, e.name.clone()));
                ui.close();
            }
            if ui.button("Remove from library").on_hover_text("Copies already placed keep their image").clicked() {
                remove = true;
                ui.close();
            }
        });
        remove
    }

    /// Group / ungroup and copy-to-pages for the current selection.
    fn copy_controls(&mut self, ui: &mut Ui) {
        ui.strong("Selection");
        let Some(page) = self.lead().map(|a| a.page) else {
            ui.weak("Place the seal, signature and date on one sheet, then select them (with Select V, drag a box around them) to group them or copy them to the other sheets.");
            return;
        };
        let n = self.annots.iter().filter(|a| a.page == page && self.selected.contains(&a.id)).count();
        ui.label(format!("{n} markup{} on page {}", if n == 1 { "" } else { "s" }, page + 1));
        ui.horizontal(|ui| {
            if ui.add_enabled(n > 1, egui::Button::new("Group")).on_hover_text("Ctrl+G: select and move them as one").clicked() {
                self.group_selected();
            }
            let grouped = self.annots.iter().any(|a| self.selected.contains(&a.id) && a.group.is_some());
            if ui.add_enabled(grouped, egui::Button::new("Ungroup")).on_hover_text("Ctrl+Shift+G").clicked() {
                self.ungroup_selected();
            }
        });
        ui.add_space(6.0);
        ui.label("Copy to the same spot on");
        ui.radio_value(&mut self.copy_all, true, format!("All other pages ({})", self.pages.len().saturating_sub(1)));
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.copy_all, false, "Pages");
            let r = ui.add(egui::TextEdit::singleline(&mut self.copy_pages).hint_text("e.g. 2-14, 16").desired_width(110.0));
            if r.gained_focus() || r.changed() {
                self.copy_all = false;
            }
        });
        let tip = "Placed the same distance from the nearest page corner, so it stays in the title block on other sheet sizes. Pages that already have this stamp are skipped. One undo step.";
        if ui.button("Copy to pages").on_hover_text(tip).clicked() {
            let targets = if self.copy_all { Ok((0..self.pages.len()).collect()) } else { stamps::parse_pages(&self.copy_pages, self.pages.len()) };
            match targets {
                Ok(t) => self.copy_selection_to(&t),
                Err(e) => self.toast(format!("Pages: {e}")),
            }
        }
        ui.add_space(4.0);
        ui.weak("Each copy is grouped on its own sheet. Check them in the overview (O); drag one to move its set, Alt+drag to move one piece.");
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
        let items: Vec<(u64, usize, Rgb, &'static str, String)> = order.iter().map(|a| (a.id, a.page, a.color, a.kind_name(), note_of(a).to_string())).collect();
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last_page = None;
            for (id, page, color, kind, text) in &items {
                if last_page != Some(*page) {
                    last_page = Some(*page);
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(format!("Page {}", page + 1)).strong());
                }
                // Custom row: swatch + note wrapped to the panel width (max 4 lines).
                let note = text.trim();
                let (label, colour) = if note.is_empty() {
                    (kind.to_string(), ui.visuals().weak_text_color())
                } else {
                    (note.replace('\n', " "), ui.visuals().text_color())
                };
                let width = ui.available_width();
                let mut job = egui::text::LayoutJob::simple(label, FontId::proportional(13.0), colour, (width - 26.0).max(40.0));
                job.wrap.max_rows = 4;
                job.wrap.overflow_character = Some('…');
                let galley = ui.painter().layout_job(job);
                let (rect, resp) = ui.allocate_exact_size(vec2(width, galley.size().y + 6.0), Sense::click());
                let selected = self.selected.contains(id);
                if selected || resp.hovered() {
                    let bg = if selected { ui.visuals().selection.bg_fill } else { ui.visuals().widgets.hovered.weak_bg_fill };
                    ui.painter().rect_filled(rect, 3.0, bg);
                }
                if resp.has_focus() {
                    ui.painter().rect_stroke(rect, 3.0, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
                }
                let sw = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(9.0, 9.0));
                ui.painter().rect_filled(sw, 2.0, Color32::from_rgb(color[0], color[1], color[2]));
                ui.painter().galley(rect.min + vec2(22.0, 3.0), galley, colour);
                let resp = if note.is_empty() { resp } else { resp.on_hover_text(note) };
                if resp.clicked() {
                    clicked = Some(*id);
                }
            }
            if items.is_empty() {
                ui.weak("No markups yet.");
            }
        });
        if let Some(id) = clicked {
            if ui.input(|i| i.modifiers.command || i.modifiers.shift) {
                self.commit_edit();
                self.set_tool(Tool::Select);
                self.toggle_selected(id);
            } else {
                self.focus_annot(id);
            }
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
            csv += &format!("{},{},{},{},{}\n", n + 1, a.page + 1, a.kind_name(), colour, q(note_of(a).trim()));
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
        egui::Window::new(format!("Redline {} — shortcuts", env!("CARGO_PKG_VERSION"))).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("keys").striped(true).show(ui, |ui| {
                let rows = [
                    ("C", "Callout: press on the target, drag to where the note goes (click = quick note)"),
                    ("T", "Text box (click, or drag for width)"),
                    ("K / R / H", "Revision cloud / rectangle / highlight (drag)"),
                    ("A / L / P", "Arrow / line / pen"),
                    ("V or Esc", "Select: drag to move, handles to resize, double-click or Enter to edit text"),
                    ("Ctrl/Shift+click", "Add to or remove from selection (also in the markup list)"),
                    ("Drag on empty page", "Select every markup the box crosses (Ctrl/Shift adds)   ·   Ctrl+A all on page"),
                    ("Esc / Ctrl+Enter", "Finish typing"),
                    ("1–6", "Colour (applies to all selected)"),
                    ("[ ]", "Line width   ·   { } text size"),
                    ("Del", "Delete selection   ·   Ctrl+D duplicate"),
                    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
                    ("Ctrl+E", "Export flattened PDF next to original (Ctrl+Shift+E: Save As)"),
                    ("Ctrl+Alt+E", "Export editable PDF (markups as movable comments for Bluebeam/Acrobat)"),
                    ("S / O / M / I", "Page thumbnails / overview of all pages / markup list / stamps"),
                    ("Ctrl+G / Ctrl+Shift+G", "Group / ungroup the selection   ·   Alt+click one piece of a group"),
                    ("In overview", "Arrows move, Enter opens, Ctrl+wheel resizes, O or Esc closes"),
                    ("Wheel / Ctrl+wheel", "Scroll / zoom at cursor"),
                    ("Middle or right drag, Space+drag", "Pan"),
                    ("W / F / G", "Fit width / fit page / go to page"),
                    ("PgUp PgDn ← →  Home End", "Previous / next page"),
                    ("Arrows with a selection", "Nudge it 1 pixel at the current zoom (Shift: 10)"),
                ];
                for (k, d) in rows {
                    ui.strong(k);
                    ui.label(d);
                    ui.end_row();
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Your name on editable-export comments:");
                let hint = default_author();
                if ui.add(egui::TextEdit::singleline(&mut self.prefs.author).hint_text(hint).desired_width(180.0)).changed() {
                    self.prefs_dirty = true;
                }
            });
            ui.weak(format!("Markups autosave to {} as you go; the original PDF is never modified.", sidecar::dir().display()));
            ui.weak(format!("Redline {}", env!("CARGO_PKG_VERSION")));
        });
        if !open {
            self.show_help = false;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.poll_worker(&ctx);
        self.poll_dialog();

        if let Some(p) = ctx.input(|i| i.raw.dropped_files.iter().next().map(|f| f.path().to_path_buf())) {
            self.open(p);
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, now));
        let has_doc = !self.pages.is_empty();
        if has_doc && self.show_sidebar && !self.overview {
            egui::Panel::left("thumbs").resizable(true).default_size(170.0).size_range(90.0..=420.0).show(ui, |ui| self.sidebar(ui));
        }
        self.load_textures(&ctx);
        if self.show_stamps {
            egui::Panel::right("stamps").resizable(true).default_size(280.0).size_range(220.0..=600.0).show(ui, |ui| self.stamp_panel(ui));
        } else if self.show_markups {
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
        self.show_dialog(&ctx, frame);

        let shown = (self.show_sidebar, self.show_markups, self.show_stamps);
        if shown != (self.prefs.show_sidebar, self.prefs.show_markups, self.prefs.show_stamps) || self.prefs_dirty {
            self.prefs.show_sidebar = self.show_sidebar;
            self.prefs.show_markups = self.show_markups;
            self.prefs.show_stamps = self.show_stamps;
            self.prefs_dirty = false;
            sidecar::save_prefs(&self.prefs);
        }
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
fn paint_prims(painter: &Painter, origin: Pos2, z: f32, prims: &[Prim], k: (f32, f32), alpha_mul: f32, images: &HashMap<String, TextureHandle>) {
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
            Prim::Image { rect, image } => {
                let r = Rect::from_min_max(to(rect.min), to(rect.max));
                if let Some(t) = images.get(image) {
                    painter.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(alpha_mul));
                } else {
                    // Image file missing: a crossed box, so the gap can't be missed.
                    let s = Stroke::new(1.0, Color32::from_rgb(200, 0, 0));
                    painter.rect_stroke(r, 0.0, s, egui::StrokeKind::Inside);
                    painter.line_segment([r.left_top(), r.right_bottom()], s);
                    painter.line_segment([r.right_top(), r.left_bottom()], s);
                }
            }
        }
    }
}

/// Default name on editable-export comments when none is set in Shortcuts:
/// $REDLINE_AUTHOR, else the account's full name (Linux /etc/passwd), else the login name.
fn default_export(src: &std::path::Path, mode: ExportMode) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let suffix = if mode == ExportMode::Editable { "_markup_editable" } else { "_markup" };
    src.with_file_name(format!("{stem}{suffix}.pdf"))
}

fn default_author() -> String {
    if let Ok(a) = std::env::var("REDLINE_AUTHOR") {
        return a;
    }
    #[cfg(windows)]
    {
        std::env::var("USERNAME").unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        let user = std::env::var("USER").unwrap_or_default();
        std::fs::read_to_string("/etc/passwd")
            .ok()
            .and_then(|p| {
                p.lines().find(|l| l.split(':').next() == Some(user.as_str())).and_then(|l| l.split(':').nth(4)).map(|g| g.split(',').next().unwrap_or("").to_string())
            })
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(user)
    }
}

/// The note text, or a stamp's name: what the markup list and CSV show.
fn note_of(a: &Annot) -> &str {
    match &a.kind {
        Kind::Image { name, .. } => name,
        _ => a.text(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One 600×800 page drawn at 100% with its top-left at the canvas origin,
    /// so screen points equal page points.
    fn app() -> App {
        let mut app = App::with(None, String::new(), sidecar::Prefs::default());
        app.pages = vec![vec2(600.0, 800.0)];
        app.page_y = vec![0.0];
        app.doc_w = 600.0;
        app.canvas = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 1000.0));
        app.set_tool(Tool::Select);
        app
    }

    fn rect(app: &mut App, x: f32, y: f32) -> u64 {
        app.add(0, Kind::Rect { rect: Rect::from_min_size(pos2(x, y), vec2(100.0, 100.0)) })
    }

    const PLAIN: Mods = Mods { add: false, single: false };
    const ADD: Mods = Mods { add: true, single: false };
    const ALT: Mods = Mods { add: false, single: true };

    fn drag(app: &mut App, from: Pos2, to: Pos2, mods: Mods) {
        app.begin_drag(from, false, mods);
        app.update_drag(to, to - from);
        app.end_drag();
    }

    #[test]
    fn click_and_modifier_click_select() {
        let mut app = app();
        let (a, b) = (rect(&mut app, 100.0, 100.0), rect(&mut app, 300.0, 300.0));
        app.click(pos2(100.0, 150.0), false, PLAIN);
        assert_eq!(app.selected, vec![a]);
        app.click(pos2(300.0, 350.0), false, ADD);
        assert_eq!(app.selected, vec![a, b]);
        app.click(pos2(300.0, 350.0), false, ADD);
        assert_eq!(app.selected, vec![a], "modifier-click toggles off");
        app.click(pos2(300.0, 350.0), false, PLAIN);
        assert_eq!(app.selected, vec![b], "plain click replaces");
        app.click(pos2(550.0, 50.0), false, PLAIN);
        assert!(app.selected.is_empty(), "click on empty page clears");
    }

    #[test]
    fn marquee_selects_and_group_moves() {
        let mut app = app();
        let (a, b) = (rect(&mut app, 100.0, 100.0), rect(&mut app, 300.0, 300.0));
        let c = rect(&mut app, 450.0, 600.0);
        drag(&mut app, pos2(50.0, 50.0), pos2(420.0, 420.0), PLAIN);
        assert_eq!(app.selected, vec![a, b]);
        drag(&mut app, pos2(440.0, 580.0), pos2(470.0, 620.0), ADD);
        assert_eq!(app.selected, vec![a, b, c], "modifier marquee adds");

        app.selected = vec![a, b];
        let before = app.offset;
        drag(&mut app, pos2(100.0, 150.0), pos2(110.0, 170.0), PLAIN);
        assert_eq!(app.offset, before, "no pan");
        assert_eq!(app.find(a).unwrap().bounds().min, pos2(108.0, 118.0));
        assert_eq!(app.find(b).unwrap().bounds().min, pos2(308.0, 318.0));
        assert_eq!(app.find(c).unwrap().bounds().min, pos2(448.0, 598.0), "unselected stays");

        app.set_color(2);
        assert!(app.annots.iter().filter(|x| x.id != c).all(|x| x.color == COLORS[2].0));
        app.delete_selected();
        assert_eq!(app.annots.iter().map(|x| x.id).collect::<Vec<_>>(), vec![c]);
        app.undo();
        assert_eq!(app.annots.len(), 3, "delete is one undo step");
    }

    fn seal(app: &mut App, page: usize, x: f32, y: f32) -> u64 {
        app.add(page, Kind::Image { rect: Rect::from_min_size(pos2(x, y), vec2(100.0, 100.0)), image: "ab.png".into(), name: "Seal".into() })
    }

    #[test]
    fn groups_select_and_move_together_alt_picks_one() {
        let mut app = app();
        let (a, b, c) = (rect(&mut app, 100.0, 100.0), rect(&mut app, 300.0, 300.0), rect(&mut app, 450.0, 600.0));
        app.selected = vec![a, b];
        app.group_selected();
        app.selected.clear();
        app.click(pos2(100.0, 150.0), false, PLAIN);
        assert_eq!(app.selected, vec![a, b], "click selects the whole group");
        app.click(pos2(100.0, 150.0), false, ALT);
        assert_eq!(app.selected, vec![a], "Alt+click picks one piece");
        app.click(pos2(450.0, 650.0), false, PLAIN);
        drag(&mut app, pos2(300.0, 350.0), pos2(310.0, 360.0), PLAIN);
        assert_eq!(app.selected, vec![a, b]);
        assert_eq!(app.find(a).unwrap().bounds().min, pos2(108.0, 108.0), "moved with its group");
        assert_eq!(app.find(c).unwrap().bounds().min, pos2(448.0, 598.0));
        drag(&mut app, pos2(310.0, 360.0), pos2(320.0, 360.0), ALT);
        assert_eq!(app.find(b).unwrap().bounds().min, pos2(318.0, 308.0), "Alt-drag moves one piece");
        assert_eq!(app.find(a).unwrap().bounds().min, pos2(108.0, 108.0));
        app.selected = vec![a, b];
        app.ungroup_selected();
        app.click(pos2(108.0, 150.0), false, PLAIN);
        assert_eq!(app.selected, vec![a]);
    }

    #[test]
    fn nudges_move_selection_and_coalesce_undo() {
        let mut app = app();
        let (a, b) = (rect(&mut app, 100.0, 100.0), rect(&mut app, 300.0, 300.0));
        app.zoom = 2.0;
        app.selected = vec![a];
        app.nudge(vec2(0.5, 0.0), 10.0);
        app.nudge(vec2(0.5, 0.0), 10.4);
        app.nudge(vec2(0.0, -5.0), 10.8);
        assert_eq!(app.find(a).unwrap().bounds().min, pos2(99.0, 93.0));
        assert_eq!(app.find(b).unwrap().bounds().min, pos2(298.0, 298.0), "unselected stays");
        app.selected = vec![b];
        app.nudge(vec2(1.0, 0.0), 11.0);
        app.undo();
        assert_eq!(app.find(b).unwrap().bounds().min, pos2(298.0, 298.0), "other selection: its own step");
        app.undo();
        assert_eq!(app.find(a).unwrap().bounds().min, pos2(98.0, 98.0), "a held key is one step");
        app.selected = vec![a];
        app.nudge(vec2(1.0, 0.0), 20.0);
        app.nudge(vec2(1.0, 0.0), 21.5);
        app.undo();
        assert_eq!(app.find(a).unwrap().bounds().min, pos2(99.0, 98.0), "a pause starts a new step");
    }

    #[test]
    fn place_image_clicked_or_dragged_then_select() {
        let mut app = app();
        app.placing = Some(Place::Image { image: "ab.png".into(), name: "Seal".into(), aspect: 2.0, width: 120.0 });
        app.click(pos2(300.0, 300.0), false, PLAIN);
        let a = app.annots.last().unwrap();
        assert!(matches!(&a.kind, Kind::Image { rect, .. } if *rect == Rect::from_center_size(pos2(300.0, 300.0), vec2(120.0, 60.0))));
        assert_eq!(app.selected, vec![a.id]);
        assert!(app.placing.is_none(), "one-shot");

        app.placing = Some(Place::Image { image: "ab.png".into(), name: "Seal".into(), aspect: 2.0, width: 120.0 });
        drag(&mut app, pos2(100.0, 100.0), pos2(180.0, 110.0), PLAIN);
        assert!(matches!(&app.annots.last().unwrap().kind, Kind::Image { rect, .. } if *rect == Rect::from_min_size(pos2(100.0, 100.0), vec2(80.0, 40.0))));
        app.undo();
        assert_eq!(app.annots.len(), 1);

        app.date_text = "10/29/2026".into();
        app.placing = Some(Place::Date);
        app.click(pos2(200.0, 500.0), false, PLAIN);
        let d = app.annots.last().unwrap();
        assert_eq!((d.text(), d.color), ("10/29/2026", DATE_COLOR));
        assert!((d.bounds().center() - pos2(200.0, 500.0)).length() < 0.5, "centred on the click");
    }

    #[test]
    fn copy_to_pages_keeps_corner_offset_groups_per_page_and_skips_stamped() {
        let mut app = app();
        app.pages = vec![vec2(600.0, 800.0), vec2(600.0, 800.0), vec2(500.0, 700.0), vec2(600.0, 800.0)];
        app.page_y = vec![0.0, 812.0, 1624.0, 2336.0];
        let s = seal(&mut app, 0, 450.0, 650.0);
        let d = app.add(0, Kind::Text { pos: pos2(450.0, 760.0), box_w: 80.0, text: "10/29/2026".into(), size: 12.0 });
        seal(&mut app, 3, 10.0, 10.0);
        app.selected = vec![s, d];
        app.copy_selection_to(&[0, 1, 2, 3]);
        let on = |p: usize| app.annots.iter().filter(|a| a.page == p).collect::<Vec<_>>();
        assert_eq!(on(3).len(), 1, "page 4 already had the seal");
        let (p1, p2) = (on(1), on(2));
        assert_eq!(p1.len(), 2);
        assert_eq!(p1[0].bounds().min, pos2(450.0, 650.0), "same size: same place");
        assert_eq!(p2[0].bounds().min, pos2(350.0, 550.0), "smaller sheet: same distance from bottom-right");
        let groups: HashSet<_> = app.annots.iter().filter(|a| a.page < 3).map(|a| a.group.unwrap()).collect();
        assert_eq!(groups.len(), 3, "a group per page, originals included");
        assert!(p1.iter().all(|a| a.group == p1[0].group));
        app.undo();
        assert_eq!(app.annots.len(), 3, "one undo step");
        assert!(app.annots.iter().all(|a| a.group.is_none()));
    }
}
