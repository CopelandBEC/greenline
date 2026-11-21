use crate::annot::{self, Annot, Fill, Kind, Prim, Rgb, Style, COLORS, WHITE};
use crate::patterns::{self, Hatch, HatchFill, LineType};
use std::sync::Arc;
use crate::measure::{self, Scale, Units};
use crate::proposals::{self, Proposals};
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
/// Outline round agents' proposed markups until they're accepted.
const PROPOSED: Color32 = Color32::from_rgb(235, 120, 0);
/// Search matches.
const FOUND: Color32 = Color32::from_rgb(255, 190, 0);
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
    /// Bluebeam line type / hatch files to import.
    Patterns,
    /// Where to save a new blank document: page size (points) and count.
    NewDoc(Vec2, usize),
    /// A photo or sketch to place as a markup.
    Picture,
    /// The shared pattern library folder.
    PatternDir,
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
    /// Click out the corners; double-click or Enter to finish (or drag one segment).
    Polyline,
    /// Click out the corners; close on the first or double-click (or drag a rectangle).
    Polygon,
    /// Length measurement: click its points, double-click or Enter to finish (or drag one segment).
    Length,
    /// Area measurement: click its corners, close on the first or double-click (or drag a rectangle).
    Area,
    /// Draw a line along a known dimension to calibrate the scale (from the scale window).
    Calibrate,
    /// Drag a box round a detail that has its own scale (from the scale window).
    ScaleRegion,
}

/// Polygon shares N with Polyline, taken with Shift.
const TOOLS: [(Tool, &str, Key); 13] = [
    (Tool::Select, "Select", Key::V),
    (Tool::Callout, "Callout", Key::C),
    (Tool::Text, "Text", Key::T),
    (Tool::Cloud, "Cloud", Key::K),
    (Tool::Rect, "Rect", Key::R),
    (Tool::Highlight, "Highlight", Key::H),
    (Tool::Arrow, "Arrow", Key::A),
    (Tool::Line, "Line", Key::L),
    (Tool::Pen, "Pen", Key::P),
    (Tool::Polyline, "Polyline", Key::N),
    (Tool::Polygon, "Polygon", Key::N),
    (Tool::Length, "Length", Key::D),
    (Tool::Area, "Area", Key::E),
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
    /// Tip of a callout's extra leader, by index in its `leaders`.
    Leader(usize),
    /// A measurement's point, by index.
    Vertex(usize),
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
    /// Ctrl (Cmd) without Shift: with one callout selected, clicking empty
    /// space adds a leader there.
    ctrl: bool,
    /// Shift: square a measurement's next point to the last one.
    shift: bool,
}

/// A stored image (stored name, file name, pixels) or why not, where it was dropped, and the placing epoch it began in.
type PictureDone = (Result<(String, String, Vec2), String>, Option<(usize, Pos2)>, u64);

/// A document in a background tab: everything that belongs to it, parked while
/// another tab is shown. Its rendered pages are dropped (re-rendered on return);
/// the worker keeps the PDF open under `generation`.
struct Parked {
    path: Option<PathBuf>,
    generation: u64,
    pages: Vec<Vec2>,
    page_y: Vec<f32>,
    doc_w: f32,
    doc_h: f32,
    zoom: f32,
    offset: Vec2,
    props: Proposals,
    props_path: Option<PathBuf>,
    markups_file: Option<PathBuf>,
    accepted: Vec<String>,
    props_mtime: Option<std::time::SystemTime>,
    overview_cursor: usize,
    annots: Vec<Annot>,
    scales: Vec<Scale>,
    scale_found: HashMap<usize, Vec<(String, f64)>>,
    scale_asked: HashSet<usize>,
    find: Find,
    find_from: usize,
    next_id: u64,
    undo: Vec<(Vec<Annot>, Vec<Scale>)>,
    redo: Vec<(Vec<Annot>, Vec<Scale>)>,
    dirty: bool,
    selected: Vec<u64>,
    lock: Option<std::fs::File>,
    read_only: bool,
    read_only_why: String,
    failed: HashSet<usize>,
    /// Loaded while in the background: fit it when first shown.
    fit_pending: bool,
}

/// An open document's place in the tab strip; the shown tab's state is in `App`.
struct Tab {
    parked: Option<Box<Parked>>,
}

/// Where each page sits in the scroll of pages, the widest page, and the total height.
fn layout(pages: &[Vec2]) -> (Vec<f32>, f32, f32) {
    let mut y = GAP;
    let mut ys = Vec::with_capacity(pages.len());
    let mut w = 0.0f32;
    for s in pages {
        ys.push(y);
        y += s.y + GAP;
        w = w.max(s.x);
    }
    (ys, w, y)
}

/// Page sizes offered for a new document: name and portrait size in points.
const PAGE_SIZES: [(&str, Vec2); 9] = [
    ("Letter  8.5 × 11 in", Vec2::new(612.0, 792.0)),
    ("Legal  8.5 × 14 in", Vec2::new(612.0, 1008.0)),
    ("Tabloid  11 × 17 in", Vec2::new(792.0, 1224.0)),
    ("ARCH C  18 × 24 in", Vec2::new(1296.0, 1728.0)),
    ("ARCH D  24 × 36 in", Vec2::new(1728.0, 2592.0)),
    ("ARCH E  36 × 48 in", Vec2::new(2592.0, 3456.0)),
    ("A4  210 × 297 mm", Vec2::new(595.3, 841.9)),
    ("A3  297 × 420 mm", Vec2::new(841.9, 1190.6)),
    ("A1  594 × 841 mm", Vec2::new(1683.8, 2383.9)),
];

/// The New document window's choices.
struct NewDoc {
    size: usize,
    landscape: bool,
    pages: usize,
}

impl Default for NewDoc {
    fn default() -> Self {
        Self { size: 0, landscape: false, pages: 1 }
    }
}

/// The find bar (Ctrl+F) and its results.
#[derive(Default)]
struct Find {
    open: bool,
    query: String,
    /// Search the results belong to; bumped on every new query.
    id: u64,
    /// Matches in page order: page and a box per line of the match.
    hits: Vec<(usize, Vec<Rect>)>,
    /// Pages still to report for the current search.
    pending: usize,
    /// A page had more matches than were reported.
    more: bool,
    /// Index into `hits` of the match shown.
    cur: Option<usize>,
    focus: bool,
}

/// The scale window (U) and what it is in the middle of.
#[derive(Default)]
struct ScaleWin {
    open: bool,
    /// Region the next scale is for, drawn with `Tool::ScaleRegion`; otherwise the whole page.
    target: Option<(usize, Rect)>,
    /// Calibration line awaiting its real length.
    calib: Option<(usize, Pos2, Pos2)>,
    calib_text: String,
    /// Scales written on each page, from its text.
    found: HashMap<usize, Vec<(String, f64)>>,
    asked: HashSet<usize>,
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
    /// Review panel (Y): agents' proposals. Takes the right side over from markups/stamps.
    show_review: bool,
    /// Proposals for this PDF as last read, and the file's time then.
    props: Proposals,
    /// The proposals file for this PDF (beside its markups file).
    props_path: Option<PathBuf>,
    /// This PDF's markups file.
    markups_file: Option<PathBuf>,
    /// Proposed markups already accepted, saved with the markups (`Sidecar::accepted`).
    accepted: Vec<String>,
    props_mtime: Option<std::time::SystemTime>,
    props_checked: f64,
    /// Proposal row under the pointer: batch id and markup id (outlined on the sheet).
    props_hover: Option<(String, u64)>,
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
    /// Drawing scales on this document's pages.
    scales: Vec<Scale>,
    /// Measurement (or calibration line) being clicked out: its page and points so far.
    building: Option<(usize, Vec<Pos2>)>,
    scale_win: ScaleWin,
    find: Find,
    /// The New document window, while open.
    new_doc: Option<NewDoc>,
    /// A blank PDF being written: its path and the markups lock held for it.
    creating: Option<(PathBuf, std::fs::File)>,
    /// Images being stored for placing: (stored name, file name, pixels) or an error, the drop target, the document then.
    pictures: (mpsc::Sender<PictureDone>, mpsc::Receiver<PictureDone>),
    /// Images waiting to be placed after the one being placed now.
    queued_pictures: std::collections::VecDeque<Place>,
    /// Bumped when placing is cancelled, so images still being stored are let go.
    picture_epoch: u64,
    /// Open documents in tab order; `tabs[cur_tab]` is the one shown (its state is in `self`).
    tabs: Vec<Tab>,
    cur_tab: usize,
    /// The window title needs updating (tab switched or closed).
    title_dirty: bool,
    /// The latest document generation handed out.
    last_generation: u64,
    /// When to next try saving background tabs whose save failed.
    parked_retry_at: f64,
    /// Scroll the tab strip to the shown tab on the next frame.
    tab_scroll: bool,
    /// Page in view when the search started: the first match shown is at or after it.
    find_from: usize,
    /// Shift held over the canvas (squares a measurement's next point).
    shift: bool,
    next_id: u64,
    /// Markups and scales before each change (scale changes relabel measurements, so they undo with them).
    undo: Vec<(Vec<Annot>, Vec<Scale>)>,
    redo: Vec<(Vec<Annot>, Vec<Scale>)>,
    dirty: bool,

    tool: Tool,
    tool_color: HashMap<Tool, usize>,
    /// Fill chosen per tool: a colour index (`COLORS`, then white) or none.
    /// Absent: the markup's own default (callouts white, areas a tint, others none).
    tool_fill: HashMap<Tool, Option<usize>>,
    /// Opacity of new fills, 0..=255.
    fill_alpha: u8,
    /// The opacity slider's value during a drag (percent), applied on release.
    fill_pending: Option<f32>,
    /// The hatch scale being dragged, applied on release.
    hatch_scale_pending: Option<f32>,
    /// Line type per tool (absent or `None`: solid).
    tool_line: HashMap<Tool, Arc<LineType>>,
    /// Hatch per tool (absent: none).
    tool_hatch: HashMap<Tool, HatchFill>,
    /// Bold, italic, underline for new notes.
    text_format: [bool; 3],
    /// Line types and hatches to pick from.
    patterns: patterns::Library,
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
        crate::theme::install(&cc.egui_ctx, prefs.theme);
        let (worker, status) = match Worker::spawn(cc.egui_ctx.clone()) {
            Ok(w) => (Some(w), String::from("Ctrl+O to open a PDF (or drop one here)")),
            Err(e) => (None, format!("PDFium failed to load: {e}")),
        };
        let mut app = Self::with(worker, status, prefs);
        app.library = stamps::Library::load();
        app.reload_patterns();
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
            show_review: false,
            props: Proposals::default(),
            props_path: None,
            markups_file: None,
            accepted: Vec::new(),
            props_mtime: None,
            props_checked: f64::NEG_INFINITY,
            props_hover: None,
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
            scales: Vec::new(),
            building: None,
            scale_win: ScaleWin::default(),
            find: Find::default(),
            new_doc: None,
            creating: None,
            pictures: mpsc::channel(),
            queued_pictures: Default::default(),
            picture_epoch: 0,
            tabs: vec![Tab { parked: None }],
            cur_tab: 0,
            title_dirty: false,
            last_generation: 0,
            parked_retry_at: 0.0,
            tab_scroll: false,
            find_from: 0,
            shift: false,
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            tool: Tool::Callout,
            tool_color: HashMap::from([(Tool::Highlight, 5)]),
            tool_fill: HashMap::new(),
            fill_alpha: 128,
            fill_pending: None,
            hatch_scale_pending: None,
            tool_line: HashMap::new(),
            tool_hatch: HashMap::new(),
            text_format: [false; 3],
            patterns: patterns::Library::default(),
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

    /// Moves the shown document's state out of `self`, leaving it empty.
    fn park(&mut self) -> Box<Parked> {
        // A move or resize under way already changed the markups: finish it, so it's saved and undoable.
        if matches!(self.drag, Drag::Move { .. } | Drag::Handle { .. }) {
            self.end_drag();
        }
        self.commit_edit();
        self.save_sidecar();
        let p = Parked {
            path: std::mem::take(&mut self.path),
            generation: self.generation,
            pages: std::mem::take(&mut self.pages),
            page_y: std::mem::take(&mut self.page_y),
            doc_w: self.doc_w,
            doc_h: self.doc_h,
            zoom: self.zoom,
            offset: self.offset,
            props: std::mem::take(&mut self.props),
            props_path: std::mem::take(&mut self.props_path),
            markups_file: std::mem::take(&mut self.markups_file),
            accepted: std::mem::take(&mut self.accepted),
            props_mtime: std::mem::take(&mut self.props_mtime),
            overview_cursor: self.overview_cursor,
            annots: std::mem::take(&mut self.annots),
            scales: std::mem::take(&mut self.scales),
            scale_found: std::mem::take(&mut self.scale_win.found),
            scale_asked: std::mem::take(&mut self.scale_win.asked),
            find: std::mem::take(&mut self.find),
            find_from: self.find_from,
            next_id: self.next_id,
            undo: std::mem::take(&mut self.undo),
            redo: std::mem::take(&mut self.redo),
            dirty: self.dirty,
            selected: std::mem::take(&mut self.selected),
            lock: std::mem::take(&mut self.lock),
            read_only: self.read_only,
            read_only_why: std::mem::take(&mut self.read_only_why),
            failed: std::mem::take(&mut self.failed),
            fit_pending: self.fit_pending,
        };
        let mut p = p;
        // Scale suggestions asked for but not back yet are asked again when it's shown.
        p.scale_asked.retain(|pg| p.scale_found.contains_key(pg));
        self.dirty = false;
        self.read_only = false;
        self.next_id = 1;
        self.drop_view_state();
        Box::new(p)
    }

    /// Makes a parked document the shown one.
    fn unpark(&mut self, p: Box<Parked>) {
        let p = *p;
        self.drop_view_state();
        self.path = p.path;
        self.generation = p.generation;
        self.pages = p.pages;
        self.page_y = p.page_y;
        self.doc_w = p.doc_w;
        self.doc_h = p.doc_h;
        self.zoom = p.zoom;
        self.offset = p.offset;
        self.props = p.props;
        self.props_path = p.props_path;
        self.markups_file = p.markups_file;
        self.accepted = p.accepted;
        self.props_mtime = p.props_mtime;
        self.overview_cursor = p.overview_cursor;
        self.annots = p.annots;
        self.scales = p.scales;
        self.scale_win.found = p.scale_found;
        self.scale_win.asked = p.scale_asked;
        self.find = p.find;
        self.find_from = p.find_from;
        self.next_id = p.next_id;
        self.undo = p.undo;
        self.redo = p.redo;
        self.dirty = p.dirty;
        self.selected = p.selected;
        self.lock = p.lock;
        self.read_only = p.read_only;
        self.read_only_why = p.read_only_why;
        self.failed = p.failed;
        self.fit_pending = p.fit_pending;
        // Find results are for this document's search; let the worker know which one is
        // wanted, and pick up a search that was cut short by the switch.
        if let Some(w) = &self.worker {
            w.keep.lock().unwrap().search = self.find.id;
        }
        if self.find.open && self.find.pending > 0 {
            self.start_search();
        }
    }

    /// Clears what's tied to the shown document's view: rendered pages, gestures in progress.
    fn drop_view_state(&mut self) {
        self.base.clear();
        self.patch.clear();
        self.req_base.clear();
        self.req_patch.clear();
        self.thumbs.clear();
        self.req_thumb.clear();
        self.thumbs_wanted.clear();
        self.overview = false;
        self.sidebar_synced = None;
        self.editing = None;
        self.drag = Drag::None;
        self.placing = None;
        // Images on their way to being placed belong to the document they were dropped on.
        self.queued_pictures.clear();
        self.picture_epoch += 1;
        self.building = None;
        self.scale_win.target = None;
        self.scale_win.calib = None;
        self.props_hover = None;
        self.props_checked = f64::NEG_INFINITY;
        self.last_nudge = (0.0, Vec::new());
        self.fit_pending = false;
        self.save_retry_at = 0.0;
        // A toolbar drag (opacity, hatch scale) mustn't land on the next tab's selection.
        self.fill_pending = None;
        self.hatch_scale_pending = None;
    }

    /// Shows tab `i`.
    fn switch_tab(&mut self, i: usize) {
        if i == self.cur_tab || i >= self.tabs.len() {
            return;
        }
        let parked = self.park();
        self.tabs[self.cur_tab].parked = Some(parked);
        self.cur_tab = i;
        if let Some(p) = self.tabs[i].parked.take() {
            self.unpark(p);
        }
        self.set_title();
    }

    /// Closes tab `i`, letting its document go. The shown one is saved first,
    /// and stays open if that fails.
    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if i == self.cur_tab {
            if matches!(self.drag, Drag::Move { .. } | Drag::Handle { .. }) {
                self.end_drag();
            }
            self.commit_edit();
            self.save_sidecar();
            if self.dirty && !self.read_only {
                self.toast("Autosave failed; keeping this tab open so markups aren't lost".into());
                return;
            }
            let p = self.park();
            // Parking saves again: if that failed, keep it.
            if p.dirty && !p.read_only {
                self.unpark(p);
                self.toast("Autosave failed; keeping this tab open so markups aren't lost".into());
                return;
            }
            if let Some(w) = &self.worker {
                w.send(Req::Close { generation: p.generation });
            }
            drop(p);
            self.tabs.remove(i);
            if self.tabs.is_empty() {
                self.tabs.push(Tab { parked: None });
                self.cur_tab = 0;
                self.last_generation += 1;
                self.generation = self.last_generation;
                self.status = String::from("Ctrl+O to open a PDF (or drop one here)");
                self.status_until = f64::INFINITY;
            } else {
                self.cur_tab = i.min(self.tabs.len() - 1);
                if let Some(p) = self.tabs[self.cur_tab].parked.take() {
                    self.unpark(p);
                }
            }
            self.set_title();
        } else {
            // Parked: it was saved when parked; a failed save is retried once more here.
            let tab = self.tabs.remove(i);
            if i < self.cur_tab {
                self.cur_tab -= 1;
            }
            if let (Some(p), Some(w)) = (tab.parked, &self.worker) {
                if p.dirty && !p.read_only {
                    // Bring it back to save it properly, then close it as the shown tab.
                    self.tabs.insert(i, Tab { parked: Some(p) });
                    if i <= self.cur_tab {
                        self.cur_tab += 1;
                    }
                    self.switch_tab(i);
                    self.close_tab(i);
                    return;
                }
                w.send(Req::Close { generation: p.generation });
            }
        }
    }

    /// The window title follows the shown tab (set on the next frame, which has the context).
    fn set_title(&mut self) {
        self.title_dirty = true;
        self.tab_scroll = true;
    }

    /// Background tabs whose save failed when parked: tried again every few seconds,
    /// and the failure kept in the status line until it goes through.
    fn save_parked(&mut self, now: f64) {
        if now < self.parked_retry_at {
            return;
        }
        let mut failed = None;
        for p in self.tabs.iter_mut().filter_map(|t| t.parked.as_mut()).filter(|p| p.dirty && !p.read_only) {
            let (Some(pdf), Some(f)) = (&p.path, &p.markups_file) else {
                p.dirty = false;
                continue;
            };
            match sidecar::save_file(f, pdf, &p.annots, &p.scales, &p.accepted) {
                Ok(()) => p.dirty = false,
                Err(e) => failed = Some(format!("AUTOSAVE FAILED for {}: {e}", pdf.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
            }
        }
        if let Some(msg) = failed {
            self.status = msg;
            self.status_until = f64::INFINITY;
            self.parked_retry_at = now + 5.0;
        }
    }

    /// A tab's label: its file name.
    fn tab_name(&self, i: usize) -> String {
        let path = if i == self.cur_tab { self.path.as_ref() } else { self.tabs[i].parked.as_ref().and_then(|p| p.path.as_ref()) };
        path.and_then(|p| p.file_name()).map_or_else(|| "New tab".into(), |f| f.to_string_lossy().into_owned())
    }

    /// Moves tab `from` to index `to` (counted after it's taken out); the shown tab stays shown.
    fn move_tab(&mut self, from: usize, to: usize) {
        let n = self.tabs.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        let cur = self.cur_tab;
        self.cur_tab = if cur == from {
            to
        } else if from < cur && cur <= to {
            cur - 1
        } else if to <= cur && cur < from {
            cur + 1
        } else {
            cur
        };
        self.tab_scroll = true;
    }

    /// The tab strip, shown once two or more documents are open. Tabs drag to reorder.
    fn tab_strip(&mut self, ui: &mut Ui) {
        let mut switch = None;
        let mut close = None;
        let mut reorder = None;
        egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                // Each tab's extent (label and ×), and the tab being dragged or just dropped.
                let mut spans = Vec::with_capacity(self.tabs.len());
                let mut dragging = None;
                let mut dropped = None;
                for i in 0..self.tabs.len() {
                    let name = self.tab_name(i);
                    let mut short: String = if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.clone() };
                    let unsaved = self.tabs[i].parked.as_ref().is_some_and(|p| p.dirty && !p.read_only);
                    if unsaved {
                        short.insert_str(0, "● ");
                    }
                    let on = i == self.cur_tab;
                    let tip = if unsaved { format!("{name} — not saved yet (retrying)") } else { name.clone() };
                    let r = ui.add(egui::Button::selectable(on, short).sense(Sense::click_and_drag())).on_hover_text(tip);
                    if on && std::mem::take(&mut self.tab_scroll) {
                        r.scroll_to_me(None);
                    }
                    if r.clicked() {
                        switch = Some(i);
                    }
                    if r.middle_clicked() {
                        close = Some(i);
                    }
                    if r.dragged_by(PointerButton::Primary) {
                        dragging = Some(i);
                    }
                    if r.drag_stopped_by(PointerButton::Primary) {
                        dropped = Some(i);
                    }
                    let x = ui.add(egui::Button::new(egui::RichText::new("×").weak()).frame(false)).on_hover_text("Close (Ctrl+W)");
                    if x.clicked() {
                        close = Some(i);
                    }
                    spans.push(r.rect.union(x.rect));
                    ui.add_space(6.0);
                }
                // Where a dragged tab would land: before the first tab whose middle is right of the pointer.
                let Some(from) = dragging.or(dropped) else { return };
                let Some(p) = ui.input(|i| i.pointer.interact_pos()) else { return };
                let slot = spans.iter().filter(|s| s.center().x < p.x).count();
                // Counted with the dragged tab taken out.
                let to = if slot > from { slot - 1 } else { slot };
                if dropped.is_some() {
                    reorder = Some((from, to));
                    return;
                }
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if to != from {
                    let x = match spans.get(slot) {
                        Some(s) => s.left() - 3.0,
                        None => spans[spans.len() - 1].right() + 3.0,
                    };
                    let y = spans[from].y_range();
                    ui.painter().vline(x, y, Stroke::new(2.0, ui.visuals().selection.stroke.color));
                }
                // Near either end of a strip too long to show whole: scroll it along.
                let clip = ui.clip_rect();
                let edge = 24.0;
                let step = if p.x < clip.left() + edge {
                    8.0
                } else if p.x > clip.right() - edge {
                    -8.0
                } else {
                    0.0
                };
                if step != 0.0 {
                    ui.scroll_with_delta(vec2(step, 0.0));
                    ui.ctx().request_repaint();
                }
            });
        });
        if let Some(i) = close {
            self.close_tab(i);
        } else if let Some((from, to)) = reorder {
            self.move_tab(from, to);
        } else if let Some(i) = switch {
            self.switch_tab(i);
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.open_with(path, None);
    }

    /// Opens `path`, with its markups lock already held when `lock` is given.
    fn open_with(&mut self, path: PathBuf, lock: Option<std::fs::File>) {
        if self.worker.is_none() {
            return;
        }
        let path = dunce::canonicalize(&path).unwrap_or(path);
        // Already open: show its tab.
        if self.path.as_ref() == Some(&path) {
            return;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.parked.as_ref().is_some_and(|p| p.path.as_ref() == Some(&path))) {
            self.switch_tab(i);
            return;
        }
        self.tab_scroll = true;
        // Open beside what's shown, in a new tab.
        if self.path.is_some() {
            let parked = self.park();
            self.tabs[self.cur_tab].parked = Some(parked);
            self.tabs.insert(self.cur_tab + 1, Tab { parked: None });
            self.cur_tab += 1;
        }
        self.commit_edit();
        self.save_sidecar();
        if self.dirty && !self.read_only {
            self.toast("Autosave failed; not switching documents so markups aren't lost".into());
            return;
        }
        let w = self.worker.as_ref().unwrap();
        // Unique across tabs: the worker keeps each tab's document under its generation.
        self.last_generation += 1;
        self.generation = self.last_generation;
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
        self.queued_pictures.clear();
        self.picture_epoch += 1;
        self.annots = Vec::new();
        self.scales = Vec::new();
        self.accepted = Vec::new();
        self.building = None;
        self.find.hits.clear();
        self.find.cur = None;
        self.find.pending = 0;
        self.scale_win = ScaleWin { open: self.scale_win.open, ..Default::default() };
        self.next_id = 1;
        self.lock = None;
        self.read_only = false;
        let mut why = None;
        match lock.map_or_else(|| sidecar::lock(&path), |l| Ok(Some(l))) {
            Ok(Some(l)) => self.lock = Some(l),
            Ok(None) => why = Some("open in another Greenline window".to_string()),
            Err(e) => why = Some(format!("could not lock markups file ({e})")),
        }
        match sidecar::load(&path) {
            Ok(Some(s)) => {
                self.next_id = first_free_id(&s.annots, &s.accepted);
                self.annots = s.annots;
                self.scales = s.scales;
                self.accepted = s.accepted;
            }
            Ok(None) => {}
            // Never autosave over a markups file we couldn't read.
            Err(e) => why = Some(format!("saved markups unreadable ({e})")),
        }
        self.read_only = why.is_some();
        self.read_only_why = why.unwrap_or_default();
        self.props = Proposals::default();
        self.props_path = Some(proposals::file_for(&path));
        self.markups_file = Some(sidecar::file_for(&path));
        self.props_mtime = None;
        self.props_checked = f64::NEG_INFINITY;
        self.props_hover = None;
        self.path = Some(path);
        self.status = "Opening…".into();
    }

    fn on_opened(&mut self, ctx: &egui::Context, pages: Vec<Vec2>) {
        (self.page_y, self.doc_w, self.doc_h) = layout(&pages);
        self.pages = pages;
        self.fit_pending = true;
        if self.find.open {
            self.start_search();
        }
        let name = self.file_name();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — Greenline")));
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
        // Nor a PDF open in another tab.
        let open_elsewhere = self.tabs.iter().filter_map(|t| t.parked.as_ref()).filter_map(|p| p.path.as_ref()).any(|p| *p == real_out || dunce::canonicalize(&out).is_ok_and(|o| o == *p));
        if open_elsewhere {
            self.toast("That PDF is open in another tab: export to another name".into());
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
            Pick::Patterns => d.add_filter("Bluebeam line types and hatches", &["blx", "bhx"]),
            Pick::Picture => d.add_filter("PNG or JPEG image", &["png", "jpg", "jpeg"]),
            Pick::NewDoc(..) => {
                let d = d.add_filter("PDF", &["pdf"]).set_file_name("Untitled.pdf");
                match self.path.as_ref().and_then(|p| p.parent()) {
                    Some(dir) => d.set_directory(dir),
                    None => d,
                }
            }
            Pick::PatternDir => d,
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let paths = match pick {
                Pick::Open => d.pick_file().into_iter().collect(),
                Pick::Export(..) => d.save_file().into_iter().collect(),
                Pick::Images(_) | Pick::Patterns => d.pick_files().unwrap_or_default(),
                Pick::Picture => d.pick_file().into_iter().collect(),
                Pick::NewDoc(..) => d.save_file().into_iter().collect(),
                Pick::PatternDir => d.pick_folder().into_iter().collect(),
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
            Pick::Patterns => {
                if !paths.is_empty() {
                    match patterns::import(&paths) {
                        Ok(n) => {
                            self.reload_patterns();
                            self.toast(format!("Imported {n} line types and hatches into your library"));
                        }
                        Err(e) => self.toast(format!("Import failed: {e}")),
                    }
                }
            }
            Pick::NewDoc(size, pages) => {
                if let Some(chosen) = paths.into_iter().next() {
                    // The dialog may leave off the extension; a name it didn't confirm mustn't replace a file.
                    let has_ext = chosen.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
                    let path = if has_ext { chosen } else { chosen.with_extension("pdf") };
                    let real = path.parent().and_then(|d| dunce::canonicalize(d).ok()).zip(path.file_name()).map(|(d, f)| d.join(f)).unwrap_or_else(|| path.clone());
                    if self.path.as_ref().is_some_and(|p| *p == real || dunce::canonicalize(&path).is_ok_and(|o| o == *p)) {
                        self.toast("Refusing to overwrite the open PDF: choose another name".into());
                    } else if path.exists() || sidecar::file_for(&real).exists() || proposals::file_for(&real).exists() {
                        // New makes a new file: never one over a document (or its saved markups).
                        self.toast(format!("{} already exists or has saved markups: New needs a new name", path.display()));
                    } else {
                        self.create_blank(path, real, size, pages);
                    }
                }
            }
            Pick::Picture => {
                if let Some(p) = paths.into_iter().next() {
                    self.place_picture(&p, None);
                }
            }
            Pick::PatternDir => {
                if let Some(dir) = paths.into_iter().next() {
                    self.prefs.pattern_dir = Some(dir);
                    self.prefs_dirty = true;
                    self.reload_patterns();
                }
            }
        }
    }

    /// Stores a photo or sketch and places it: at `at` (screen) when dropped on
    /// a sheet, otherwise with the next click (or drag to size).
    /// Writes a new blank PDF at `path` (`real`: its canonical form; nothing is
    /// there yet). The path's markups lock is taken first and held until it opens.
    fn create_blank(&mut self, path: PathBuf, real: PathBuf, size: Vec2, pages: usize) {
        if self.creating.is_some() || self.worker.is_none() {
            return;
        }
        let lock = match sidecar::lock(&real) {
            Ok(Some(l)) => l,
            Ok(None) => return self.toast("That path is open in another Greenline window: choose another name".into()),
            Err(e) => return self.toast(format!("Could not lock that path's markups: {e}")),
        };
        self.creating = Some((real, lock));
        if let Some(w) = &self.worker {
            w.send(Req::Create { path, size, pages });
        }
    }

    /// The image is decoded and stored off the UI thread (a big photo takes a moment).
    fn place_picture(&mut self, src: &std::path::Path, at: Option<Pos2>) {
        // Where it was dropped, fixed now: the view may move before it's ready.
        let target = at.filter(|_| !self.overview).and_then(|s| self.page_at(s).map(|pg| (pg, self.screen_to_page(pg, s))));
        // Placing starts in Select now; changing tool or Esc before it's ready cancels it.
        if self.tool != Tool::Select {
            self.set_tool(Tool::Select);
        }
        let (tx, src) = (self.pictures.0.clone(), src.to_path_buf());
        let epoch = self.picture_epoch;
        std::thread::spawn(move || {
            tx.send((stamps::store_picture(&src).map_err(|e| e.to_string()), target, epoch)).ok();
        });
        self.toast("Adding the image…".into());
    }

    /// An image stored by `place_picture`: placed where it was dropped, or
    /// ready to place with the next click.
    fn picture_ready(&mut self, stored: Result<(String, String, Vec2), String>, target: Option<(usize, Pos2)>, epoch: u64) {
        // Cancelled since (Esc, another tool, another document): let it go.
        if epoch != self.picture_epoch || self.pages.is_empty() {
            return;
        }
        let (image, name, px) = match stored {
            Ok(r) => r,
            Err(e) => return self.toast(format!("Could not add the image: {e}")),
        };
        let aspect = px.x / px.y.max(1.0);
        // As if at 150 dpi, but within 60% of the page either way.
        let page = target.map_or(self.current_page(), |t| t.0).min(self.pages.len() - 1);
        let ps = self.pages[page];
        let width = (px.x / 150.0 * 72.0).max(24.0).min(ps.x * 0.6).min(ps.y * 0.6 * aspect);
        let place = Place::Image { image, name, aspect, width };
        // Several without a spot (dropped on the overview): placed one after another.
        if target.is_none() && self.placing.is_some() {
            self.queued_pictures.push_back(place);
            return;
        }
        self.start_placing(place);
        if let Some((page, p)) = target {
            // Centred on the drop, kept on the sheet.
            let half = vec2(width, width / aspect) / 2.0;
            let p = pos2(p.x.clamp(half.x, (ps.x - half.x).max(half.x)), p.y.clamp(half.y, (ps.y - half.y).max(half.y)));
            self.place(page, p, p);
        }
    }

    /// Rereads the pattern library (built-ins, own, shared folder).
    fn reload_patterns(&mut self) {
        self.patterns = patterns::Library::load(self.prefs.pattern_dir.as_deref());
        let n = self.patterns.line_types.len() + self.patterns.hatches.len();
        if let Some(p) = self.patterns.problems.first() {
            let more = self.patterns.problems.len() - 1;
            self.toast(format!("{n} patterns; couldn't read {p}{}", if more > 0 { format!(" (and {more} more)") } else { String::new() }));
        }
    }

    fn save_sidecar(&mut self) {
        if !self.dirty || self.read_only {
            return;
        }
        let (Some(p), Some(f)) = (&self.path, &self.markups_file) else {
            self.dirty = false;
            return;
        };
        match sidecar::save_file(f, p, &self.annots, &self.scales, &self.accepted) {
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
                // A document that finished loading after its tab went to the background.
                Resp::Opened { generation, pages } => {
                    if let Some(p) = self.tabs.iter_mut().filter_map(|t| t.parked.as_mut()).find(|p| p.generation == generation) {
                        (p.page_y, p.doc_w, p.doc_h) = layout(&pages);
                        p.pages = pages;
                        p.fit_pending = true;
                    }
                }
                Resp::OpenFailed { generation, error } if generation != self.generation => {
                    if let Some(p) = self.tabs.iter_mut().filter_map(|t| t.parked.as_mut()).find(|p| p.generation == generation) {
                        let name = p.path.as_ref().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                        p.path = None;
                        self.toast(format!("Could not open {name}: {error}"));
                    }
                }
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
                Resp::Created(Ok(path)) => {
                    let lock = self.creating.take().map(|c| c.1);
                    self.open_with(path, lock);
                }
                Resp::Created(Err(e)) => {
                    self.creating = None;
                    self.toast(format!("Could not create the document: {e}"));
                }
                Resp::Found { generation, id, page, hits, more } if generation == self.generation && id == self.find.id => {
                    self.find.more |= more;
                    self.on_found(page, hits);
                }
                Resp::PageText { generation, page, text } if generation == self.generation => {
                    self.scale_win.found.insert(page, measure::find_scales(&text));
                }
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
        self.set_zoom(self.zoom * factor, screen);
    }

    /// Sets the zoom, keeping the document point under `screen` where it is.
    fn set_zoom(&mut self, zoom: f32, screen: Pos2) {
        let before = self.to_doc(screen);
        self.zoom = zoom.clamp(0.02, 40.0);
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

    /// Zooms so `r` (view points on `page`) fills the canvas with a margin, up to 800%.
    fn zoom_to(&mut self, page: usize, r: Rect) {
        let r = r.translate(self.page_doc_rect(page).min.to_vec2());
        let margin = 60.0;
        let z = ((self.canvas.width() - margin) / r.width().max(1.0)).min((self.canvas.height() - margin) / r.height().max(1.0));
        self.zoom = z.clamp(0.02, 8.0);
        self.offset = r.center().to_vec2() - self.canvas.size() / (2.0 * self.zoom);
    }

    /// Fits the view to the selected markups (on the lead's page).
    fn zoom_to_selection(&mut self) {
        let Some(page) = self.lead().map(|a| a.page) else {
            self.toast("Select markups to zoom to (Z)".into());
            return;
        };
        let r = self.annots.iter().filter(|a| a.page == page && self.selected.contains(&a.id)).fold(Rect::NOTHING, |r, a| r.union(a.bounds()));
        self.zoom_to(page, r);
    }

    /// Centres the current match, zooming in only if its text would be too small to read.
    fn show_hit(&mut self) {
        let Some((page, rects)) = self.find.cur.and_then(|i| self.find.hits.get(i)).cloned() else { return };
        let r = rects.iter().fold(Rect::NOTHING, |a, b| a.union(*b));
        let readable = 14.0 / r.height().max(1.0);
        if self.zoom < readable {
            self.zoom = readable.min(8.0);
        }
        let c = self.page_doc_rect(page).min + r.center().to_vec2();
        self.offset = c.to_vec2() - self.canvas.size() / (2.0 * self.zoom);
    }

    /// One page's matches for the current search.
    fn on_found(&mut self, page: usize, hits: Vec<Vec<Rect>>) {
        self.find.pending = self.find.pending.saturating_sub(1);
        let at = self.find.hits.partition_point(|(p, _)| *p < page);
        let n = hits.len();
        self.find.hits.splice(at..at, hits.into_iter().map(|h| (page, h)));
        // Keep the shown match the same one as earlier pages arrive.
        if let Some(c) = &mut self.find.cur {
            if *c >= at {
                *c += n;
            }
        }
        // Show the first match at or after the page in view as soon as it comes in
        // (pages are searched from there on, wrapping round).
        if self.find.cur.is_none() && n > 0 && page >= self.find_from {
            self.find.cur = Some(at);
            self.show_hit();
        }
        if self.find.pending == 0 && self.find.cur.is_none() && !self.find.hits.is_empty() {
            self.find.cur = Some(0);
            self.show_hit();
        }
    }

    fn step_hit(&mut self, dir: i32) {
        let n = self.find.hits.len();
        if n == 0 {
            return;
        }
        self.find.cur = Some(match self.find.cur {
            Some(c) => (c as i32 + dir).rem_euclid(n as i32) as usize,
            None => 0,
        });
        self.show_hit();
    }

    /// Starts searching every page for the find bar's text, from the page in view.
    fn start_search(&mut self) {
        self.find.id += 1;
        self.find.hits.clear();
        self.find.cur = None;
        self.find.pending = 0;
        self.find.more = false;
        let q = self.find.query.trim().to_string();
        let Some(w) = &self.worker else { return };
        w.keep.lock().unwrap().search = self.find.id;
        if q.chars().count() < 2 || self.pages.is_empty() {
            return;
        }
        let n = self.pages.len();
        self.find_from = self.current_page();
        self.find.pending = n;
        let pages = (0..n).map(|k| (self.find_from + k) % n).collect();
        w.send(Req::Search { generation: self.generation, id: self.find.id, pages, query: q });
    }

    /// The New document window: page size, orientation and count, then where to save it.
    fn new_doc_window(&mut self, ctx: &egui::Context) {
        let Some(nd) = &mut self.new_doc else { return };
        let mut open = true;
        let mut create = None;
        let mut cancel = false;
        egui::Window::new("New document").open(&mut open).resizable(false).collapsible(false).anchor(Align2::CENTER_TOP, vec2(0.0, 80.0)).show(ctx, |ui| {
            egui::Grid::new("newdoc").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Page size");
                egui::ComboBox::from_id_salt("newsize").width(180.0).selected_text(PAGE_SIZES[nd.size].0).show_ui(ui, |ui| {
                    for (i, (name, _)) in PAGE_SIZES.iter().enumerate() {
                        ui.selectable_value(&mut nd.size, i, *name);
                    }
                });
                ui.end_row();
                ui.label("Orientation");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut nd.landscape, false, "Portrait");
                    ui.selectable_value(&mut nd.landscape, true, "Landscape");
                });
                ui.end_row();
                ui.label("Pages");
                ui.add(egui::DragValue::new(&mut nd.pages).range(1..=200));
                ui.end_row();
            });
            ui.weak("Then drop in images (or Insert image) and mark up as usual.");
            ui.horizontal(|ui| {
                if ui.button("Create…").on_hover_text("Choose where to save it").clicked() {
                    let s = PAGE_SIZES[nd.size].1;
                    create = Some((if nd.landscape { vec2(s.y, s.x) } else { s }, nd.pages));
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if let Some((size, pages)) = create {
            self.ask = Some(Pick::NewDoc(size, pages));
            self.new_doc = None;
        } else if !open || cancel {
            self.new_doc = None;
        }
    }

    /// The find bar, top right of the drawing: text, match count, previous/next.
    fn find_bar(&mut self, ctx: &egui::Context) {
        if !self.find.open || self.pages.is_empty() || self.overview {
            return;
        }
        let mut close = false;
        egui::Area::new(egui::Id::new("greenline-find"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(self.canvas.max.x - 360.0, self.canvas.min.y + 8.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let edit = ui.add(egui::TextEdit::singleline(&mut self.find.query).hint_text("Find in document").desired_width(170.0));
                        if self.find.focus {
                            edit.request_focus();
                            self.find.focus = false;
                        }
                        if edit.changed() {
                            self.start_search();
                        }
                        let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
                        if edit.lost_focus() && enter {
                            self.step_hit(if shift { -1 } else { 1 });
                            edit.request_focus();
                        }
                        // F3 while typing (the shortcut handler sees it otherwise).
                        if edit.has_focus() && ui.input(|i| i.key_pressed(Key::F3)) {
                            self.step_hit(if shift { -1 } else { 1 });
                        }
                        if esc && (edit.has_focus() || edit.lost_focus()) {
                            close = true;
                        }
                        let count = match (self.find.hits.len(), self.find.cur) {
                            _ if self.find.query.trim().chars().count() < 2 => String::new(),
                            (0, _) if self.find.pending > 0 => "…".into(),
                            (0, _) => "No matches".into(),
                            (n, Some(c)) => format!("{} of {n}{}", c + 1, if self.find.pending > 0 || self.find.more { "+" } else { "" }),
                            (n, None) => format!("{n}"),
                        };
                        ui.add_sized([72.0, 18.0], egui::Label::new(egui::RichText::new(count).weak()));
                        let any = !self.find.hits.is_empty();
                        if ui.add_enabled(any, egui::Button::new("⏶")).on_hover_text("Previous (Shift+Enter, Shift+F3)").clicked() {
                            self.step_hit(-1);
                        }
                        if ui.add_enabled(any, egui::Button::new("⏷")).on_hover_text("Next (Enter, F3)").clicked() {
                            self.step_hit(1);
                        }
                        if ui.button("×").on_hover_text("Close (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if close {
            self.close_find();
        }
    }

    fn close_find(&mut self) {
        self.find.open = false;
        self.find.hits.clear();
        self.find.cur = None;
        self.find.pending = 0;
        self.find.id += 1;
        if let Some(w) = &self.worker {
            w.keep.lock().unwrap().search = self.find.id;
        }
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
        self.undo.push((self.annots.clone(), self.scales.clone()));
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

    /// The fill chosen for the current tool, if one was (`Some(None)`: none).
    fn tool_fill(&self) -> Option<Option<Fill>> {
        self.tool_fill.get(&self.tool).map(|i| i.map(|i| Fill { color: fill_color(i), alpha: self.fill_alpha }))
    }

    /// Gives a new markup the current tool's look.
    fn apply_style(&self, a: &mut Annot) {
        if let Some(f) = self.tool_fill().filter(|_| a.fillable()) {
            a.set_fill(f);
        }
        if a.takes_line_type() {
            a.style.line = self.tool_line.get(&self.tool).cloned();
        }
        if a.hatchable() {
            a.style.hatch = self.tool_hatch.get(&self.tool).cloned();
        }
        if a.text_size().is_some() && !matches!(a.kind, Kind::Measure { .. }) {
            [a.style.bold, a.style.italic, a.style.underline] = self.text_format;
        }
    }

    /// The bold / italic / underline the toolbar shows: the note being typed or
    /// the lead selected one, else what new notes get.
    fn shown_format(&self) -> [bool; 3] {
        let note = self.editing.or_else(|| self.selected.iter().rev().copied().find(|&id| self.find(id).is_some_and(|a| matches!(a.kind, Kind::Callout { .. } | Kind::Text { .. }))));
        match note.and_then(|id| self.find(id)) {
            Some(a) => [a.style.bold, a.style.italic, a.style.underline],
            None => self.text_format,
        }
    }

    /// Turns bold (0), italic (1) or underline (2) on or off for new notes and the
    /// selected or edited ones, as the toolbar shows it.
    fn toggle_format(&mut self, which: usize) {
        let on = !self.shown_format()[which];
        self.text_format[which] = on;
        let ids: Vec<u64> = match self.editing {
            Some(id) => vec![id],
            None => self.selected.clone(),
        };
        let notes = |a: &Annot| ids.contains(&a.id) && matches!(a.kind, Kind::Callout { .. } | Kind::Text { .. });
        if self.annots.iter().any(notes) {
            // While typing, the edit (begun with its own undo step) takes the change with it.
            if self.editing.is_none() {
                self.snapshot();
            }
            for a in self.annots.iter_mut().filter(|a| notes(a)) {
                [&mut a.style.bold, &mut a.style.italic, &mut a.style.underline][which].clone_from(&on);
                // Bold text is wider: refit the box from the width it wraps at.
                if which == 0 && self.editing != Some(a.id) {
                    a.widen_for_edit();
                    a.fit_box_w();
                }
            }
            self.changed();
        }
        // A click on B / I / U mustn't end the typing.
        if self.editing.is_some() {
            self.edit_needs_focus = true;
        }
    }

    fn set_line_type(&mut self, lt: Option<Arc<LineType>>) {
        if self.tool != Tool::Select {
            match &lt {
                Some(l) => self.tool_line.insert(self.tool, l.clone()),
                None => self.tool_line.remove(&self.tool),
            };
        }
        if self.annots.iter().any(|a| self.selected.contains(&a.id) && a.takes_line_type()) {
            self.edit_selected(|a| {
                if a.takes_line_type() {
                    a.style.line = lt.clone();
                }
            });
        }
    }

    /// The line type the toolbar shows: the lead selected markup's, else the tool's.
    fn shown_line_type(&self) -> Option<Arc<LineType>> {
        match self.selected.iter().rev().filter_map(|&id| self.find(id)).find(|a| a.takes_line_type()) {
            Some(a) => a.style.line.clone(),
            None => self.tool_line.get(&self.tool).cloned(),
        }
    }

    fn set_hatch(&mut self, h: Option<HatchFill>) {
        if self.tool != Tool::Select {
            match &h {
                Some(h) => self.tool_hatch.insert(self.tool, h.clone()),
                None => self.tool_hatch.remove(&self.tool),
            };
        }
        if self.annots.iter().any(|a| self.selected.contains(&a.id) && a.hatchable()) {
            self.edit_selected(|a| {
                if a.hatchable() {
                    a.style.hatch = h.clone();
                }
            });
        }
    }

    fn shown_hatch(&self) -> Option<HatchFill> {
        match self.selected.iter().rev().filter_map(|&id| self.find(id)).find(|a| a.hatchable()) {
            Some(a) => a.style.hatch.clone(),
            None => self.tool_hatch.get(&self.tool).cloned(),
        }
    }

    /// A markup drawn with the current tool, for previews.
    fn sketch(&self, page: usize, color: Rgb, kind: Kind) -> Annot {
        let mut a = Annot { id: 0, page, color, width: self.width, kind, wrap_w: None, group: None, style: Style::default() };
        self.apply_style(&mut a);
        a
    }

    fn add(&mut self, page: usize, kind: Kind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut a = Annot { id, page, color: self.color(), width: self.width, kind, wrap_w: None, group: None, style: Style::default() };
        self.apply_style(&mut a);
        self.annots.push(a);
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
        self.selected.iter().rev().filter_map(|&id| self.find(id)).find_map(label_size)
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

    /// A group id no markup has, and none a proposal batch has reserved for
    /// the rest of a group it's partly accepted.
    fn next_group(&self) -> u64 {
        let reserved = self.props.batches.iter().flat_map(|b| b.accepted_groups.values().copied());
        self.annots.iter().filter_map(|a| a.group).chain(reserved).max().unwrap_or(0) + 1
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
            Kind::Callout { anchor, leaders, .. } => {
                let r = a.text_box().unwrap();
                let mut hs = vec![(Handle::Anchor, *anchor), (Handle::BoxWidth, pos2(r.max.x, r.center().y))];
                hs.extend(leaders.iter().enumerate().map(|(i, tip)| (Handle::Leader(i), *tip)));
                hs
            }
            Kind::Text { .. } => {
                let r = a.text_box().unwrap();
                vec![(Handle::BoxWidth, pos2(r.max.x, r.center().y))]
            }
            Kind::Arrow { a: p, b } | Kind::Line { a: p, b } => vec![(Handle::A, *p), (Handle::B, *b)],
            Kind::Pen { .. } => vec![],
            Kind::Poly { pts, .. } | Kind::Measure { pts, .. } => pts.iter().enumerate().map(|(i, p)| (Handle::Vertex(i), *p)).collect(),
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
            (Kind::Callout { leaders, .. }, Handle::Leader(i)) => {
                if let Some(tip) = leaders.get_mut(i) {
                    *tip = p;
                }
            }
            (Kind::Callout { pos, box_w, size, .. } | Kind::Text { pos, box_w, size, .. }, Handle::BoxWidth) => {
                *box_w = (p.x - pos.x).max(4.0 * *size);
                a.wrap_w = Some(*box_w);
            }
            (Kind::Arrow { a, .. } | Kind::Line { a, .. }, Handle::A) => *a = p,
            (Kind::Arrow { b, .. } | Kind::Line { b, .. }, Handle::B) => *b = p,
            (Kind::Poly { pts, .. } | Kind::Measure { pts, .. }, Handle::Vertex(i)) => {
                if let Some(v) = pts.get_mut(i) {
                    *v = p;
                }
            }
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
        if let Some((annots, scales)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.annots, annots), std::mem::replace(&mut self.scales, scales)));
            self.selected.clear();
            self.changed();
        }
    }

    fn redo(&mut self) {
        if let Some((annots, scales)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.annots, annots), std::mem::replace(&mut self.scales, scales)));
            self.selected.clear();
            self.changed();
        }
    }

    fn set_tool(&mut self, t: Tool) {
        self.commit_edit();
        self.finish_measure();
        // Still open (an area that crosses itself): it belongs to the old tool.
        if self.building.take().is_some() {
            let what = if self.tool == Tool::Polygon { "polygon" } else { "area" };
            self.toast(format!("Unfinished {what} dropped: its outline crossed itself"));
        }
        self.placing = None;
        self.queued_pictures.clear();
        self.picture_epoch += 1;
        self.tool = t;
        if t != Tool::Select {
            self.selected.clear();
        }
    }

    fn set_color(&mut self, idx: usize) {
        self.tool_color.insert(self.tool, idx);
        self.edit_selected(|a| a.color = COLORS[idx].0);
    }

    /// Fill for the tool and the selected markups that take one: a colour
    /// index (`COLORS`, then white) or none.
    fn set_fill(&mut self, idx: Option<usize>) {
        // With Select it's an edit of the selection, not a default for what's placed next.
        if self.tool != Tool::Select {
            self.tool_fill.insert(self.tool, idx);
        }
        let alpha = self.fill_alpha;
        // A recoloured fill keeps its opacity.
        self.edit_fills(|a| {
            let keep = a.effective_fill().map_or(alpha, |f| f.alpha);
            a.set_fill(idx.map(|i| Fill { color: fill_color(i), alpha: keep }));
        });
    }

    fn set_fill_alpha(&mut self, alpha: u8) {
        // Never fully clear: that's what None is for, and it would leave nothing to adjust.
        let alpha = alpha.max(MIN_FILL_ALPHA);
        self.fill_alpha = alpha;
        // A callout's or area's default fill becomes a chosen one, so new ones get this opacity.
        if self.tool != Tool::Select && !self.tool_fill.contains_key(&self.tool) {
            match self.shown_fill() {
                Some(f) if f.color == WHITE => {
                    self.tool_fill.insert(self.tool, Some(COLORS.len()));
                }
                Some(f) => {
                    let i = COLORS.iter().position(|c| c.0 == f.color).unwrap_or(0);
                    self.tool_fill.insert(self.tool, Some(i));
                }
                None => {}
            }
        }
        self.edit_fills(|a| {
            if let Some(f) = a.effective_fill() {
                a.set_fill(Some(Fill { alpha, ..f }));
            }
        });
    }

    /// Applies `f` to each selected markup that takes a fill, as one undo step.
    fn edit_fills(&mut self, mut f: impl FnMut(&mut Annot)) {
        if self.annots.iter().any(|a| self.selected.contains(&a.id) && a.fillable()) {
            self.edit_selected(|a| {
                if a.fillable() {
                    f(a);
                }
            });
        }
    }

    /// The fill the toolbar shows: the lead selected markup's, else what the
    /// current tool will draw.
    fn shown_fill(&self) -> Option<Fill> {
        match self.selected.iter().rev().filter_map(|&id| self.find(id)).find(|a| a.fillable()) {
            Some(a) => a.effective_fill(),
            None => match self.tool_fill() {
                Some(f) => f,
                None => match self.tool {
                    Tool::Callout => Some(Fill { color: WHITE, alpha: 255 }),
                    Tool::Area => Some(Fill { color: self.color(), alpha: annot::AREA_TINT }),
                    _ => None,
                },
            },
        }
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
                if let Kind::Callout { size, .. } | Kind::Text { size, .. } | Kind::Measure { size, .. } = &mut a.kind {
                    *size = s;
                }
            });
        }
    }

    // -----------------------------------------------------------------------
    // Input

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Tabs answer even while typing (switching commits the note being typed).
        let n = self.tabs.len();
        let take = |k: Key, m: egui::Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
        let (cmd, cmd_shift) = (egui::Modifiers::COMMAND, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
        if take(Key::W, cmd) && (self.path.is_some() || n > 1) {
            self.close_tab(self.cur_tab);
        }
        if n > 1 {
            // Shift first: Ctrl+Tab would also match Ctrl+Shift+Tab.
            if take(Key::Tab, cmd_shift) || take(Key::PageUp, cmd) {
                self.switch_tab((self.cur_tab + n - 1) % n);
            } else if take(Key::Tab, cmd) || take(Key::PageDown, cmd) {
                self.switch_tab((self.cur_tab + 1) % n);
            }
        }
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
            if pressed(Key::N) {
                self.new_doc.get_or_insert_with(NewDoc::default);
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
            for (i, k) in [Key::B, Key::I, Key::U].into_iter().enumerate() {
                if pressed(k) {
                    self.toggle_format(i);
                }
            }
            if pressed(Key::F) && !self.pages.is_empty() {
                // Reopened with its old text: search again (closing dropped the results).
                if !self.find.open && !self.find.query.trim().is_empty() {
                    self.find.open = true;
                    self.start_search();
                }
                self.find.open = true;
                self.find.focus = true;
            }
            return;
        }
        if pressed(Key::F3) && !self.find.hits.is_empty() {
            self.step_hit(if shift { -1 } else { 1 });
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
        if pressed(Key::Y) {
            self.show_review = !self.show_review;
        }
        if pressed(Key::U) {
            self.scale_win.open = !self.scale_win.open;
        }
        // A measurement being clicked out: Enter finishes, Esc drops it, Backspace takes back a point.
        if self.building.is_some() {
            if pressed(Key::Enter) {
                self.finish_measure();
            } else if pressed(Key::Escape) {
                self.building = None;
                let what = if self.outlining() { "Outline" } else { "Measurement" };
                self.toast(format!("{what} cancelled"));
            } else if pressed(Key::Backspace) || pressed(Key::Delete) {
                if let Some((_, pts)) = &mut self.building {
                    pts.pop();
                    if pts.is_empty() {
                        self.building = None;
                    }
                }
            }
            return;
        }
        for (t, _, k) in TOOLS {
            if pressed(k) && (k != Key::N || shift == (t == Tool::Polygon)) {
                self.set_tool(t);
            }
        }
        if pressed(Key::Escape) {
            self.escape();
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
        if pressed(Key::Z) {
            self.zoom_to_selection();
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

    /// Esc: cancels a gesture, then closes the find bar, drops a placement or a
    /// scale tool, clears the selection, and finally returns to Select.
    fn escape(&mut self) {
        // Cancel any gesture in progress (the rest of it is swallowed until release).
        let mut cancelled = false;
        if let Drag::Move { before, .. } | Drag::Handle { before, .. } = std::mem::replace(&mut self.drag, Drag::Ignore) {
            self.annots = before;
            cancelled = true;
        }
        if self.find.open && !cancelled && self.placing.is_none() {
            // An open find bar goes first: its highlights are what's on top.
            self.close_find();
        } else if self.placing.take().is_some() {
            self.picture_epoch += 1;
            let more = std::mem::take(&mut self.queued_pictures).len();
            self.toast(if more > 0 { format!("Placing cancelled (and {more} more)") } else { "Placing cancelled".into() });
        } else if matches!(self.tool, Tool::Calibrate | Tool::ScaleRegion) {
            self.set_tool(Tool::Select);
        } else if !self.selected.is_empty() {
            self.selected.clear()
        } else {
            self.set_tool(Tool::Select)
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
        let mods = ctx.input(|i| Mods {
            add: i.modifiers.command || i.modifiers.shift,
            single: i.modifiers.alt,
            ctrl: i.modifiers.command && !i.modifiers.shift,
            shift: i.modifiers.shift,
        });
        self.shift = mods.shift;
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
                        // Ctrl+click on empty space drops a leader on the selected callout.
                        Some((pg, _)) if mods.ctrl && self.sole_callout(pg).is_some() => CursorIcon::Crosshair,
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
                    self.undo.push((before, self.scales.clone()));
                    self.redo.clear();
                    self.changed();
                }
            }
            Drag::Handle { id, before, .. } => {
                // A polygon's outline mustn't end up crossing itself.
                if self.find(id).is_some_and(|a| matches!(&a.kind, Kind::Poly { pts, closed: true } if measure::not_simple(pts))) {
                    self.annots = before;
                    self.toast("That would make the polygon's outline cross itself".into());
                    return;
                }
                self.undo.push((before, self.scales.clone()));
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
        let was_editing = self.editing.is_some();
        if was_editing {
            self.commit_edit();
        }
        let Some(page) = self.page_at(screen) else {
            if !was_editing && !mods.add {
                self.selected.clear();
            }
            return;
        };
        let p = self.screen_to_page(page, screen);
        if self.placing.is_some() {
            if !was_editing {
                self.place(page, p, p);
            }
            return;
        }
        if matches!(self.tool, Tool::Length | Tool::Area | Tool::Calibrate | Tool::Polyline | Tool::Polygon) && !was_editing {
            self.measure_point(page, p, double, mods.shift);
            return;
        }
        if self.tool == Tool::ScaleRegion {
            self.toast("Drag a box round the detail that has its own scale · Esc cancels".into());
            return;
        }
        // Ctrl+click on empty space with one callout selected adds a leader.
        // This beats the other tool actions, so it works on the click that
        // ends typing and right after placing a callout with C.
        if mods.ctrl && self.hit(page, p).is_none() && self.sole_callout(page).is_some() {
            self.add_leader_at(page, p);
            return;
        }
        // The click that ends typing is otherwise swallowed.
        if was_editing {
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

    /// The selected callout when it is the only thing selected on `page`.
    fn sole_callout(&self, page: usize) -> Option<u64> {
        self.sole().filter(|a| a.page == page && matches!(&a.kind, Kind::Callout { .. })).map(|a| a.id)
    }

    /// Ctrl+click on empty space with one callout selected: a new leader whose
    /// arrowhead sits at `tip` and whose tail meets the first leader's bend.
    /// Switches to Select so its tip can be dragged straight away.
    fn add_leader_at(&mut self, page: usize, tip: Pos2) {
        let Some(id) = self.sole_callout(page) else { return };
        self.snapshot();
        if let Some(a) = self.find_mut(id) {
            if let Kind::Callout { leaders, .. } = &mut a.kind {
                leaders.push(tip);
            }
        }
        self.set_tool(Tool::Select);
        self.changed();
        self.toast("Leader added — Ctrl+click again for another · drag its tip to adjust".into());
    }

    fn create(&mut self, page: usize, start: Pos2, end: Pos2, dragged: bool) {
        let bounds = Rect::from_min_size(Pos2::ZERO, self.pages[page]);
        let end = bounds.clamp(end);
        if matches!(self.tool, Tool::Length | Tool::Area | Tool::Calibrate | Tool::ScaleRegion | Tool::Polyline | Tool::Polygon) {
            self.create_measure(page, start, end);
            return;
        }
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
                Kind::Callout { anchor: start, pos, box_w, text: String::new(), size, leaders: Vec::new() }
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
            Tool::Select | Tool::Pen | Tool::Length | Tool::Area | Tool::Calibrate | Tool::ScaleRegion | Tool::Polyline | Tool::Polygon => return,
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
    // Measuring

    /// A click with a measuring tool: starts a measurement, adds a point, or
    /// finishes it (double-click, the area's first point again, or a
    /// calibration line's second point).
    fn measure_point(&mut self, page: usize, p: Pos2, double: bool, shift: bool) {
        let p = Rect::from_min_size(Pos2::ZERO, self.pages[page]).clamp(p);
        let tol = HIT_PX / self.zoom;
        let (area, calibrating) = (matches!(self.tool, Tool::Area | Tool::Polygon), self.tool == Tool::Calibrate);
        let Some((pg, pts)) = &mut self.building else {
            self.building = Some((page, vec![p]));
            if !calibrating {
                let finish = if area { "click the first point or double-click to close" } else { "double-click or Enter to finish" };
                self.toast(format!("Click the next point · {finish} · Shift squares it · Backspace takes one back · Esc drops it"));
            }
            return;
        };
        if *pg != page {
            let other = *pg + 1;
            let what = if matches!(self.tool, Tool::Polyline | Tool::Polygon) { "outline" } else { "measurement" };
            self.toast(format!("Finish the {what} on page {other} first (Enter), or Esc to drop it"));
            return;
        }
        let last = *pts.last().unwrap();
        let p = if shift { square(last, p) } else { p };
        let closes = area && pts.len() >= 3 && (pts[0] - p).length() <= tol;
        // The second click of a double-click lands on the first.
        if !closes && (p - last).length() > 2.0 / self.zoom {
            pts.push(p);
        }
        if double || closes || (calibrating && pts.len() >= 2) {
            self.finish_measure();
        }
    }

    /// Ends the measurement being clicked out, adding it if it has enough
    /// points; a calibration line goes to the scale window instead.
    fn finish_measure(&mut self) {
        let Some((page, pts)) = self.building.take() else { return };
        match self.tool {
            Tool::Calibrate if pts.len() >= 2 => self.set_calibration(page, pts[0], pts[pts.len() - 1]),
            Tool::Length if pts.len() >= 2 => self.add_measure(page, pts, false),
            Tool::Area if pts.len() >= 3 && measure::crosses_itself(&pts) => {
                // Keep it going, so Backspace can take back the point that crossed.
                self.building = Some((page, pts));
                self.toast("The outline crosses itself: Backspace to take back a point, or Esc to drop it".into());
            }
            Tool::Area if pts.len() >= 3 => self.add_measure(page, pts, true),
            Tool::Area => self.toast("An area needs at least three points".into()),
            Tool::Polyline if pts.len() >= 2 => self.add_poly(page, pts, false),
            Tool::Polygon if pts.len() >= 3 && measure::not_simple(&pts) => {
                self.building = Some((page, pts));
                self.toast("The outline crosses itself: Backspace to take back a point, or Esc to drop it".into());
            }
            Tool::Polygon if pts.len() >= 3 => self.add_poly(page, pts, true),
            Tool::Polygon => self.toast("A polygon needs at least three points".into()),
            _ => {}
        }
    }

    /// A drag with a measuring tool: one segment, a rectangle, a calibration
    /// line or a scale region. While clicking out a measurement, its end is the next point.
    fn create_measure(&mut self, page: usize, start: Pos2, end: Pos2) {
        if self.building.is_some() {
            self.measure_point(page, end, false, self.shift);
            return;
        }
        if (end - start).length() * self.zoom < 3.0 {
            if self.tool != Tool::ScaleRegion {
                self.measure_point(page, start, false, false);
            }
            return;
        }
        let squared = if self.shift { square(start, end) } else { end };
        match self.tool {
            Tool::Length => self.add_measure(page, vec![start, squared], false),
            Tool::Area => {
                let r = Rect::from_two_pos(start, end);
                if r.width().min(r.height()) * self.zoom >= 3.0 {
                    self.add_measure(page, annot::rect_pts(r), true);
                }
            }
            Tool::Polyline => self.add_poly(page, vec![start, squared], false),
            Tool::Polygon => {
                let r = Rect::from_two_pos(start, end);
                if r.width().min(r.height()) * self.zoom >= 3.0 {
                    self.add_poly(page, annot::rect_pts(r), true);
                }
            }
            Tool::Calibrate => self.set_calibration(page, start, squared),
            Tool::ScaleRegion => {
                // A calibration line drawn before belongs to the whole page it's on, not this region.
                self.scale_win.calib = None;
                self.scale_win.target = Some((page, Rect::from_two_pos(start, end)));
                self.scale_win.open = true;
                self.tool = Tool::Select;
                self.toast("Region drawn: now choose or calibrate its scale in the scale window".into());
            }
            _ => {}
        }
    }

    /// Whether the tool clicks out a polyline or polygon (rather than a measurement).
    fn outlining(&self) -> bool {
        matches!(self.tool, Tool::Polyline | Tool::Polygon)
    }

    fn add_poly(&mut self, page: usize, pts: Vec<Pos2>, closed: bool) {
        self.snapshot();
        self.add(page, Kind::Poly { pts, closed });
    }

    fn add_measure(&mut self, page: usize, pts: Vec<Pos2>, area: bool) {
        self.snapshot();
        let id = self.add(page, Kind::Measure { pts, area, name: String::new(), value: String::new(), size: self.text_size });
        measure::relabel(&mut self.annots, &self.scales, self.prefs.units);
        let Some(a) = self.find(id) else { return };
        let msg = if let Kind::Measure { pts, .. } = &a.kind {
            match measure::scale_for(&self.scales, page, pts) {
                Some(_) => format!("{}: {}", a.kind_name(), a.label()),
                None => format!("No scale on page {} yet: press U to set one", page + 1),
            }
        } else {
            String::new()
        };
        self.toast(msg);
    }

    fn set_calibration(&mut self, page: usize, a: Pos2, b: Pos2) {
        self.tool = Tool::Select;
        if (b - a).length() * self.zoom < 10.0 {
            self.toast("Calibration line too short: zoom in and drag along a longer dimension".into());
            return;
        }
        if let Some((rp, _)) = self.scale_win.target.filter(|(rp, _)| *rp != page) {
            self.toast(format!("The new region is on page {}: calibrate along a dimension there, or go back to the whole page", rp + 1));
            return;
        }
        self.scale_win.calib = Some((page, a, b));
        self.scale_win.calib_text.clear();
        self.scale_win.open = true;
        self.toast("Type the line's real length in the scale window".into());
    }

    /// The page the scale window is working on.
    fn scale_page(&self) -> usize {
        self.scale_win.calib.map(|c| c.0).or(self.scale_win.target.map(|t| t.0)).unwrap_or_else(|| self.current_page())
    }

    /// Sets `page`'s scale (or the scale window's target region's), replacing the one it had.
    fn apply_scale(&mut self, page: usize, per_pt: f64, label: String) {
        if !(per_pt.is_finite() && per_pt > 0.0) {
            self.toast("That scale can't be used".into());
            return;
        }
        let region = self.scale_win.target.filter(|(p, _)| *p == page).map(|(_, r)| r);
        self.snapshot();
        self.scales.retain(|s| !(s.page == page && s.region == region));
        self.scales.push(Scale { page, region, per_pt, label: label.clone() });
        self.scale_win.target = None;
        self.scale_win.calib = None;
        self.scale_win.calib_text.clear();
        self.changed();
        let what = if region.is_some() { format!("Page {} region", page + 1) } else { format!("Page {}", page + 1) };
        self.toast(format!("{what} scale: {label} · check it against a dimension on the sheet"));
    }

    fn scale_window(&mut self, ctx: &egui::Context) {
        if !self.scale_win.open || self.pages.is_empty() {
            return;
        }
        let page = self.scale_page().min(self.pages.len() - 1);
        if let Some(w) = &self.worker {
            if self.scale_win.asked.insert(page) {
                w.send(Req::PageText { generation: self.generation, page });
            }
        }
        let units = self.prefs.units;
        let target = self.scale_win.target.filter(|(p, _)| *p == page).map(|(_, r)| r);
        let mut open = true;
        let mut apply: Option<(f64, String)> = None;
        let mut remove = None;
        let mut copy_all = false;
        egui::Window::new("Scale").open(&mut open).resizable(false).default_pos(self.canvas.right_top() + vec2(-330.0, 20.0)).show(ctx, |ui| {
            ui.set_max_width(300.0);
            let current = if target.is_some() { None } else { self.scales.iter().find(|s| s.page == page && s.region.is_none()).cloned() };
            ui.label(egui::RichText::new(format!("Page {}{}", page + 1, if target.is_some() { " · new region" } else { "" })).strong());
            ui.label(match (&current, target) {
                (_, Some(_)) => "Set this region's scale:".to_string(),
                (Some(s), None) => format!("Scale: {}", s.label),
                (None, None) => "No scale yet: measurements read \"no scale\"".to_string(),
            });
            ui.horizontal(|ui| {
                ui.label("Drawing scale");
                egui::ComboBox::from_id_salt("scale-preset").selected_text("Choose…").height(420.0).show_ui(ui, |ui| {
                    for (label, ratio) in measure::PRESETS {
                        if ui.selectable_label(false, *label).clicked() {
                            apply = Some((Scale::from_ratio(page, None, *ratio, label).per_pt, label.to_string()));
                        }
                    }
                });
            });
            let found = self.scale_win.found.get(&page).cloned().unwrap_or_default();
            if !found.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("On this sheet:");
                    for (label, ratio) in &found {
                        if ui.button(label).on_hover_text("Written on the sheet. Use it for this page (or the region)").clicked() {
                            apply = Some((Scale::from_ratio(page, None, *ratio, label).per_pt, label.clone()));
                        }
                    }
                });
            }
            ui.separator();
            match self.scale_win.calib {
                Some((cp, a, b)) if cp == page => {
                    let len = (b - a).length() as f64;
                    ui.label(format!("Calibration line: {} on paper", measure::fmt_len(len / 72.0, Units::Imperial)));
                    ui.horizontal(|ui| {
                        ui.label("Its real length");
                        let hint = if units == Units::Metric { "3.2 m" } else { "12'-6\"" };
                        let r = ui.add(egui::TextEdit::singleline(&mut self.scale_win.calib_text).hint_text(hint).desired_width(90.0));
                        if !r.has_focus() && self.scale_win.calib_text.is_empty() {
                            r.request_focus();
                        }
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if ui.button("Apply").clicked() || enter {
                            match measure::parse_length(&self.scale_win.calib_text, units) {
                                Some(real) => {
                                    let per_pt = real / len;
                                    let near = measure::PRESETS.iter().find(|(_, r)| (r / (per_pt * 72.0) - 1.0).abs() < 0.015).map(|(l, _)| format!(" (≈ {l})")).unwrap_or_default();
                                    apply = Some((per_pt, format!("Calibrated: {}{near}", measure::fmt_len(real, units))));
                                }
                                None => self.toast("Couldn't read that length: try 12'-6\", 150\", 3.2 m or 3200 mm".into()),
                            }
                        }
                    });
                    if ui.small_button("Cancel calibration").clicked() {
                        self.scale_win.calib = None;
                    }
                }
                _ => {
                    if ui.button("Calibrate…").on_hover_text("Drag along a dimension you know, then type its real length").clicked() {
                        self.set_tool(Tool::Calibrate);
                        self.toast("Drag along a dimension you know (or click its two ends) · Esc cancels".into());
                    }
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Add region…").on_hover_text("For a detail drawn at its own scale: drag a box round it, then set its scale").clicked() {
                    self.set_tool(Tool::ScaleRegion);
                    self.toast("Drag a box round the detail that has its own scale · Esc cancels".into());
                }
                if target.is_some() && ui.button("Back to whole page").clicked() {
                    self.scale_win.target = None;
                }
            });
            let regions: Vec<(usize, String)> = self.scales.iter().enumerate().filter(|(_, s)| s.page == page && s.region.is_some()).map(|(i, s)| (i, s.label.clone())).collect();
            for (n, (i, label)) in regions.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("Region {}: {label}", n + 1));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(*i);
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Units");
                for (u, name) in [(Units::Imperial, "Feet-inches"), (Units::Metric, "Metric")] {
                    if ui.selectable_value(&mut self.prefs.units, u, name).changed() {
                        self.prefs_dirty = true;
                    }
                }
            });
            if current.is_some() && self.pages.len() > 1 && ui.button(format!("Use on all {} pages", self.pages.len())).on_hover_text("This page's scale on every page; regions stay as they are").clicked() {
                copy_all = true;
            }
            let size = self.pages[page] / 72.0;
            let size_text = format!("{} × {} in", trim_num(size.x), trim_num(size.y));
            if size.x.max(size.y) < 16.5 {
                ui.colored_label(Color32::from_rgb(230, 120, 0), format!("This sheet is {size_text}, smaller than a drawing sheet. If it's a reduced print, the scale written on it is wrong here: calibrate instead."));
            } else {
                ui.weak(format!("Sheet {size_text}. A drawing scale assumes the PDF is at its printed size; a reduced or scanned set needs calibrating."));
            }
            ui.weak("Check a known dimension before relying on measurements.");
        });
        if !open {
            self.scale_win.open = false;
            self.scale_win.calib = None;
            self.scale_win.target = None;
            if matches!(self.tool, Tool::Calibrate | Tool::ScaleRegion) {
                self.set_tool(Tool::Select);
            }
        }
        if let Some((per_pt, label)) = apply {
            self.apply_scale(page, per_pt, label);
        }
        if let Some(i) = remove {
            self.snapshot();
            self.scales.remove(i);
            self.changed();
        }
        if copy_all {
            if let Some(s) = self.scales.iter().find(|s| s.page == page && s.region.is_none()).cloned() {
                self.snapshot();
                self.scales.retain(|t| t.region.is_some());
                self.scales.extend((0..self.pages.len()).map(|p| Scale { page: p, ..s.clone() }));
                self.changed();
                self.toast(format!("{} on all {} pages", s.label, self.pages.len()));
            }
        }
    }

    /// Writes `<name>_measurements.csv` next to the PDF: one row per
    /// measurement in page order, each value as text and as a number.
    fn export_measurements(&mut self) {
        self.commit_edit();
        let Some(src) = &self.path else { return };
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let out = src.with_file_name(format!("{stem}_measurements.csv"));
        let units = self.prefs.units;
        let mut order: Vec<&Annot> = self.annots.iter().filter(|a| matches!(a.kind, Kind::Measure { .. })).collect();
        if order.is_empty() {
            self.toast("No measurements yet (D for length, E for area)".into());
            return;
        }
        order.sort_by_key(|a| page_order(a));
        let csv = measure::csv(&order, &self.scales, units);
        let unscaled = order.iter().filter(|a| measure::measured(a, &self.scales).is_none()).count();
        match std::fs::write(&out, csv) {
            Ok(()) if unscaled > 0 => self.toast(format!("Measurements → {} ({unscaled} with no scale)", out.display())),
            Ok(()) => self.toast(format!("Measurements → {}", out.display())),
            Err(e) => self.toast(format!("Could not write {}: {e}", out.display())),
        }
    }

    // -----------------------------------------------------------------------
    // Proposals (agents' markups for review)

    /// Rereads the proposals file when it changes (checked once a second);
    /// opens the review panel when new ones arrive.
    fn poll_proposals(&mut self, now: f64) {
        if now - self.props_checked < 1.0 {
            return;
        }
        self.props_checked = now;
        let Some(file) = self.props_path.clone() else { return };
        if proposals::modified(&file) == self.props_mtime {
            return;
        }
        let before = self.props.count();
        self.reload_proposals();
        let n = self.props.count();
        if n > before {
            self.show_review = true;
            let who: Vec<&str> = self.props.batches.iter().map(|b| b.agent.as_str()).collect();
            self.toast(format!("{} proposed by {} to review (Y)", n - before, who.last().copied().unwrap_or("an agent")));
        }
    }

    /// Reads the proposals file as it is now.
    fn reload_proposals(&mut self) {
        let Some(file) = self.props_path.clone() else { return };
        self.props_mtime = proposals::modified(&file);
        match proposals::load_file(&file) {
            Ok(p) => self.props = p,
            Err(e) => self.toast(format!("Couldn't read the proposals: {e}")),
        }
        self.relabel_proposals();
    }

    /// Accepted scales with every batch's scales over them, as the proposals would leave things.
    fn proposed_scales(&self) -> Vec<Scale> {
        self.props.batches.iter().fold(self.scales.clone(), |acc, b| crate::ops::merged_scales(&acc, &b.scales))
    }

    fn relabel_proposals(&mut self) {
        let scales = self.proposed_scales();
        let units = self.prefs.units;
        for b in &mut self.props.batches {
            measure::relabel(&mut b.annots, &scales, units);
        }
    }

    /// Takes markups (`None`: all of them) and scales out of batch `id`.
    /// Accepted ones are added to the markups, saved, and only then taken out
    /// of the proposals, all under the proposals' lock: a failed save changes
    /// nothing, and a batch arriving meanwhile isn't lost. One undo step.
    fn settle(&mut self, id: &str, annot: Option<u64>, scale: Option<Scale>, accept: bool) {
        let (Some(path), Some(file)) = (self.path.clone(), self.props_path.clone()) else { return };
        if accept && (self.read_only || self.markups_file.is_none()) {
            self.toast(format!("Can't accept while not autosaving ({})", self.read_only_why));
            return;
        }
        let whole = annot.is_none() && scale.is_none();
        let markups_file = self.markups_file.clone();
        let (mut annots_now, mut scales_now, mut accepted_now) = (self.annots.clone(), self.scales.clone(), self.accepted.clone());
        let mut next_id = self.next_id;
        let mut next_group = self.next_group();
        // Set once the markups file has the accepted work, whatever happens to the proposals write.
        let mut saved = false;
        let taken = proposals::update_file(&file, &path, |p| {
            // Keys for batches that are gone can go; this batch's stay until it's written.
            accepted_now.retain(|k| k.split_once('/').is_some_and(|(bid, _)| p.batches.iter().any(|b| b.id == bid)));
            let Some(b) = p.batches.iter_mut().find(|b| b.id == id) else { return Ok(0) };
            // Markups an accept took in before its proposals write failed are
            // skipped below; first recover, from the whole batch, the groups they
            // joined, so the rest of each group joins them too.
            let receipt = |bid: &str, a: &Annot| accepted_now.iter().find_map(|k| k.strip_prefix(&format!("{bid}/{}:", a.id)).and_then(|n| n.parse::<u64>().ok()));
            let done: Vec<(u64, u64)> = b.annots.iter().filter_map(|a| receipt(&b.id, a).map(|n| (a.id, n))).collect();
            for (pid, new_id) in &done {
                let g = b.annots.iter().find(|a| a.id == *pid).and_then(|a| a.group);
                if let (Some(g), Some(ng)) = (g, annots_now.iter().find(|x| x.id == *new_id).and_then(|x| x.group)) {
                    b.accepted_groups.insert(g, ng);
                }
            }
            let annots: Vec<Annot> = match annot {
                Some(aid) => b.annots.iter().position(|a| a.id == aid).map(|i| vec![b.annots.remove(i)]).unwrap_or_default(),
                None if whole => std::mem::take(&mut b.annots),
                None => Vec::new(),
            };
            // By value, not row: the file may have changed since the row was drawn.
            let scales: Vec<Scale> = match scale {
                Some(want) => b.scales.iter().position(|s| *s == want).map(|i| vec![b.scales.remove(i)]).unwrap_or_default(),
                None if whole => std::mem::take(&mut b.scales),
                None => Vec::new(),
            };
            let n = annots.len() + scales.len();
            if accept && n > 0 {
                for a in annots {
                    if done.iter().any(|(pid, _)| *pid == a.id) {
                        continue;
                    }
                    accepted_now.push(format!("{}/{}:{}", b.id, a.id, next_id));
                    let group = a.group.map(|g| {
                        *b.accepted_groups.entry(g).or_insert_with(|| {
                            next_group += 1;
                            next_group - 1
                        })
                    });
                    annots_now.push(Annot { id: next_id, group, ..a });
                    next_id += 1;
                }
                for s in scales {
                    // Receipts for scales too: a retry mustn't put back a scale the
                    // reviewer has since changed.
                    let key = format!("{}/scale:{}", b.id, serde_json::to_string(&s).unwrap_or_default());
                    if accepted_now.contains(&key) {
                        continue;
                    }
                    accepted_now.push(key);
                    scales_now.retain(|t| !(t.page == s.page && t.region == s.region));
                    scales_now.push(s);
                }
                if let Some(f) = &markups_file {
                    sidecar::save_file(f, &path, &annots_now, &scales_now, &accepted_now)?;
                    saved = true;
                }
            }
            p.batches.retain(|b| !b.annots.is_empty() || !b.scales.is_empty());
            Ok(n)
        });
        // Reread: the file is the truth, including anything added meanwhile.
        self.reload_proposals();
        let n = match taken {
            Ok(n) => n,
            // The markups are saved; only the proposals couldn't be rewritten.
            // Show what's on disk: a later accept skips what's already in.
            Err(e) if saved => {
                self.snapshot();
                self.annots = annots_now;
                self.scales = scales_now;
                self.accepted = accepted_now;
                self.next_id = next_id;
                self.selected.clear();
                self.dirty = false;
                self.toast(format!("Accepted, but couldn't update the proposals ({e}): they still show, and accepting them again adds nothing twice"));
                return;
            }
            Err(e) => {
                let what = if accept { "accept" } else { "reject" };
                self.toast(format!("Couldn't {what}: {e}"));
                return;
            }
        };
        if !accept {
            self.toast(format!("Rejected {n}"));
            return;
        }
        if n == 0 {
            return;
        }
        self.snapshot();
        self.annots = annots_now;
        self.scales = scales_now;
        self.accepted = accepted_now;
        self.next_id = next_id;
        self.selected.clear();
        // Saved above.
        self.dirty = false;
        self.toast(format!("Accepted {n} · Ctrl+Z takes {} back out of your markups", if n == 1 { "it" } else { "them" }));
    }

    /// Scrolls a proposed markup into the middle of the view.
    fn focus_proposal(&mut self, a: &Annot) {
        if a.page >= self.pages.len() {
            return;
        }
        let c = self.page_doc_rect(a.page).min + a.bounds().center().to_vec2();
        self.overview = false;
        self.offset = c.to_vec2() - self.canvas.size() / (2.0 * self.zoom);
    }

    fn review_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.strong(format!("Review ({})", self.props.count()));
        });
        ui.separator();
        if self.props.batches.is_empty() {
            ui.weak("Nothing to review. Markups an agent proposes with greenline-cli appear here (and on the sheets, outlined in orange) for you to accept or reject.");
            return;
        }
        if self.read_only {
            ui.colored_label(Color32::from_rgb(230, 80, 60), format!("Not autosaving ({}): you can look, but not accept.", self.read_only_why));
        }
        let mut action: Option<(String, Option<u64>, Option<Scale>, bool)> = None;
        let mut focus: Option<Annot> = None;
        let batches = self.props.batches.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for b in &batches {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("{} · {}", b.agent, b.created.get(..16).unwrap_or(&b.created).replace('T', " "))).strong());
                if !b.summary.is_empty() {
                    ui.label(&b.summary);
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.read_only, egui::Button::new("Accept all")).clicked() {
                        action = Some((b.id.clone(), None, None, true));
                    }
                    if ui.button("Reject all").clicked() {
                        action = Some((b.id.clone(), None, None, false));
                    }
                });
                for s in &b.scales {
                    ui.horizontal(|ui| {
                        let what = if s.region.is_some() { "region scale" } else { "scale" };
                        ui.label(format!("Page {} {what}: {}", s.page + 1, s.label));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Reject").clicked() {
                                action = Some((b.id.clone(), None, Some(s.clone()), false));
                            }
                            if ui.add_enabled(!self.read_only, egui::Button::new("Accept").small()).clicked() {
                                action = Some((b.id.clone(), None, Some(s.clone()), true));
                            }
                        });
                    });
                }
                let mut order: Vec<&Annot> = b.annots.iter().collect();
                order.sort_by_key(|a| page_order(a));
                for a in order {
                    let r = ui.horizontal(|ui| {
                        let (sw, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().rect_filled(sw, 2.0, Color32::from_rgb(a.color[0], a.color[1], a.color[2]));
                        let text = format!("p{} {}", a.page + 1, if note_of(a).trim().is_empty() { a.kind_name().to_string() } else { note_of(a).replace('\n', " ") });
                        let label = ui.add(egui::Label::new(text).truncate().sense(Sense::click())).on_hover_text("Click to show it");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Reject").clicked() {
                                action = Some((b.id.clone(), Some(a.id), None, false));
                            }
                            if ui.add_enabled(!self.read_only, egui::Button::new("Accept").small()).clicked() {
                                action = Some((b.id.clone(), Some(a.id), None, true));
                            }
                        });
                        label
                    });
                    if r.inner.clicked() {
                        focus = Some(a.clone());
                    }
                    if r.response.hovered() || r.inner.hovered() {
                        self.props_hover = Some((b.id.clone(), a.id));
                    }
                }
                ui.separator();
            }
        });
        if let Some(a) = focus {
            self.focus_proposal(&a);
        }
        if let Some((id, annot, scale, accept)) = action {
            self.settle(&id, annot, scale, accept);
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
        if let Some(a) = self.find_mut(id) {
            // Placed as previewed: no fill from whatever tool was last used.
            a.style = Style::default();
            if matches!(place, Place::Date) {
                a.color = DATE_COLOR;
            }
        }
        // Ready to nudge it into the box.
        self.tool = Tool::Select;
        self.selected = vec![id];
        if let Some(next) = self.queued_pictures.pop_front() {
            self.start_placing(next);
        }
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
        self.show_review = false;
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
            // The image being placed, so its preview shows it.
            .chain(match &self.placing {
                Some(Place::Image { image, .. }) => Some(image),
                _ => None,
            })
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
        painter.rect_filled(self.canvas, 0.0, crate::theme::canvas(ui.visuals()));
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
                    prims = typed_note_prims(a);
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
                Drag::Create { page, start, cur } if *page == i && self.building.is_none() => {
                    let cur = if self.shift && matches!(self.tool, Tool::Length | Tool::Calibrate | Tool::Polyline) { square(*start, *cur) } else { *cur };
                    let preview = self.preview_kind(*start, cur);
                    if let Some(kind) = preview {
                        let color = if matches!(self.tool, Tool::Calibrate | Tool::ScaleRegion) { [0, 150, 255] } else { self.color() };
                        let mut a = self.sketch(i, color, kind);
                        measure::relabel(std::slice::from_mut(&mut a), &self.scales, self.prefs.units);
                        paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.7, &self.textures);
                    }
                }
                Drag::Pen { page, pts } if *page == i => {
                    let a = Annot { id: 0, page: i, color: self.color(), width: self.width, kind: Kind::Pen { pts: pts.clone() }, wrap_w: None, group: None, style: Default::default() };
                    paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 1.0, &self.textures);
                }
                Drag::Marquee { page, start, cur } if *page == i => {
                    let r = Rect::from_two_pos(self.page_to_screen(i, *start), self.page_to_screen(i, *cur));
                    pp.rect(r, 0.0, SEL.gamma_multiply(0.12), Stroke::new(1.0, SEL), egui::StrokeKind::Middle);
                }
                _ => {}
            }
            // Proposed markups: drawn as they'd be accepted, each over a faint orange
            // wash and in a dashed orange outline.
            for b in &self.props.batches {
                for a in b.annots.iter().filter(|a| a.page == i) {
                    let r = a.bounds();
                    let r = Rect::from_min_max(self.page_to_screen(i, r.min), self.page_to_screen(i, r.max)).expand(4.0);
                    let hot = self.props_hover.as_ref().is_some_and(|(bid, id)| *bid == b.id && *id == a.id);
                    pp.rect_filled(r, 0.0, PROPOSED.gamma_multiply(if hot { 0.14 } else { 0.08 }));
                    paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.85, &self.textures);
                    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                    pp.extend(Shape::dashed_line(&pts, Stroke::new(if hot { 2.5 } else { 1.2 }, PROPOSED), 5.0, 3.0));
                }
            }
            // The measurement being clicked out, running to the pointer.
            if let Some((page, pts)) = self.building.as_ref().filter(|(p, _)| *p == i) {
                let mut pts = pts.clone();
                if let Some(h) = self.hover.filter(|h| self.page_at(*h) == Some(*page)) {
                    let h = self.screen_to_page(i, h);
                    pts.push(if self.shift { square(*pts.last().unwrap(), h) } else { h });
                }
                let calibrating = self.tool == Tool::Calibrate;
                let color = if calibrating { [0, 150, 255] } else { self.color() };
                let kind = if self.outlining() {
                    Kind::Poly { pts: pts.clone(), closed: self.tool == Tool::Polygon && pts.len() >= 3 }
                } else {
                    Kind::Measure { pts: pts.clone(), area: self.tool == Tool::Area && pts.len() >= 3, name: String::new(), value: String::new(), size: self.text_size }
                };
                let mut a = self.sketch(i, color, kind);
                if !calibrating && !self.outlining() {
                    measure::relabel(std::slice::from_mut(&mut a), &self.scales, self.prefs.units);
                }
                paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.8, &self.textures);
                for p in &pts {
                    pp.circle_stroke(self.page_to_screen(i, *p), 3.0, Stroke::new(1.0, SEL));
                }
            }
            // Scale regions and the calibration line while the scale window is up.
            if self.scale_win.open || self.tool == Tool::ScaleRegion {
                let regions = self.scales.iter().filter(|s| s.page == i).filter_map(|s| s.region.map(|r| (r, s.label.as_str())));
                let target = self.scale_win.target.filter(|(p, _)| *p == i).map(|(_, r)| (r, "New region"));
                for (r, label) in regions.chain(target) {
                    let r = Rect::from_min_max(self.page_to_screen(i, r.min), self.page_to_screen(i, r.max));
                    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                    pp.extend(Shape::dashed_line(&pts, Stroke::new(1.5, SEL), 6.0, 4.0));
                    pp.text(r.left_top() + vec2(4.0, 3.0), Align2::LEFT_TOP, label, FontId::proportional(12.0), SEL);
                }
            }
            if let Some((_, a, b)) = self.scale_win.calib.filter(|(p, ..)| *p == i) {
                pp.line_segment([self.page_to_screen(i, a), self.page_to_screen(i, b)], Stroke::new(2.0, SEL));
                for p in [a, b] {
                    pp.circle_stroke(self.page_to_screen(i, p), 4.0, Stroke::new(1.5, SEL));
                }
            }
            // Where a stamp being placed will land: the drag so far, or centred on the pointer.
            if let Some(place) = &self.placing {
                let at = match &self.drag {
                    Drag::Place { page, start, cur } => (*page == i).then_some((*start, *cur)),
                    _ => self.hover.filter(|h| self.page_at(*h) == Some(i)).map(|h| self.screen_to_page(i, h)).map(|p| (p, p)),
                };
                if let Some((start, cur)) = at {
                    let color = if matches!(place, Place::Date) { DATE_COLOR } else { self.color() };
                    let a = Annot { id: 0, page: i, color, width: self.width, kind: self.placed_kind(place, start, cur), wrap_w: None, group: None, style: Default::default() };
                    paint_prims(&pp, self.page_to_screen(i, Pos2::ZERO), self.zoom, &a.prims(), k, 0.6, &self.textures);
                    let b = a.bounds();
                    pp.rect_stroke(Rect::from_min_max(self.page_to_screen(i, b.min), self.page_to_screen(i, b.max)), 0.0, Stroke::new(1.0, SEL), egui::StrokeKind::Outside);
                }
            }
            // Search matches: a soft wash on each, the current one outlined.
            if self.find.open {
                let cur = self.find.cur.and_then(|c| self.find.hits.get(c)).map(|h| h as *const _);
                for h in self.find.hits.iter().filter(|(p, _)| *p == i) {
                    let current = cur == Some(h as *const _);
                    for r in &h.1 {
                        let r = Rect::from_min_max(self.page_to_screen(i, r.min), self.page_to_screen(i, r.max)).expand(1.5);
                        pp.rect_filled(r, 2.0, FOUND.gamma_multiply(if current { 0.45 } else { 0.25 }));
                        if current {
                            pp.rect_stroke(r, 2.0, Stroke::new(2.0, FOUND), egui::StrokeKind::Outside);
                        }
                    }
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
                Kind::Callout { anchor: start, pos: pos2(x, cur.y - h / 2.0), box_w, text: String::new(), size, leaders: Vec::new() }
            }
            Tool::Text => Kind::Rect { rect },
            Tool::Cloud => Kind::Cloud { rect },
            Tool::Rect => Kind::Rect { rect },
            Tool::Highlight => Kind::Highlight { rect },
            Tool::Arrow => Kind::Arrow { a: start, b: cur },
            Tool::Line => Kind::Line { a: start, b: cur },
            Tool::Length => Kind::Measure { pts: vec![start, cur], area: false, name: String::new(), value: String::new(), size },
            Tool::Area => Kind::Measure { pts: annot::rect_pts(rect), area: true, name: String::new(), value: String::new(), size },
            Tool::Calibrate => Kind::Line { a: start, b: cur },
            Tool::Polyline => Kind::Poly { pts: vec![start, cur], closed: false },
            Tool::Polygon => Kind::Poly { pts: annot::rect_pts(rect), closed: true },
            Tool::ScaleRegion => Kind::Rect { rect },
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
        let fs = (size * z * k.0).max(6.0);
        let mut text = text_of(a).to_string();
        let needs_focus = std::mem::take(&mut self.edit_needs_focus);
        for (i, k) in [Key::B, Key::I, Key::U].into_iter().enumerate() {
            if ctx.input_mut(|inp| inp.consume_key(egui::Modifiers::COMMAND, k)) {
                self.toggle_format(i);
            }
        }
        // Formatting may have just changed (B / I / U): lay out with it. The sheet
        // leaves out the note's underline while it's typed, so the editor draws it.
        let style = self.find(id).map(|a| a.style.clone()).unwrap_or_default();
        let ink = Color32::from_rgb(color[0], color[1], color[2]);
        let font = FontId::new(fs, helv_styled(style.bold, style.italic));
        let format = egui::TextFormat {
            font_id: font.clone(),
            color: ink,
            underline: if style.underline { Stroke::new((fs * 0.06).max(1.0), ink) } else { Stroke::NONE },
            ..Default::default()
        };
        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
            let mut job = egui::text::LayoutJob::single_section(buf.as_str().to_owned(), format.clone());
            job.wrap.max_width = wrap;
            ui.painter().layout_job(job)
        };
        let resp = egui::Area::new(egui::Id::new("greenline-edit"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                // Areas default to a narrow max width; wrap exactly at the box's text width.
                let w = width.max(20.0);
                ui.set_min_width(w);
                ui.set_max_width(w);
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .id(egui::Id::new(("greenline-edit", self.generation, id)))
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .desired_width(width.max(20.0))
                        .desired_rows(1)
                        // Its row height sizes the editor and the caret, even with a layouter.
                        .font(font)
                        .layouter(&mut layouter),
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
            if ui.button("New").on_hover_text("A blank document to mark up (Ctrl+N)").clicked() {
                self.new_doc.get_or_insert_with(NewDoc::default);
            }
            if ui.button("Open").on_hover_text("Ctrl+O").clicked() {
                self.ask = Some(Pick::Open);
            }
            ui.add_enabled_ui(self.path.is_some(), |ui| {
                ui.menu_button("Export ⏷", |ui| {
                    if ui.button("Flattened PDF   (Ctrl+E)").on_hover_text("Markups burned in → <name>_markup.pdf").clicked() {
                        self.export(ExportMode::Flatten, false);
                        ui.close();
                    }
                    if ui.button("Flattened PDF as…   (Ctrl+Shift+E)").clicked() {
                        self.export(ExportMode::Flatten, true);
                        ui.close();
                    }
                    if ui.button("Editable PDF   (Ctrl+Alt+E)").on_hover_text("Each markup a movable comment in Bluebeam/Acrobat → <name>_markup_editable.pdf").clicked() {
                        self.export(ExportMode::Editable, false);
                        ui.close();
                    }
                    if ui.button("Comment list (CSV)").on_hover_text("<name>_comments.csv: page, type, comment").clicked() {
                        self.export_csv();
                        ui.close();
                    }
                    if ui.button("Measurements (CSV)").on_hover_text("<name>_measurements.csv: page, name, length, area, width, height").clicked() {
                        self.export_measurements();
                        ui.close();
                    }
                });
            });
            ui.separator();
            // Panels in one menu, so the bar stays a single row on normal windows.
            ui.menu_button("Panels ⏷", |ui| {
                if ui.selectable_label(self.show_sidebar, "Pages   (S)").on_hover_text("Thumbnail sidebar").clicked() {
                    self.show_sidebar = !self.show_sidebar;
                    ui.close();
                }
                if ui.selectable_label(self.overview, "Overview   (O)").on_hover_text("All pages at a glance").clicked() && !self.pages.is_empty() {
                    self.toggle_overview();
                    ui.close();
                }
                if ui.selectable_label(self.show_markups, "Markups   (M)").on_hover_text("List of markups").clicked() {
                    self.toggle_right_panel(false);
                    ui.close();
                }
                if ui.selectable_label(self.show_stamps, "Stamps   (I)").on_hover_text("Seals, signatures and dates").clicked() {
                    self.toggle_right_panel(true);
                    ui.close();
                }
                let n = self.props.count();
                let review = if n > 0 { format!("Review   (Y)  ·  {n}") } else { "Review   (Y)".to_string() };
                if ui.selectable_label(self.show_review, review).on_hover_text("Markups proposed by agents, to accept or reject").clicked() {
                    self.show_review = !self.show_review;
                    ui.close();
                }
            });
            // The active tool labels its menu, so what clicks will draw stays in sight.
            let label = match self.tool {
                Tool::Calibrate => "Calibrate",
                Tool::ScaleRegion => "Scale region",
                t => TOOLS.iter().find(|(x, ..)| *x == t).map_or("Select", |(_, label, _)| *label),
            };
            ui.menu_button(format!("{label} ⏷"), |ui| {
                for (t, label, k) in TOOLS {
                    if t == Tool::Length {
                        ui.separator();
                    }
                    let key = if t == Tool::Polygon { format!("Shift+{}", k.name()) } else { k.name().to_string() };
                    if ui.selectable_label(self.tool == t, format!("{label}   ({key})")).clicked() {
                        self.set_tool(t);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.add_enabled(!self.pages.is_empty(), egui::Button::new("Insert image…")).on_hover_text("A photo or sketch as a markup (or drop a PNG/JPEG on a sheet)").clicked() {
                    self.ask = Some(Pick::Picture);
                    ui.close();
                }
                if ui.selectable_label(self.scale_win.open, "Scale   (U)").on_hover_text("Set the drawing scale for measuring").clicked() {
                    self.scale_win.open = !self.scale_win.open;
                    ui.close();
                }
            })
            .response
            .on_hover_text("Markup tool: the button shows the active one");
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
            self.fill_button(ui);
            ui.separator();
            let mut w = self.width;
            if ui.add(egui::DragValue::new(&mut w).range(0.5..=12.0).speed(0.1).prefix("Line ").suffix(" pt")).on_hover_text("[ / ]").changed() {
                self.set_width(w);
            }
            self.line_type_button(ui);
            let cur_size = self.lead_text_size().unwrap_or(self.text_size);
            egui::ComboBox::from_id_salt("tsize").width(56.0).selected_text(format!("Text {cur_size} pt")).show_ui(ui, |ui| {
                for s in TEXT_SIZES {
                    if ui.selectable_label(s == cur_size, format!("{s} pt")).clicked() {
                        self.set_text_size(s);
                    }
                }
            });
            let fmt = self.shown_format();
            let labels = [
                (egui::RichText::new("B").strong(), "Bold (Ctrl+B)"),
                (egui::RichText::new("I").italics(), "Italic (Ctrl+I)"),
                (egui::RichText::new("U").underline(), "Underline (Ctrl+U)"),
            ];
            for (i, (label, tip)) in labels.into_iter().enumerate() {
                if ui.add(egui::Button::selectable(fmt[i], label).min_size(vec2(20.0, 0.0))).on_hover_text(tip).clicked() {
                    self.toggle_format(i);
                }
            }
        });
    }

    /// Fill swatch: opens a picker for the fill colour (or none) and its opacity.
    fn fill_button(&mut self, ui: &mut Ui) {
        let shown = self.shown_fill();
        let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 18.0), Sense::click());
        let r = rect.shrink2(vec2(4.0, 2.0));
        let edge = ui.visuals().widgets.inactive.fg_stroke.color;
        paint_swatch(ui.painter(), r, shown);
        ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        let resp = resp.on_hover_text(match shown {
            Some(f) => format!("Fill: {} at {}% — for shapes and text boxes", fill_name(f.color), (f.alpha as f32 / 2.55).round()),
            None => "Fill: none — for shapes and text boxes".into(),
        });
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.label("Fill");
            ui.horizontal(|ui| {
                let (r, none) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                paint_swatch(ui.painter(), r.shrink(2.0), None);
                if shown.is_none() {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
                }
                if none.on_hover_text("None").clicked() {
                    self.set_fill(None);
                }
                for i in 0..=COLORS.len() {
                    let c = fill_color(i);
                    let (r, sw) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    paint_swatch(ui.painter(), r.shrink(2.0), Some(Fill { color: c, alpha: 255 }));
                    ui.painter().rect_stroke(r.shrink(2.0), 3.0, Stroke::new(1.0, edge.gamma_multiply(0.5)), egui::StrokeKind::Inside);
                    if shown.is_some_and(|f| f.color == c) {
                        ui.painter().rect_stroke(r, 3.0, Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
                    }
                    if sw.on_hover_text(fill_name(c)).clicked() {
                        self.set_fill(Some(i));
                    }
                }
            });
            // While dragging, the slider shows the value it will set on release (one undo step per drag).
            let mut pct = self.fill_pending.unwrap_or_else(|| (shown.map_or(self.fill_alpha, |f| f.alpha) as f32 / 2.55).round());
            let slider = ui.add_enabled(shown.is_some(), egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").text("Opacity"));
            if slider.changed() {
                self.fill_pending = Some(pct);
            }
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                if let Some(p) = self.fill_pending.take() {
                    self.set_fill_alpha((p * 2.55).round() as u8);
                }
            }
            ui.separator();
            self.hatch_picker(ui);
        });
    }

    /// Hatch list in the fill picker: none, then each library hatch with a sample.
    fn hatch_picker(&mut self, ui: &mut Ui) {
        let shown = self.shown_hatch();
        ui.horizontal(|ui| {
            ui.label("Hatch");
            if let Some(h) = &shown {
                // Shows the value being dragged; applied on release, as one undo step.
                let mut s = self.hatch_scale_pending.unwrap_or(h.scale);
                let r = ui.add(egui::DragValue::new(&mut s).range(0.1..=20.0).speed(0.02).prefix("Scale ").suffix("×"));
                if r.changed() {
                    self.hatch_scale_pending = Some(s);
                }
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    if let Some(s) = self.hatch_scale_pending.take() {
                        self.set_hatch(Some(HatchFill { pattern: h.pattern.clone(), scale: s }));
                    }
                }
            }
        });
        let dense = self.selected.iter().filter_map(|&id| self.find(id)).any(|a| {
            a.hatchable() && a.style.hatch.as_ref().is_some_and(|h| patterns::too_dense(a.bounds(), &h.pattern, h.scale))
        });
        if dense {
            ui.colored_label(ui.visuals().warn_fg_color, "Too fine for a shape this big: raise the scale");
        }
        let mut pick: Option<Option<Arc<Hatch>>> = None;
        egui::ScrollArea::vertical().id_salt("hatches").max_height(260.0).show(ui, |ui| {
            if ui.selectable_label(shown.is_none(), "None").clicked() {
                pick = Some(None);
            }
            let mut last_set: Option<&str> = None;
            for (set, h) in &self.patterns.hatches {
                if last_set != Some(set.as_str()) && !set.is_empty() {
                    ui.weak(set);
                }
                last_set = Some(set.as_str());
                let on = shown.as_ref().is_some_and(|s| *s.pattern == **h);
                let row = ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(28.0, 18.0), Sense::hover());
                    paint_hatch_sample(ui.painter(), r, h, &self.textures);
                    ui.selectable_label(on, h.name.trim())
                });
                if row.inner.clicked() {
                    pick = Some(Some(h.clone()));
                }
            }
        });
        if ui.button("Import hatches (.bhx)…").on_hover_text("Add Bluebeam hatch files to your own library. A shared folder can be set in F1.").clicked() {
            self.ask = Some(Pick::Patterns);
        }
        if let Some(p) = pick {
            let scale = shown.map_or(1.0, |s| s.scale);
            self.set_hatch(p.map(|pattern| HatchFill { pattern, scale }));
        }
    }

    /// Line type button beside the width: a sample of the current one; opens the list.
    fn line_type_button(&mut self, ui: &mut Ui) {
        let shown = self.shown_line_type();
        let ink = ui.visuals().strong_text_color();
        let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 18.0), Sense::click());
        paint_line_sample(ui.painter(), rect.shrink2(vec2(3.0, 0.0)), shown.as_deref(), ink, &self.textures);
        if resp.hovered() {
            ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color), egui::StrokeKind::Inside);
        }
        let resp = resp.on_hover_text(format!("Line type: {}", shown.as_ref().map_or("solid", |l| l.name.as_str())));
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            let mut pick: Option<Option<Arc<LineType>>> = None;
            egui::ScrollArea::vertical().id_salt("linetypes").max_height(360.0).show(ui, |ui| {
                let row = |ui: &mut Ui, lt: Option<&LineType>, name: &str, on: bool| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(90.0, 18.0), Sense::hover());
                        paint_line_sample(ui.painter(), r, lt, ink, &self.textures);
                        ui.selectable_label(on, name).clicked()
                    })
                    .inner
                };
                if row(ui, None, "Solid", shown.is_none()) {
                    pick = Some(None);
                }
                let mut last_set: Option<&str> = None;
                for (set, lt) in &self.patterns.line_types {
                    if last_set != Some(set.as_str()) && !set.is_empty() {
                        ui.weak(set);
                    }
                    last_set = Some(set.as_str());
                    if row(ui, Some(lt), &lt.name, shown.as_ref().is_some_and(|s| **s == **lt)) {
                        pick = Some(Some(lt.clone()));
                    }
                }
            });
            if ui.button("Import line types (.blx)…").on_hover_text("Add Bluebeam line type files to your own library. A shared folder can be set in F1.").clicked() {
                self.ask = Some(Pick::Patterns);
                ui.close();
            }
            if let Some(p) = pick {
                self.set_line_type(p);
                ui.close();
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, now: f64) {
        if self.status_until.is_nan() {
            self.status_until = now + 5.0;
        }
        ui.horizontal(|ui| {
            // The help button holds the right edge; the path truncates before it.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("?").on_hover_text("Shortcuts (F1)").clicked() {
                    self.show_help = !self.show_help;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if !self.pages.is_empty() {
                        ui.label(format!("Page {} / {}", self.current_page() + 1, self.pages.len()));
                        ui.separator();
                        ui.add_enabled_ui(!self.overview, |ui| {
                            ui.menu_button(format!("{:.0}% ⏷", self.zoom * 100.0), |ui| {
                                if ui.button("Fit width   (W)").clicked() {
                                    self.fit_width();
                                    ui.close();
                                }
                                if ui.button("Fit page   (F)").clicked() {
                                    self.fit_page();
                                    ui.close();
                                }
                                if ui.button("Actual size").clicked() {
                                    self.set_zoom(1.0, self.canvas.center());
                                    ui.close();
                                }
                                if ui.add_enabled(!self.selected.is_empty(), egui::Button::new("Zoom to selection   (Z)")).clicked() {
                                    self.zoom_to_selection();
                                    ui.close();
                                }
                                ui.separator();
                                for pct in [50, 100, 200, 400] {
                                    if ui.button(format!("{pct}%")).clicked() {
                                        self.set_zoom(pct as f32 / 100.0, self.canvas.center());
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Zoom: fit width, fit page, actual size or a preset");
                        });
                        ui.separator();
                        let page = self.current_page();
                        let scale = self.scales.iter().find(|s| s.page == page && s.region.is_none()).map_or("No scale".to_string(), |s| s.label.clone());
                        if ui.link(scale).on_hover_text("Drawing scale for measuring (U)").clicked() {
                            self.scale_win.open = true;
                        }
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
                        let shown = p.display().to_string();
                        ui.add(egui::Label::new(egui::RichText::new(&shown).weak()).truncate()).on_hover_text(shown);
                    }
                });
            });
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
        egui::Area::new(egui::Id::new("greenline-goto"))
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
            painter.rect_stroke(r.expand(2.0), 2.0, Stroke::new(2.5, crate::theme::accent(ui.visuals())), egui::StrokeKind::Outside);
        }
        let n = self.markup_count(i);
        if n > 0 {
            let c = pos2(r.max.x - 9.0, r.min.y + 9.0);
            painter.circle_filled(c, 9.0, crate::theme::accent(ui.visuals()));
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
        order.sort_by_key(|a| page_order(a));
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
        order.sort_by_key(|a| page_order(a));
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
        egui::Window::new(format!("Greenline {} — shortcuts", env!("CARGO_PKG_VERSION"))).open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("keys").striped(true).show(ui, |ui| {
                let rows = [
                    ("C", "Callout: press on the target, drag to where the note goes (click = quick note)"),
                    ("T", "Text box (click, or drag for width)"),
                    ("K / R / H", "Revision cloud / rectangle / highlight (drag)"),
                    ("A / L / P", "Arrow / line / pen"),
                    ("N / Shift+N", "Polyline / polygon: click the corners, double-click or Enter to finish (a polygon also closes on its first point)"),
                    ("D / E", "Measure length / area: click the points, double-click or Enter to finish (drag: one segment / a rectangle)"),
                    ("Y", "Review: markups proposed by agents (greenline-cli); accept or reject each, or a whole batch"),
                    ("U", "Scale: pick the drawing scale, use one written on the sheet, calibrate, or add a region for a detail"),
                    ("V or Esc", "Select: drag to move, handles to resize, double-click or Enter to edit text"),
                    ("Ctrl/Shift+click", "Add to or remove from selection (also in the markup list)"),
                    ("Ctrl+click, callout selected", "Add another leader, arrowhead at the click; its tail meets the first leader's bend"),
                    ("Drag on empty page", "Select every markup the box crosses (Ctrl/Shift adds)   ·   Ctrl+A all on page"),
                    ("Esc / Ctrl+Enter", "Finish typing"),
                    ("Ctrl+B / I / U", "Bold / italic / underline the note being typed or the selected notes (also the B I U buttons)"),
                    ("1–6", "Colour (applies to all selected)   ·   fill swatch beside the colours: fill, opacity and hatch"),
                    ("Line sample beside the width", "Line type: dashed, dotted, or one from the pattern library (set its folder below)"),
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
                    ("Z", "Zoom to the selected markups"),
                    ("Ctrl+Tab / Ctrl+W", "Next tab (Shift: previous; also Ctrl+PgDn/PgUp) / close tab — opening another PDF opens it in a new tab; drag a tab to move it"),
                    ("Ctrl+N", "New blank document: pick the page size and count, then mark up or drop images on it"),
                    ("Drop an image", "A PNG or JPEG dropped on a sheet is placed there as a markup (also tool menu ▸ Insert image…)"),
                    ("Ctrl+F", "Find text in the document: Enter / F3 next, Shift+Enter / Shift+F3 previous, Esc closes"),
                    ("Zoom % (status bar)", "Click for fit width, fit page, actual size and preset zooms"),
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
            ui.horizontal(|ui| {
                ui.label("Appearance:");
                for (t, name) in [(egui::ThemePreference::System, "System"), (egui::ThemePreference::Light, "Light"), (egui::ThemePreference::Dark, "Dark")] {
                    if ui.selectable_value(&mut self.prefs.theme, t, name).changed() {
                        ctx.set_theme(t);
                        self.prefs_dirty = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Pattern library folder:");
                match &self.prefs.pattern_dir {
                    Some(d) => ui.monospace(d.display().to_string()),
                    None => ui.weak("none"),
                };
                if ui.button("Choose…").on_hover_text("A folder of Bluebeam line type (.blx) and hatch (.bhx) files, such as your firm's shared standards").clicked() {
                    self.ask = Some(Pick::PatternDir);
                }
                if self.prefs.pattern_dir.is_some() && ui.button("Clear").clicked() {
                    self.prefs.pattern_dir = None;
                    self.prefs_dirty = true;
                    self.reload_patterns();
                }
                if ui.button("Reload").on_hover_text("Read the library again after its files change").clicked() {
                    self.reload_patterns();
                }
            });
            let (nl, nh) = (self.patterns.line_types.len(), self.patterns.hatches.len());
            ui.weak(format!("{nl} line types and {nh} hatches; your own imports are kept in {}.", patterns::own_dir().display()));
            for p in self.patterns.problems.iter().take(3) {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Couldn't read {p}"));
            }
            ui.weak(format!("Markups autosave to {} as you go; the original PDF is never modified.", sidecar::dir().display()));
            ui.weak(format!("Greenline {}", env!("CARGO_PKG_VERSION")));
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
        while let Ok((stored, target, generation)) = self.pictures.1.try_recv() {
            self.picture_ready(stored, target, generation);
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            let at = ctx.input(|i| i.pointer.hover_pos());
            for p in dropped {
                // An image dropped on a sheet is placed there; each PDF opens in a tab.
                if stamps::is_picture(&p) && !self.pages.is_empty() {
                    self.place_picture(&p, at);
                } else {
                    self.open(p);
                }
            }
        }

        // Measurements read from the scales as they are now (cheap: measurements only).
        measure::relabel(&mut self.annots, &self.scales, self.prefs.units);
        self.relabel_proposals();
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        if self.tabs.len() > 1 {
            egui::Panel::top("tabs").show(ui, |ui| self.tab_strip(ui));
        }
        if std::mem::take(&mut self.title_dirty) {
            let title = if self.path.is_some() { format!("{} — Greenline", self.file_name()) } else { "Greenline".into() };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, now));
        let has_doc = !self.pages.is_empty();
        let sheets = crate::theme::sheets(ui.visuals());
        if has_doc && self.show_sidebar && !self.overview {
            egui::Panel::left("thumbs")
                .resizable(true)
                .default_size(170.0)
                .size_range(90.0..=420.0)
                .frame(egui::Frame::side_top_panel(ui.style()).fill(sheets))
                .show(ui, |ui| self.sidebar(ui));
        }
        self.load_textures(&ctx);
        self.poll_proposals(now);
        // The review row under the pointer is set while its panel draws; start
        // each frame clear so a closed panel leaves no hung highlight.
        self.props_hover = None;
        if self.show_review {
            egui::Panel::right("review").resizable(true).default_size(300.0).size_range(220.0..=600.0).frame(egui::Frame::side_top_panel(ui.style()).fill(sheets)).show(ui, |ui| self.review_panel(ui));
        } else if self.show_stamps {
            egui::Panel::right("stamps").resizable(true).default_size(280.0).size_range(220.0..=600.0).frame(egui::Frame::side_top_panel(ui.style()).fill(sheets)).show(ui, |ui| self.stamp_panel(ui));
        } else if self.show_markups {
            egui::Panel::right("markups").resizable(true).default_size(280.0).size_range(180.0..=600.0).frame(egui::Frame::side_top_panel(ui.style()).fill(sheets)).show(ui, |ui| self.markup_list(ui));
        }
        if self.overview && has_doc {
            self.shortcuts(&ctx);
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).fill(sheets)).show(ui, |ui| self.overview_grid(ui));
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
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, &self.status, FontId::proportional(18.0), ui.visuals().text_color());
            }
        });
        }
        self.text_editor(&ctx);
        self.scale_window(&ctx);
        self.goto_overlay(&ctx);
        self.find_bar(&ctx);
        self.new_doc_window(&ctx);
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
        self.save_parked(now);
        // Background tabs still unsaved: keep retrying even when nothing else wakes the window.
        if self.tabs.iter().filter_map(|t| t.parked.as_ref()).any(|p| p.dirty && !p.read_only) {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
        // Wake once a second to notice proposals an agent adds.
        if self.path.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        if now < self.status_until && self.status_until.is_finite() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(self.status_until - now));
        }
    }

    fn on_exit(&mut self) {
        self.commit_edit();
        self.save_sidecar();
        // Background tabs were saved when parked; one whose save failed then gets another go.
        for i in 0..self.tabs.len() {
            if self.tabs[i].parked.as_ref().is_some_and(|p| p.dirty && !p.read_only) {
                self.switch_tab(i);
                self.save_sidecar();
            }
        }
    }
}

/// `11`, `8.5`: a sheet dimension without trailing zeros.
fn trim_num(v: f32) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// Sort key for lists and CSVs: page, then top to bottom, left to right.
fn page_order(a: &Annot) -> (usize, i64, i64) {
    let b = a.bounds();
    (a.page, b.min.y as i64, b.min.x as i64)
}

fn cross(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b - a).x * (c - b).y - (b - a).y * (c - b).x
}

/// Whether every turn of the closed outline goes the same way.
fn convex(pts: &[Pos2]) -> bool {
    let n = pts.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let c = cross(pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        if c.abs() > 1e-3 {
            if sign != 0.0 && c.signum() != sign {
                return false;
            }
            sign = c.signum();
        }
    }
    true
}

/// Ear-clipping triangulation of a simple polygon (vertex indices). `None`
/// for one that crosses itself.
fn triangulate(pts: &[Pos2]) -> Option<Vec<[usize; 3]>> {
    let n = pts.len();
    let area: f32 = (0..n).map(|i| pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y).sum();
    let ccw = area > 0.0;
    let in_tri = |p: Pos2, a: Pos2, b: Pos2, c: Pos2| {
        let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tris = Vec::new();
    while idx.len() > 3 {
        let m = idx.len();
        let ear = (0..m).find_map(|i| {
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let t = cross(pts[a], pts[b], pts[c]);
            if t.abs() < 1e-6 {
                return Some((i, None)); // a straight-through point: drop it
            }
            let ok = (t > 0.0) == ccw && !idx.iter().any(|&j| j != a && j != b && j != c && in_tri(pts[j], pts[a], pts[b], pts[c]));
            ok.then_some((i, Some([a, b, c])))
        })?;
        tris.extend(ear.1);
        idx.remove(ear.0);
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    Some(tris)
}

type Triangles = std::rc::Rc<Vec<[usize; 3]>>;

/// `triangulate` remembered by outline, so a filled cloud or polygon isn't
/// re-triangulated every frame (the triangles don't change with pan or zoom).
fn cached_triangles(pts: &[Pos2]) -> Option<Triangles> {
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<HashMap<u64, Option<Triangles>>> = RefCell::new(HashMap::new());
    }
    let bytes: Vec<u8> = pts.iter().flat_map(|p| [p.x.to_bits().to_le_bytes(), p.y.to_bits().to_le_bytes()]).flatten().collect();
    let key = sidecar::fnv(&bytes);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 2000 {
            c.clear();
        }
        c.entry(key).or_insert_with(|| triangulate(pts).map(std::rc::Rc::new)).clone()
    })
}

/// `p` moved onto the horizontal or vertical through `from`, whichever is nearer.
fn square(from: Pos2, p: Pos2) -> Pos2 {
    let d = p - from;
    if d.x.abs() >= d.y.abs() { pos2(p.x, from.y) } else { pos2(from.x, p.y) }
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
            Prim::Stroke { pts, width, color, .. } if pts.len() == 2 && pts[0] == pts[1] => {
                // A dot in a dotted line.
                painter.circle_filled(to(pts[0]), (width * z / 2.0).max(min_w / 2.0), col(*color, 255));
            }
            Prim::Segments { segs, offset, width, color, tile, tint } => {
                // A hatch too fine to make out at this zoom: a tint of its ink instead
                // (also far cheaper than thousands of sub-pixel lines).
                if tile * z < 4.0 {
                    if let Some((outline, alpha)) = tint {
                        let pts = map(outline);
                        let c = col(*color, *alpha);
                        if convex(&pts) {
                            painter.add(Shape::convex_polygon(pts, c, Stroke::NONE));
                        } else if let Some(tris) = cached_triangles(outline) {
                            let mut mesh = egui::Mesh::default();
                            for p in &pts {
                                mesh.colored_vertex(*p, c);
                            }
                            for [a, b, c] in tris.iter() {
                                mesh.add_triangle(*a as u32, *b as u32, *c as u32);
                            }
                            painter.add(Shape::mesh(mesh));
                        }
                    }
                    continue;
                }
                let w = (width * z).max(0.6);
                let c = col(*color, 255);
                let s = Stroke::new(w, c);
                let view = painter.clip_rect();
                let o = to(Pos2::ZERO + *offset);
                for [a, b] in segs.iter() {
                    let (a, b) = (o + a.to_vec2() * z, o + b.to_vec2() * z);
                    if !view.intersects(Rect::from_two_pos(a, b).expand(w)) {
                        continue;
                    }
                    if a == b {
                        painter.rect_filled(Rect::from_center_size(a, Vec2::splat(w)), 0.0, c);
                    } else {
                        painter.line_segment([a, b], s);
                    }
                }
            }
            Prim::Stroke { pts, closed, width, color } => {
                let s = Stroke::new((width * z).max(min_w), col(*color, 255));
                let pts = map(pts);
                if *closed {
                    painter.add(Shape::closed_line(pts, s));
                } else {
                    painter.add(Shape::line(pts, s));
                }
            }
            Prim::Fill { pts: page_pts, color, alpha, outline } => {
                let stroke = outline.map(|(c, w)| Stroke::new((w * z).max(min_w), col(c, 255))).unwrap_or(Stroke::NONE);
                let pts = map(page_pts);
                if convex(&pts) {
                    painter.add(Shape::convex_polygon(pts, col(*color, *alpha), stroke));
                } else {
                    // An L- or U-shaped area or a cloud: egui fills convex outlines only, so fill triangles.
                    if let Some(tris) = cached_triangles(page_pts) {
                        let mut mesh = egui::Mesh::default();
                        for p in &pts {
                            mesh.colored_vertex(*p, col(*color, *alpha));
                        }
                        for [a, b, c] in tris.iter() {
                            mesh.add_triangle(*a as u32, *b as u32, *c as u32);
                        }
                        painter.add(Shape::mesh(mesh));
                    }
                    if stroke != Stroke::NONE {
                        painter.add(Shape::closed_line(pts, stroke));
                    }
                }
            }
            Prim::Text { origin: o, text, size, color, bold, italic } => {
                let fs = size * z * k.0;
                let o = to(*o);
                if fs < 3.0 {
                    // Too small to read: a grey bar where the line of text is.
                    let w = annot::text_width_b(text, *size, *bold) * z;
                    painter.line_segment([o - vec2(0.0, fs * 0.3), o + vec2(w, -fs * 0.3)], Stroke::new(fs.max(0.7), col(*color, 120)));
                    continue;
                }
                painter.text(o - vec2(0.0, k.1 * size * z), Align2::LEFT_TOP, text, FontId::new(fs, helv_styled(*bold, *italic)), col(*color, 255));
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

/// A short sample of line type `lt` (solid when `None`) across `r`.
fn paint_line_sample(p: &Painter, r: Rect, lt: Option<&LineType>, ink: Color32, images: &HashMap<String, TextureHandle>) {
    let c = [ink.r(), ink.g(), ink.b()];
    let y = r.center().y;
    // Symbols are drawn in points; fit the tallest across the row's height.
    let z = 1.0;
    let mut prims = Vec::new();
    let path = [pos2(0.0, 0.0), pos2(r.width() / z, 0.0)];
    match lt {
        Some(lt) => patterns::pattern_prims(&mut prims, &path, false, lt, 1.0, c),
        None => prims.push(Prim::Stroke { pts: path.to_vec(), closed: false, width: 1.0, color: c }),
    }
    let p = p.with_clip_rect(r);
    paint_prims(&p, pos2(r.min.x, y), z, &prims, (1.0, 0.0), 1.0, images);
}

/// A swatch of hatch `h` filling `r`, at a size that shows its pattern.
fn paint_hatch_sample(p: &Painter, r: Rect, h: &Hatch, images: &HashMap<String, TextureHandle>) {
    p.rect_filled(r, 2.0, Color32::WHITE);
    // Show about two tiles across.
    let z = r.width() / (2.0 * h.tile.x.max(1.0));
    let outline = annot::rect_pts(Rect::from_min_size(Pos2::ZERO, r.size() / z));
    let (groups, offset) = patterns::cached_hatch(&outline, h, 1.0);
    let prims: Vec<Prim> = groups.iter().map(|(w, segs)| Prim::Segments { segs: segs.clone(), offset, width: *w, color: [40, 40, 40], tile: f32::INFINITY, tint: None }).collect();
    paint_prims(&p.with_clip_rect(r), r.min, z, &prims, (1.0, 0.0), 1.0, images);
    p.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Inside);
}

/// Lowest fill opacity the slider sets (5%); a fill is removed with None.
const MIN_FILL_ALPHA: u8 = 13;

/// Fill colours: the markup colours, then white.
fn fill_color(i: usize) -> Rgb {
    COLORS.get(i).map_or(WHITE, |c| c.0)
}

fn fill_name(c: Rgb) -> &'static str {
    COLORS.iter().find(|x| x.0 == c).map_or(if c == WHITE { "White" } else { "Custom" }, |x| x.1)
}

/// A fill swatch: the colour at its opacity over a light checker, or a red slash for none.
fn paint_swatch(p: &Painter, r: Rect, fill: Option<Fill>) {
    p.rect_filled(r, 3.0, Color32::WHITE);
    match fill {
        Some(f) => {
            let h = r.height() / 2.0;
            let mut x = r.min.x;
            let mut odd = false;
            while x < r.max.x && f.alpha < 255 {
                let w = h.min(r.max.x - x);
                p.rect_filled(Rect::from_min_size(pos2(x, if odd { r.min.y } else { r.min.y + h }), vec2(w, h)), 0.0, Color32::from_gray(205));
                x += h;
                odd = !odd;
            }
            p.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(f.color[0], f.color[1], f.color[2], f.alpha));
        }
        None => {
            p.line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.5, Color32::from_rgb(220, 0, 0)));
        }
    }
}

/// Default name on editable-export comments when none is set in Shortcuts:
/// $GREENLINE_AUTHOR, else the account's full name (Linux /etc/passwd), else the login name.
fn default_export(src: &std::path::Path, mode: ExportMode) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let suffix = if mode == ExportMode::Editable { "_markup_editable" } else { "_markup" };
    src.with_file_name(format!("{stem}{suffix}.pdf"))
}

fn default_author() -> String {
    if let Ok(a) = std::env::var("GREENLINE_AUTHOR") {
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
        Kind::Measure { .. } => a.label(),
        _ => a.text(),
    }
}

/// The first markup id after every markup's and every accept receipt's (an
/// undone accept's id must not come back for something else).
fn first_free_id(annots: &[Annot], accepted: &[String]) -> u64 {
    let receipts = accepted.iter().filter_map(|k| k.rsplit_once(':').and_then(|(_, n)| n.parse::<u64>().ok()));
    annots.iter().map(|a| a.id).chain(receipts).max().map_or(1, |m| m + 1)
}

/// Text size of a note, or of a measurement's value label.
fn label_size(a: &Annot) -> Option<f32> {
    match &a.kind {
        Kind::Measure { size, .. } => Some(*size),
        _ => a.text_size(),
    }
}

/// What the sheet draws of a note being typed: its box and leaders, but not its
/// text or underline, which the editor draws where they now fall.
fn typed_note_prims(a: &Annot) -> Vec<Prim> {
    let mut prims = if a.style.underline {
        let mut plain = a.clone();
        plain.style.underline = false;
        plain.prims()
    } else {
        a.prims()
    };
    prims.retain(|p| !matches!(p, Prim::Text { .. }));
    prims
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

/// Markup text family for a style: the styled face where the system has one,
/// falling back to the plain family.
fn helv_styled(bold: bool, italic: bool) -> FontFamily {
    match (bold, italic) {
        (false, false) => helv(),
        (true, false) => FontFamily::Name("helv-b".into()),
        (false, true) => FontFamily::Name("helv-i".into()),
        (true, true) => FontFamily::Name("helv-bi".into()),
    }
}

/// Markup text in the same fonts and order export uses (Liberation Sans, then
/// the Noto fallbacks in `fonts`), so what's on screen is what gets embedded.
/// UI text is Montserrat (`theme`).
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut family = Vec::new();
    for (i, face) in crate::fonts::faces().iter().enumerate() {
        let name = format!("sys{i}");
        fonts.font_data.insert(name.clone(), std::sync::Arc::new(egui::FontData::from_owned(face.data().to_vec())));
        family.push(name);
    }
    family.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    for (b, i) in [(true, false), (false, true), (true, true)] {
        let mut styled = Vec::new();
        if let Some(face) = crate::fonts::styled(b, i) {
            let name = format!("styled-{b}-{i}");
            fonts.font_data.insert(name.clone(), std::sync::Arc::new(egui::FontData::from_owned(face.data().to_vec())));
            styled.push(name);
        }
        styled.extend(family.iter().cloned());
        fonts.families.insert(helv_styled(b, i), styled);
    }
    fonts.families.insert(helv(), family);
    crate::theme::add_font(&mut fonts);
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

    fn callout(app: &mut App, anchor: Pos2, pos: Pos2) -> u64 {
        app.add(0, Kind::Callout { anchor, pos, box_w: 120.0, text: "Check".into(), size: 12.0, leaders: vec![] })
    }

    fn leaders(app: &App, id: u64) -> Vec<Pos2> {
        match &app.find(id).unwrap().kind {
            Kind::Callout { leaders, .. } => leaders.clone(),
            _ => panic!("not a callout"),
        }
    }

    const PLAIN: Mods = Mods { add: false, single: false, ctrl: false, shift: false };
    const ADD: Mods = Mods { add: true, single: false, ctrl: false, shift: false };
    const ALT: Mods = Mods { add: false, single: true, ctrl: false, shift: false };
    const CTRL: Mods = Mods { add: true, single: false, ctrl: true, shift: false };

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
    fn ctrl_click_adds_callout_leaders_and_their_tips_drag() {
        let mut app = app();
        let id = callout(&mut app, pos2(100.0, 100.0), pos2(300.0, 300.0));
        let r = rect(&mut app, 500.0, 500.0);
        app.selected = vec![id];
        app.click(pos2(80.0, 80.0), false, CTRL);
        assert_eq!(leaders(&app, id), vec![pos2(80.0, 80.0)]);
        app.click(pos2(200.0, 60.0), false, CTRL);
        assert_eq!(leaders(&app, id), vec![pos2(80.0, 80.0), pos2(200.0, 60.0)], "a third leader is fine");
        app.click(pos2(50.0, 50.0), false, ADD);
        assert_eq!(leaders(&app, id).len(), 2, "Shift/plain clicks on empty space add nothing");
        app.click(pos2(500.0, 550.0), false, CTRL);
        assert_eq!(app.selected, vec![id, r], "Ctrl+click on a markup still toggles the selection");
        assert_eq!(leaders(&app, id).len(), 2);

        app.selected = vec![id];
        let tip = pos2(80.0, 80.0);
        app.begin_drag(tip, false, PLAIN);
        app.update_drag(pos2(90.0, 70.0), vec2(10.0, -10.0));
        app.end_drag();
        assert_eq!(leaders(&app, id)[0], pos2(90.0, 70.0), "the tip handle moves that leader");
        app.undo();
        assert_eq!(leaders(&app, id)[0], tip, "one undo step");
    }

    #[test]
    fn ctrl_click_that_ends_typing_also_adds_a_leader() {
        let mut app = app();
        let id = callout(&mut app, pos2(100.0, 100.0), pos2(300.0, 300.0));
        app.start_edit(id);
        app.click(pos2(80.0, 80.0), false, PLAIN);
        assert!(app.editing.is_none() && app.selected == vec![id], "a plain click just ends typing, keeping the callout selected");
        assert!(leaders(&app, id).is_empty());
        app.start_edit(id);
        app.click(pos2(80.0, 80.0), false, CTRL);
        assert!(app.editing.is_none(), "Ctrl+click ends typing…");
        assert_eq!(leaders(&app, id), vec![pos2(80.0, 80.0)], "…and adds the leader");
    }

    #[test]
    fn ctrl_click_after_placing_with_c_adds_a_leader() {
        let mut app = app();
        app.set_tool(Tool::Callout);
        app.click(pos2(100.0, 100.0), false, PLAIN);
        let id = app.annots.last().unwrap().id;
        assert_eq!(app.editing, Some(id), "a quick note opens the editor");
        app.find_mut(id).unwrap().text_mut().unwrap().push_str("Check");
        app.click(pos2(80.0, 80.0), false, CTRL);
        assert_eq!(leaders(&app, id), vec![pos2(80.0, 80.0)], "Ctrl+click right after placing adds a leader");
        assert_eq!(app.tool, Tool::Select, "ready to drag the new tip");
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
    fn fit_width_and_fit_page_fit_the_canvas() {
        let mut app = app();
        app.zoom = 0.4;
        app.fit_width();
        assert!((app.zoom - (1000.0 - 24.0) / 600.0).abs() < 1e-4);
        let r = app.page_doc_rect(0);
        assert!((app.to_screen(r.min).x - 12.0).abs() < 0.01, "12 px margin on the left");
        assert!((app.to_screen(r.max).x - (1000.0 - 12.0)).abs() < 0.01, "and on the right");

        app.fit_page();
        assert!((app.zoom - (1000.0 - 24.0) / 800.0).abs() < 1e-4, "the height binds");
        assert!((app.to_screen(r.center()) - app.canvas.center()).length() < 0.5, "the page is centred");
    }

    #[test]
    fn set_zoom_keeps_the_point_under_the_screen_position() {
        let mut app = app();
        let at = pos2(250.0, 300.0);
        let before = app.to_doc(at);
        app.set_zoom(3.0, at);
        assert_eq!(app.zoom, 3.0);
        assert!((app.to_doc(at) - before).length() < 1e-3);
        app.set_zoom(1000.0, at);
        assert_eq!(app.zoom, 40.0, "clamped like zoom_at");
        app.set_zoom(0.0, at);
        assert_eq!(app.zoom, 0.02);
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

    fn measures(app: &App) -> Vec<(Vec<Pos2>, bool, String)> {
        app.annots.iter().filter_map(|a| match &a.kind {
            Kind::Measure { pts, area, value, .. } => Some((pts.clone(), *area, value.clone())),
            _ => None,
        }).collect()
    }

    #[test]
    fn length_by_clicks_finishes_on_double_click() {
        let mut app = app();
        app.scales.push(Scale::from_ratio(0, None, 48.0, "1/4\" = 1'-0\""));
        app.set_tool(Tool::Length);
        app.click(pos2(100.0, 100.0), false, PLAIN);
        app.click(pos2(154.0, 100.0), false, PLAIN);
        // Shift squares the next point to the last one.
        app.click(pos2(156.0, 172.0), false, Mods { shift: true, ..PLAIN });
        app.click(pos2(156.0, 172.0), true, PLAIN);
        assert!(app.building.is_none());
        measure::relabel(&mut app.annots, &app.scales, app.prefs.units);
        let m = measures(&app);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, vec![pos2(100.0, 100.0), pos2(154.0, 100.0), pos2(154.0, 172.0)]);
        assert_eq!(m[0].2, "7'-0\"", "3 ft + 4 ft at 1/4\" = 1'-0\"");
        assert_eq!(app.tool, Tool::Length, "stays ready for the next one");
        app.undo();
        assert!(measures(&app).is_empty(), "one undo step");
    }

    #[test]
    fn area_closes_on_its_first_point_and_backspace_takes_one_back() {
        let mut app = app();
        app.set_tool(Tool::Area);
        for p in [pos2(100.0, 100.0), pos2(200.0, 100.0), pos2(300.0, 300.0)] {
            app.click(p, false, PLAIN);
        }
        app.building.as_mut().unwrap().1.pop();
        app.click(pos2(200.0, 200.0), false, PLAIN);
        app.click(pos2(100.0, 200.0), false, PLAIN);
        app.click(pos2(102.0, 101.0), false, PLAIN);
        assert!(app.building.is_none());
        let m = measures(&app);
        assert_eq!(m.len(), 1);
        assert!(m[0].1);
        assert_eq!(m[0].0.len(), 4, "closing click isn't a point");
        // A bow-tie isn't finished: it stays open for Backspace.
        for p in [pos2(300.0, 300.0), pos2(400.0, 400.0), pos2(300.0, 400.0), pos2(400.0, 300.0)] {
            app.click(p, false, PLAIN);
        }
        app.click(pos2(400.0, 300.0), true, PLAIN);
        assert_eq!(measures(&app).len(), 1);
        assert_eq!(app.building.as_ref().map(|b| b.1.len()), Some(4));
        // Changing tools doesn't carry it over to the new one.
        app.set_tool(Tool::Length);
        assert!(app.building.is_none());
        assert_eq!(measures(&app).len(), 1);
        app.set_tool(Tool::Area);
        // Two points isn't an area.
        app.click(pos2(400.0, 400.0), false, PLAIN);
        app.click(pos2(450.0, 400.0), false, PLAIN);
        app.finish_measure();
        assert_eq!(measures(&app).len(), 1);
    }

    fn polys(app: &App) -> Vec<(Vec<Pos2>, bool)> {
        app.annots.iter().filter_map(|a| match &a.kind {
            Kind::Poly { pts, closed } => Some((pts.clone(), *closed)),
            _ => None,
        }).collect()
    }

    #[test]
    fn polylines_and_polygons_are_clicked_out() {
        let mut app = app();
        app.set_tool(Tool::Polyline);
        for p in [pos2(100.0, 100.0), pos2(200.0, 100.0), pos2(200.0, 200.0)] {
            app.click(p, false, PLAIN);
        }
        app.click(pos2(200.0, 200.0), true, PLAIN);
        assert!(app.building.is_none());
        assert_eq!(polys(&app), vec![(vec![pos2(100.0, 100.0), pos2(200.0, 100.0), pos2(200.0, 200.0)], false)]);
        // A polygon closes on its first point and takes the tool's fill.
        app.set_tool(Tool::Polygon);
        app.fill_alpha = 100;
        app.set_fill(Some(1));
        for p in [pos2(300.0, 300.0), pos2(400.0, 300.0), pos2(350.0, 400.0), pos2(301.0, 301.0)] {
            app.click(p, false, PLAIN);
        }
        let ps = polys(&app);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[1], (vec![pos2(300.0, 300.0), pos2(400.0, 300.0), pos2(350.0, 400.0)], true));
        let a = app.annots.last().unwrap().clone();
        assert_eq!(a.style.fill, Some(Fill { color: COLORS[1].0, alpha: 100 }));
        // Filled, so clicking inside selects it; the open polyline's inside doesn't count.
        app.set_tool(Tool::Select);
        app.click(pos2(350.0, 330.0), false, PLAIN);
        assert_eq!(app.selected, vec![a.id]);
        app.click(pos2(150.0, 150.0), false, PLAIN);
        assert!(app.selected.is_empty());
        // Vertices are handles.
        app.selected = vec![a.id];
        let id = a.id;
        drag(&mut app, pos2(350.0, 400.0), pos2(350.0, 450.0), PLAIN);
        assert_eq!(polys(&app)[1].0[2], pos2(350.0, 450.0));
        assert_eq!(app.find(id).unwrap().kind_name(), "Polygon");
    }

    #[test]
    fn fill_applies_to_selected_shapes_that_take_one() {
        let mut app = app();
        let r = rect(&mut app, 100.0, 100.0);
        let l = app.add(0, Kind::Line { a: pos2(0.0, 0.0), b: pos2(50.0, 50.0) });
        assert!(app.find(r).unwrap().style.fill.is_none(), "no fill until one is chosen");
        app.selected = vec![r, l];
        app.set_fill(Some(COLORS.len()));
        assert_eq!(app.find(r).unwrap().style.fill.map(|f| f.color), Some(WHITE));
        assert!(app.find(l).unwrap().style.fill.is_none(), "lines take no fill");
        app.set_fill_alpha(255);
        assert_eq!(app.find(r).unwrap().style.fill.map(|f| f.alpha), Some(255));
        app.undo();
        assert_eq!(app.find(r).unwrap().style.fill.map(|f| f.alpha), Some(128), "opacity undoes on its own");
        // A new colour keeps the markup's opacity.
        app.selected = vec![r];
        app.fill_alpha = 10;
        app.set_fill(Some(1));
        assert_eq!(app.find(r).unwrap().style.fill, Some(Fill { color: COLORS[1].0, alpha: 128 }));
        app.set_fill(None);
        assert!(app.find(r).unwrap().style.fill.is_none());
        assert!(app.find(r).unwrap().style.is_plain(), "plain style is left out of the markups file");
        // None clears a callout's white box too, and the toolbar says so.
        let c = callout(&mut app, pos2(300.0, 300.0), pos2(400.0, 400.0));
        app.selected = vec![c];
        assert_eq!(app.shown_fill(), Some(Fill { color: WHITE, alpha: 255 }));
        app.set_fill(None);
        assert_eq!(app.shown_fill(), None);
        let box_fill = app.find(c).unwrap().prims().into_iter().find_map(|p| match p {
            Prim::Fill { alpha, outline: Some(_), .. } => Some(alpha),
            _ => None,
        });
        assert_eq!(box_fill, Some(0), "the box keeps its outline but no fill");
        // Opacity never reaches zero, so a fill can always be turned back up.
        let r2 = rect(&mut app, 400.0, 100.0);
        app.selected = vec![r2];
        app.set_fill(Some(2));
        app.set_fill_alpha(0);
        assert!(app.shown_fill().is_some_and(|f| f.alpha == MIN_FILL_ALPHA));
        // With nothing selected, a callout's opacity sticks for the next ones.
        app.selected.clear();
        app.set_tool(Tool::Callout);
        app.set_fill_alpha(51);
        assert_eq!(app.shown_fill(), Some(Fill { color: WHITE, alpha: 51 }));
        let c2 = callout(&mut app, pos2(300.0, 500.0), pos2(400.0, 600.0));
        assert_eq!(app.find(c2).unwrap().effective_fill(), Some(Fill { color: WHITE, alpha: 51 }));
    }

    #[test]
    fn line_types_and_hatches_go_where_they_fit() {
        let mut app = app();
        let r = rect(&mut app, 100.0, 100.0);
        let t = app.add(0, Kind::Text { pos: pos2(300.0, 300.0), box_w: 100.0, text: "x".into(), size: 12.0 });
        let dashed = Arc::new(patterns::builtin_line_types()[0].clone());
        let hatch = Arc::new(patterns::builtin_hatches()[1].clone());
        app.selected = vec![r, t];
        app.set_line_type(Some(dashed.clone()));
        app.set_hatch(Some(HatchFill { pattern: hatch.clone(), scale: 1.0 }));
        let (ra, ta) = (app.find(r).unwrap().clone(), app.find(t).unwrap().clone());
        assert_eq!(ra.style.line.as_deref(), Some(&*dashed));
        assert!(ra.style.hatch.is_some());
        assert!(ta.style.line.is_none() && ta.style.hatch.is_none(), "text takes neither");
        assert!(ra.prims().iter().any(|p| matches!(p, Prim::Segments { .. })), "hatched");
        assert!(ra.prims().iter().filter(|p| matches!(p, Prim::Stroke { .. })).count() > 4, "dashed outline");
        // Hatched insides select on click, like filled ones.
        assert_eq!(ra.distance(pos2(150.0, 150.0)), 0.0);
        // The markups file carries the whole pattern, and reads back the same.
        let json = serde_json::to_string(&ra).unwrap();
        let back: Annot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.style, ra.style);
        // New markups with the tool get its line type.
        app.set_tool(Tool::Line);
        app.set_line_type(Some(dashed.clone()));
        let l = app.add(0, Kind::Line { a: pos2(0.0, 0.0), b: pos2(50.0, 0.0) });
        assert!(app.find(l).unwrap().style.line.is_some());
        app.set_tool(Tool::Arrow);
        let ar = app.add(0, Kind::Arrow { a: pos2(0.0, 0.0), b: pos2(50.0, 0.0) });
        assert!(app.find(ar).unwrap().style.line.is_none(), "per tool");
    }

    #[test]
    fn search_results_arrive_in_any_order_and_step_round() {
        let mut app = app();
        app.pages = vec![vec2(600.0, 800.0); 3];
        app.page_y = vec![0.0, 812.0, 1624.0];
        app.find.pending = 3;
        app.find_from = 1;
        let r = |y: f32| vec![Rect::from_min_size(pos2(100.0, y), vec2(40.0, 10.0))];
        // Searched from page 2 (index 1) on: its match is shown first.
        app.on_found(1, vec![r(100.0), r(300.0)]);
        assert_eq!(app.find.cur, Some(0));
        assert!(app.zoom >= 1.4 - 1e-3, "zoomed in to read 10 pt text");
        app.on_found(2, vec![]);
        app.on_found(0, vec![r(50.0)]);
        assert_eq!(app.find.hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![0, 1, 1]);
        assert_eq!(app.find.cur, Some(1), "still the same match after page 1's arrived");
        app.step_hit(1);
        app.step_hit(1);
        assert_eq!(app.find.cur, Some(0), "wraps round");
        app.step_hit(-1);
        assert_eq!(app.find.cur, Some(2));
        let y = app.offset.y + app.canvas.height() / (2.0 * app.zoom);
        assert!((y - (812.0 + 305.0)).abs() < 1.0, "centred on it: {y}");
    }

    #[test]
    fn esc_closes_the_find_bar_before_anything_else() {
        let mut app = app();
        let a = rect(&mut app, 100.0, 100.0);
        app.selected = vec![a];
        app.find.open = true;
        app.find.hits = vec![(0, vec![Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0))])];
        app.escape();
        assert!(!app.find.open && app.find.hits.is_empty());
        assert_eq!(app.selected, vec![a], "the selection stays for the next Esc");
        app.escape();
        assert!(app.selected.is_empty());
    }

    #[test]
    fn z_zooms_to_the_selection() {
        let mut app = app();
        let a = rect(&mut app, 100.0, 100.0);
        let b = rect(&mut app, 300.0, 100.0);
        app.selected = vec![a, b];
        app.zoom_to_selection();
        let r = app.annots.iter().fold(Rect::NOTHING, |r, x| r.union(x.bounds()));
        let (tl, br) = (app.page_to_screen(0, r.min), app.page_to_screen(0, r.max));
        assert!(app.canvas.contains(tl) && app.canvas.contains(br));
        assert!((br.x - tl.x) > app.canvas.width() - 80.0, "fills the width");
    }

    #[test]
    fn bold_italic_underline_apply_to_notes_and_new_ones() {
        let mut app = app();
        let c = callout(&mut app, pos2(100.0, 100.0), pos2(200.0, 200.0));
        let r = rect(&mut app, 300.0, 300.0);
        app.selected = vec![c, r];
        app.toggle_format(0);
        assert!(app.find(c).unwrap().style.bold);
        assert!(!app.find(r).unwrap().style.bold, "shapes have no text");
        assert_eq!(app.shown_format(), [true, false, false]);
        app.undo();
        assert!(!app.find(c).unwrap().style.bold);
        // With nothing selected it's for the next note.
        app.selected.clear();
        app.toggle_format(2);
        app.set_tool(Tool::Text);
        app.click(pos2(400.0, 100.0), false, PLAIN);
        let t = app.editing.unwrap();
        assert!(app.find(t).unwrap().style.underline);
        // While typing, the toggle is for that note, with the edit's own undo step,
        // and typing carries on.
        let steps = app.undo.len();
        app.edit_needs_focus = false;
        app.toggle_format(1);
        assert!(app.find(t).unwrap().style.italic);
        assert_eq!(app.undo.len(), steps);
        assert!(app.edit_needs_focus);
    }

    #[test]
    fn a_typed_notes_underline_is_the_editors_not_the_sheets() {
        let mut app = app();
        let c = callout(&mut app, pos2(100.0, 100.0), pos2(200.0, 200.0));
        let a = app.find(c).unwrap().clone();
        let mut under = a.clone();
        under.style.underline = true;
        assert!(under.prims().len() > a.prims().len(), "the sheet underlines it");
        // While it's typed, the sheet keeps the box and leader, and leaves the text and underline to the editor.
        let typed = typed_note_prims(&under);
        assert_eq!(typed.len(), typed_note_prims(&a).len());
        assert!(!typed.is_empty() && typed.iter().all(|p| !matches!(p, Prim::Text { .. })));
    }

    #[test]
    fn bold_on_and_off_leaves_a_fitted_note_as_it_was() {
        let mut app = app();
        let t = app.add(0, Kind::Text { pos: pos2(100.0, 100.0), box_w: 192.0, text: "Window head".into(), size: 12.0 });
        app.find_mut(t).unwrap().fit_box_w();
        let before = app.find(t).unwrap().text_box().unwrap();
        // A measurement last in the selection doesn't confuse the toggle.
        let m = app.add(0, Kind::Measure { pts: vec![pos2(0.0, 0.0), pos2(50.0, 0.0)], area: false, name: String::new(), value: String::new(), size: 10.0 });
        app.selected = vec![t, m];
        app.toggle_format(0);
        assert_eq!(app.shown_format()[0], true);
        let bold = app.find(t).unwrap().text_box().unwrap();
        assert_eq!(bold.height(), before.height(), "still one line");
        app.toggle_format(0);
        assert!(!app.find(t).unwrap().style.bold);
        assert_eq!(app.find(t).unwrap().text_box().unwrap(), before);
    }

    #[test]
    fn a_dropped_picture_fits_the_sheet_and_stays_on_it() {
        let mut app = app();
        // A tall image dropped near the bottom-right corner.
        app.picture_ready(Ok(("ab.png".into(), "Photo".into(), vec2(600.0, 2400.0))), Some((0, pos2(590.0, 790.0))), app.picture_epoch);
        let a = app.annots.last().unwrap();
        let Kind::Image { rect, name, .. } = &a.kind else { panic!() };
        assert_eq!(name, "Photo");
        assert!(rect.height() <= 800.0 * 0.6 + 0.01, "{rect:?}");
        assert!(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 800.0)).contains_rect(*rect), "{rect:?}");
        assert!((rect.width() / rect.height() - 0.25).abs() < 1e-3);
        // Two without a spot: placed one after the other.
        app.picture_ready(Ok(("e1.png".into(), "One".into(), vec2(100.0, 100.0))), None, app.picture_epoch);
        app.picture_ready(Ok(("e2.png".into(), "Two".into(), vec2(100.0, 100.0))), None, app.picture_epoch);
        assert!(matches!(&app.placing, Some(Place::Image { name, .. }) if name == "One"));
        app.click(pos2(200.0, 200.0), false, PLAIN);
        assert!(matches!(&app.placing, Some(Place::Image { name, .. }) if name == "Two"));
        app.click(pos2(300.0, 300.0), false, PLAIN);
        assert!(app.placing.is_none());
        // A very narrow, tall one still fits.
        app.picture_ready(Ok(("f.png".into(), "Tall".into(), vec2(60.0, 2400.0))), Some((0, pos2(300.0, 400.0))), app.picture_epoch);
        let Kind::Image { rect, .. } = &app.annots.last().unwrap().kind else { panic!() };
        assert!(rect.height() <= 480.01, "{rect:?}");
        // One still being stored when placing was cancelled is let go.
        let n = app.annots.len();
        let started = app.picture_epoch;
        app.set_tool(Tool::Rect);
        app.picture_ready(Ok(("cd.png".into(), "Old".into(), vec2(100.0, 100.0))), Some((0, pos2(100.0, 100.0))), started);
        assert_eq!(app.annots.len(), n);
        assert!(app.placing.is_none());
    }

    #[test]
    fn dragging_a_tab_reorders_and_keeps_the_shown_one_shown() {
        let mut app = app();
        // Four documents, "c" shown; the others parked.
        let mut parked = |name: &str| {
            let mut p = app.park();
            p.path = Some(PathBuf::from(format!("/{name}.pdf")));
            Tab { parked: Some(p) }
        };
        let (a, b, d) = (parked("a"), parked("b"), parked("d"));
        app.tabs = vec![a, b, Tab { parked: None }, d];
        app.cur_tab = 2;
        app.path = Some(PathBuf::from("/c.pdf"));
        let names = |app: &App| (0..app.tabs.len()).map(|i| app.tab_name(i)).collect::<Vec<_>>().join(" ").replace(".pdf", "");
        let shown = |app: &App| app.tab_name(app.cur_tab).replace(".pdf", "");
        assert_eq!(names(&app), "a b c d");
        // The shown tab itself.
        app.move_tab(2, 0);
        assert_eq!((names(&app).as_str(), shown(&app).as_str()), ("c a b d", "c"));
        // Others across it, both ways.
        app.move_tab(3, 0);
        assert_eq!((names(&app).as_str(), shown(&app).as_str()), ("d c a b", "c"));
        app.move_tab(0, 3);
        assert_eq!((names(&app).as_str(), shown(&app).as_str()), ("c a b d", "c"));
        // Others not crossing it.
        app.move_tab(1, 2);
        assert_eq!((names(&app).as_str(), shown(&app).as_str()), ("c b a d", "c"));
        // Out of range or in place: nothing.
        app.move_tab(1, 1);
        app.move_tab(9, 0);
        app.move_tab(0, 9);
        assert_eq!((names(&app).as_str(), shown(&app).as_str()), ("c b a d", "c"));
        // Switching still finds each document.
        app.switch_tab(3);
        assert_eq!((names(&app).as_str(), shown(&app).as_str()), ("c b a d", "d"));
        assert!(app.tabs[3].parked.is_none() && app.tabs[0].parked.is_some());
    }

    #[test]
    fn tabs_keep_each_documents_markups_view_and_undo() {
        let mut app = app();
        app.path = Some(PathBuf::from("/a.pdf"));
        let a = rect(&mut app, 100.0, 100.0);
        app.snapshot();
        app.zoom = 2.0;
        app.selected = vec![a];
        // A second document opens in a tab of its own.
        let parked = app.park();
        assert!(app.annots.is_empty() && app.path.is_none() && app.selected.is_empty());
        app.tabs[0].parked = Some(parked);
        app.tabs.push(Tab { parked: None });
        app.cur_tab = 1;
        app.path = Some(PathBuf::from("/b.pdf"));
        app.pages = vec![vec2(600.0, 800.0)];
        app.page_y = vec![0.0];
        app.zoom = 0.5;
        rect(&mut app, 300.0, 300.0);
        rect(&mut app, 400.0, 300.0);
        assert_eq!(app.tab_name(0), "a.pdf");
        assert_eq!(app.tab_name(1), "b.pdf");
        // Back to the first: its markups, zoom, selection and undo come back.
        app.switch_tab(0);
        assert_eq!(app.annots.len(), 1);
        assert_eq!((app.zoom, app.selected.clone()), (2.0, vec![a]));
        assert_eq!(app.undo.len(), 1);
        app.switch_tab(1);
        assert_eq!(app.annots.len(), 2);
        assert_eq!(app.zoom, 0.5);
        // Closing the shown tab shows its neighbour.
        app.close_tab(1);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.path, Some(PathBuf::from("/a.pdf")));
        assert_eq!(app.annots.len(), 1);
        // And closing the last leaves an empty window.
        app.close_tab(0);
        assert!(app.path.is_none() && app.annots.is_empty() && app.tabs.len() == 1);
    }

    #[test]
    fn a_move_under_way_is_finished_before_its_tab_is_parked() {
        let mut app = app();
        app.path = Some(PathBuf::from("/a.pdf"));
        let a = rect(&mut app, 100.0, 100.0);
        app.dirty = false;
        app.undo.clear();
        // Read-only here so the test keeps the unsaved flag to look at.
        app.read_only = true;
        app.begin_drag(pos2(100.0, 150.0), false, PLAIN);
        app.update_drag(pos2(140.0, 150.0), vec2(40.0, 0.0));
        let p = app.park();
        assert!(p.dirty, "the move is kept to be saved");
        assert_eq!(p.undo.len(), 1, "and can be undone");
        assert_eq!(p.annots[0].bounds().min.x, 138.0);
        let _ = a;
    }

    #[test]
    fn a_document_loaded_in_the_background_is_ready_when_shown() {
        let mut app = app();
        app.path = Some(PathBuf::from("/a.pdf"));
        app.generation = 5;
        let mut p = app.park();
        p.pages.clear();
        app.tabs[0].parked = Some(p);
        app.tabs.push(Tab { parked: None });
        app.cur_tab = 1;
        app.generation = 6;
        // What the worker sends for the first tab's document arrives while the second is shown.
        if let Some(p) = app.tabs[0].parked.as_mut() {
            (p.page_y, p.doc_w, p.doc_h) = layout(&[vec2(612.0, 792.0), vec2(1224.0, 792.0)]);
            p.pages = vec![vec2(612.0, 792.0), vec2(1224.0, 792.0)];
            p.fit_pending = true;
        }
        app.switch_tab(0);
        assert_eq!(app.pages.len(), 2);
        assert_eq!(app.doc_w, 1224.0);
        assert_eq!(app.page_y, vec![GAP, GAP + 792.0 + GAP]);
        assert!(app.fit_pending);
    }

    #[test]
    fn placed_dates_take_no_tool_fill() {
        let mut app = app();
        app.set_tool(Tool::Text);
        app.set_fill(Some(COLORS.len()));
        app.date_text = "2026-11-19".into();
        app.start_placing(Place::Date);
        app.click(pos2(100.0, 100.0), false, PLAIN);
        let a = app.annots.last().unwrap();
        assert!(matches!(a.kind, Kind::Text { .. }));
        assert!(a.style.fill.is_none());
    }

    #[test]
    fn a_vertex_drag_cant_make_a_polygon_cross_itself() {
        let mut app = app();
        let id = app.add(0, Kind::Poly { pts: annot::rect_pts(Rect::from_min_max(pos2(100.0, 100.0), pos2(200.0, 200.0))), closed: true });
        app.selected = vec![id];
        drag(&mut app, pos2(200.0, 100.0), pos2(50.0, 150.0), PLAIN);
        assert_eq!(polys(&app)[0].0[1], pos2(200.0, 100.0), "put back");
        assert!(app.undo.is_empty(), "and nothing to undo");
        drag(&mut app, pos2(200.0, 100.0), pos2(250.0, 80.0), PLAIN);
        assert_eq!(polys(&app)[0].0[1], pos2(250.0, 80.0));
    }

    #[test]
    fn filled_clouds_triangulate() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 250.0));
        let pts = annot::cloud_pts(rect);
        let tris = cached_triangles(&pts).expect("a cloud outline is simple");
        let area: f32 = tris.iter().map(|[a, b, c]| cross(pts[*a], pts[*b], pts[*c]).abs() / 2.0).sum();
        assert!((area as f64 - measure::area_pt2(&pts)).abs() < 1.0, "{area}");
        assert!(std::rc::Rc::ptr_eq(&tris, &cached_triangles(&pts).unwrap()), "second call is cached");
    }

    #[test]
    fn drags_make_a_segment_or_a_rectangle() {
        let mut app = app();
        app.set_tool(Tool::Length);
        drag(&mut app, pos2(100.0, 100.0), pos2(300.0, 100.0), PLAIN);
        app.set_tool(Tool::Area);
        drag(&mut app, pos2(100.0, 200.0), pos2(200.0, 300.0), PLAIN);
        let m = measures(&app);
        assert_eq!(m[0].0, vec![pos2(100.0, 100.0), pos2(300.0, 100.0)]);
        assert_eq!(m[1].0, annot::rect_pts(Rect::from_min_max(pos2(100.0, 200.0), pos2(200.0, 300.0))));
        assert!(m.iter().all(|m| m.2 == "no scale"));
    }

    #[test]
    fn calibration_sets_the_scale_and_relabels() {
        let mut app = app();
        app.set_tool(Tool::Length);
        drag(&mut app, pos2(100.0, 100.0), pos2(244.0, 100.0), PLAIN);
        app.set_tool(Tool::Calibrate);
        drag(&mut app, pos2(100.0, 400.0), pos2(172.0, 400.0), PLAIN);
        assert_eq!(app.tool, Tool::Select);
        let (page, a, b) = app.scale_win.calib.unwrap();
        // 72 pt (1 paper inch) is 4 ft: 1/4" = 1'-0".
        let real = measure::parse_length("4'", Units::Imperial).unwrap();
        app.apply_scale(page, real / (b - a).length() as f64, "Calibrated: 4'-0\"".into());
        measure::relabel(&mut app.annots, &app.scales, app.prefs.units);
        assert_eq!(measures(&app)[0].2, "8'-0\"");
        assert!(app.dirty, "scales are saved");
        // A region for a detail at its own scale only covers what's inside it.
        app.scale_win.target = Some((0, Rect::from_min_max(pos2(50.0, 50.0), pos2(400.0, 150.0))));
        app.apply_scale(0, 8.0 / 72.0, "1 1/2\" = 1'-0\"".into());
        measure::relabel(&mut app.annots, &app.scales, app.prefs.units);
        assert_eq!(measures(&app)[0].2, "1'-4\"");
        assert_eq!(app.scales.len(), 2);
        app.undo();
        assert_eq!(app.scales.len(), 1, "a scale change is one undo step");
        assert_eq!(measures(&app).len(), 1, "and leaves the measurements alone");
        // A region on one page can't be calibrated from another.
        app.pages.push(vec2(600.0, 800.0));
        app.scale_win.target = Some((1, Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0))));
        app.set_calibration(0, pos2(0.0, 500.0), pos2(200.0, 500.0));
        assert!(app.scale_win.calib.is_none());
        // Nor does a calibration left pending on page 1 take over a region drawn on page 2.
        app.scale_win.target = None;
        app.set_calibration(0, pos2(0.0, 500.0), pos2(200.0, 500.0));
        app.set_tool(Tool::ScaleRegion);
        app.create_measure(1, pos2(10.0, 10.0), pos2(200.0, 200.0));
        assert!(app.scale_win.calib.is_none());
        assert_eq!(app.scale_page(), 1);
    }

    #[test]
    fn text_size_applies_to_measurement_labels() {
        let mut app = app();
        app.set_tool(Tool::Length);
        drag(&mut app, pos2(100.0, 100.0), pos2(300.0, 100.0), PLAIN);
        let id = app.annots[0].id;
        app.set_tool(Tool::Select);
        app.selected = vec![id];
        app.set_text_size(18.0);
        assert!(matches!(app.annots[0].kind, Kind::Measure { size, .. } if size == 18.0));
    }

    #[test]
    fn esc_drops_a_measurement_in_progress() {
        let mut app = app();
        app.set_tool(Tool::Length);
        app.click(pos2(100.0, 100.0), false, PLAIN);
        assert!(app.building.is_some());
        app.building = None; // what Esc does
        app.set_tool(Tool::Select);
        assert!(measures(&app).is_empty());
        // Switching tools keeps one that's long enough.
        app.set_tool(Tool::Length);
        app.click(pos2(100.0, 100.0), false, PLAIN);
        app.click(pos2(200.0, 100.0), false, PLAIN);
        app.set_tool(Tool::Rect);
        assert_eq!(measures(&app).len(), 1);
    }

    #[test]
    fn concave_outlines_triangulate() {
        let l = [pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(100.0, 40.0), pos2(40.0, 40.0), pos2(40.0, 100.0), pos2(0.0, 100.0)];
        assert!(!convex(&l));
        let tris = triangulate(&l).unwrap();
        let area: f32 = tris.iter().map(|[a, b, c]| ((l[*b] - l[*a]).x * (l[*c] - l[*a]).y - (l[*b] - l[*a]).y * (l[*c] - l[*a]).x).abs() / 2.0).sum();
        assert!((area - 6400.0).abs() < 0.1, "{area}");
        assert!(convex(&annot::rect_pts(Rect::from_min_max(pos2(0.0, 0.0), pos2(5.0, 5.0)))));
    }


    /// An app on a 600x800 page whose proposals file is in a temp folder, with
    /// two batches: one markup and a scale, then a grouped cloud and callout.
    fn reviewing(name: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("greenline-review-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doc.proposals.json");
        let mut app = app();
        app.path = Some(dir.join("doc.pdf"));
        app.props_path = Some(file.clone());
        app.markups_file = Some(dir.join("doc.json"));
        let ops = serde_json::json!([
            {"op": "scale", "page": 1, "ratio": 48},
            {"op": "area", "page": 1, "rect": [100, 100, 154, 190], "name": "W1"}
        ]);
        let b1 = crate::ops::parse(&ops, &app.pages, 1, 1).unwrap();
        let ops = serde_json::json!([
            {"op": "cloud", "page": 1, "rect": [300, 300, 400, 400], "group": "g"},
            {"op": "callout", "page": 1, "target": [400, 350], "at": [420, 300], "text": "Gap in air barrier", "group": "g"}
        ]);
        let b2 = crate::ops::parse(&ops, &app.pages, 3, 2).unwrap();
        let mk = |id: &str, b: crate::ops::Batch| proposals::Proposal { id: id.into(), agent: "test".into(), summary: String::new(), created: "2026-11-13T12:00:00-05:00".into(), annots: b.annots, scales: b.scales, accepted_groups: Default::default() };
        let p = Proposals { pdf: dir.join("doc.pdf"), batches: vec![mk("1", b1), mk("2", b2)], seq: 2 };
        std::fs::write(&file, serde_json::to_vec(&p).unwrap()).unwrap();
        (app, dir)
    }

    #[test]
    fn proposals_arrive_and_are_accepted_or_rejected() {
        let (mut app, dir) = reviewing("accept");
        rect(&mut app, 10.0, 10.0);
        app.poll_proposals(10.0);
        assert!(app.show_review, "new proposals open the review panel");
        assert_eq!(app.props.count(), 4);
        assert_eq!(app.props.batches[0].annots[0].label(), "W1: 15.00 sf", "measured with the proposed scale");

        // One markup from batch 2, then the rest of it rejected.
        let callout = app.props.batches[1].annots[1].id;
        app.settle("2", Some(callout), None, true);
        assert_eq!(app.annots.len(), 2);
        assert_eq!(app.props.batches[1].annots.len(), 1, "the file keeps the rest");
        app.settle("2", None, None, false);
        assert_eq!(app.annots.len(), 2, "rejecting adds nothing");
        assert_eq!(app.props.batches.len(), 1);

        // Batch 1 whole: its markup and its scale.
        app.settle("1", None, None, true);
        assert!(app.props.batches.is_empty());
        assert_eq!(app.annots.len(), 3);
        let saved = sidecar::load_file(&dir.join("doc.json")).unwrap();
        assert_eq!((saved.annots.len(), saved.scales.len()), (3, 1), "accepted work is saved before it leaves the proposals");
        assert!(!app.dirty);
        // Ids aren't reused once their batch is gone.
        assert_eq!(proposals::load_file(&dir.join("doc.proposals.json")).unwrap().take_id(), "3");
        assert_eq!(app.scales.len(), 1);
        assert!(app.annots.iter().map(|a| a.id).collect::<HashSet<_>>().len() == 3, "fresh ids");
        measure::relabel(&mut app.annots, &app.scales, app.prefs.units);
        assert_eq!(app.annots[2].label(), "W1: 15.00 sf");

        // Accepting is one undo step, scale included.
        app.undo();
        assert_eq!((app.annots.len(), app.scales.len()), (2, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn accepted_groups_stay_together_and_read_only_cant_accept() {
        let (mut app, dir) = reviewing("groups");
        app.annots.push(Annot { id: 50, page: 0, color: [0; 3], width: 1.0, wrap_w: None, group: Some(2), style: Default::default(), kind: Kind::Rect { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)) } });
        app.next_id = 51;
        app.poll_proposals(10.0);
        app.read_only = true;
        app.settle("2", None, None, true);
        assert_eq!(app.props.count(), 4, "nothing taken while not autosaving");
        app.read_only = false;
        // One at a time: the callout joins the cloud's group.
        let (cloud, callout) = (app.props.batches[1].annots[0].id, app.props.batches[1].annots[1].id);
        app.settle("2", Some(cloud), None, true);
        app.settle("2", Some(callout), None, true);
        let groups: Vec<Option<u64>> = app.annots.iter().skip(1).map(|a| a.group).collect();
        assert_eq!(groups.len(), 2);
        assert!(groups[0].is_some());
        assert_eq!(groups[0], groups[1], "the cloud and its callout stay grouped");
        assert_ne!(groups[0], Some(2), "without joining an existing group");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_save_accepts_nothing() {
        let (mut app, dir) = reviewing("savefail");
        app.poll_proposals(10.0);
        // The markups file's folder is a file: saving fails.
        std::fs::write(dir.join("blocker"), b"").unwrap();
        app.markups_file = Some(dir.join("blocker").join("doc.json"));
        app.settle("1", None, None, true);
        assert!(app.annots.is_empty() && app.scales.is_empty());
        assert_eq!(app.props.count(), 4, "still proposed");
        assert!(app.undo.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_retried_accept_doesnt_add_twice_and_scales_match_by_value() {
        let (mut app, dir) = reviewing("retry");
        app.poll_proposals(10.0);
        // As if an earlier accept saved W1 but couldn't rewrite the proposals.
        let w1 = app.props.batches[0].annots[0].clone();
        app.annots.push(Annot { id: 77, ..w1.clone() });
        app.accepted = vec![format!("1/{}:77", w1.id)];
        app.settle("1", Some(w1.id), None, true);
        assert_eq!(app.annots.len(), 1, "not added again");
        assert!(app.props.batches[0].annots.is_empty(), "but taken out of the proposals");
        // A scale row that no longer matches the file does nothing.
        let mut stale = app.props.batches[0].scales[0].clone();
        stale.per_pt *= 2.0;
        app.settle("1", None, Some(stale), true);
        assert!(app.scales.is_empty());
        let s = app.props.batches[0].scales[0].clone();
        app.settle("1", None, Some(s), true);
        assert_eq!(app.scales.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_retried_accept_keeps_groups_together() {
        let (mut app, dir) = reviewing("retrygroup");
        app.poll_proposals(10.0);
        // The cloud was taken in (as markup 90, group 9) but the proposals write failed.
        let cloud = app.props.batches[1].annots[0].clone();
        app.annots.push(Annot { id: 90, group: Some(9), ..cloud.clone() });
        app.accepted = vec![format!("2/{}:90", cloud.id)];
        app.next_id = 91;
        app.settle("2", None, None, true);
        assert_eq!(app.annots.len(), 2, "the cloud isn't added again");
        assert_eq!(app.annots[1].group, Some(9), "the callout joins the cloud's group");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn accepting_one_row_recovers_its_groups_receipt_from_the_batch() {
        let (mut app, dir) = reviewing("retryrow");
        app.poll_proposals(10.0);
        let (cloud, callout) = (app.props.batches[1].annots[0].clone(), app.props.batches[1].annots[1].id);
        app.annots.push(Annot { id: 90, group: Some(9), ..cloud.clone() });
        app.accepted = vec![format!("2/{}:90", cloud.id)];
        app.next_id = 91;
        app.settle("2", Some(callout), None, true);
        assert_eq!(app.annots.last().unwrap().group, Some(9));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_proposals_write_still_shows_what_was_saved() {
        let (mut app, dir) = reviewing("propfail");
        app.poll_proposals(10.0);
        // The proposals temp file's name is taken by a folder: the rewrite fails after the markups save.
        std::fs::create_dir_all(dir.join(format!("doc.proposals.json.{}.tmp", std::process::id()))).unwrap();
        app.settle("1", None, None, true);
        assert_eq!(app.annots.len(), 1, "saved, so shown");
        assert_eq!(sidecar::load_file(&dir.join("doc.json")).unwrap().annots.len(), 1);
        assert_eq!(app.props.batches.len(), 2, "still proposed");
        // The reviewer changes the scale meanwhile.
        app.scales[0].per_pt *= 2.0;
        let mine = app.scales[0].clone();
        // Accepting again once the file can be written adds nothing twice, and keeps their scale.
        std::fs::remove_dir_all(dir.join(format!("doc.proposals.json.{}.tmp", std::process::id()))).unwrap();
        app.settle("1", None, None, true);
        assert_eq!(app.annots.len(), 1);
        assert_eq!(app.scales, vec![mine]);
        assert_eq!(app.props.batches.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn receipt_ids_arent_handed_out_again() {
        let a = Annot { id: 3, page: 0, color: [0; 3], width: 1.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Rect { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)) } };
        assert_eq!(first_free_id(&[a.clone()], &["1/5:12".into(), "1/scale:{\"page\":0}".into()]), 13);
        assert_eq!(first_free_id(&[a], &[]), 4);
        assert_eq!(first_free_id(&[], &[]), 1);
    }

    #[test]
    fn reserved_groups_arent_handed_out_again() {
        let (mut app, dir) = reviewing("reserved");
        app.poll_proposals(10.0);
        let cloud = app.props.batches[1].annots[0].id;
        app.settle("2", Some(cloud), None, true);
        let g = app.annots[0].group.unwrap();
        app.undo();
        assert!(app.annots.is_empty());
        assert!(app.next_group() > g, "the callout still waits to join group {g}");
        std::fs::remove_dir_all(&dir).ok();
    }

}
