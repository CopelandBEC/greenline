//! Copeland brand look: palette, light and dark visuals, Montserrat for UI text.

use egui::{Color32, FontFamily, Mesh, Painter, Pos2, Rect, Shadow, Stroke, Theme, ThemePreference, Visuals};

/// Brand palette (CopelandBEC brand standards).
pub const GREEN: Color32 = Color32::from_rgb(0x44, 0x69, 0x04);
const GREEN_MID: Color32 = Color32::from_rgb(0x6B, 0x9A, 0x1E);
const GREEN_LIGHT: Color32 = Color32::from_rgb(0xE8, 0xF0, 0xDE);
/// Between light and medium green: pressed buttons and selected items in light mode.
const GREEN_PALE: Color32 = Color32::from_rgb(0xD2, 0xE3, 0xBA);
const GRAY: Color32 = Color32::from_rgb(0x44, 0x43, 0x43);

/// Dark mode: soft, slightly cool grays (in the spirit of Things), not black.
/// Toolbar and status bar.
const D_PANEL: Color32 = Color32::from_rgb(0x2C, 0x2E, 0x33);
/// Side panes, a step darker than the bars.
const D_SHEETS: Color32 = Color32::from_rgb(0x26, 0x28, 0x2C);
/// Behind the pages.
const D_CANVAS: Color32 = Color32::from_rgb(0x3A, 0x3D, 0x42);
/// Popups and dialogs, a step lighter so they stand forward.
const D_WINDOW: Color32 = Color32::from_rgb(0x35, 0x37, 0x3C);
/// Text fields, sunk a step below the panel.
const D_FIELD: Color32 = Color32::from_rgb(0x21, 0x23, 0x27);
const D_BORDER: Color32 = Color32::from_rgb(0x41, 0x44, 0x4A);
/// Text: a soft off-white.
const D_TEXT: Color32 = Color32::from_rgb(0xE3, 0xE5, 0xE8);

pub fn install(ctx: &egui::Context, pref: ThemePreference) {
    // Light unless the system says otherwise (Linux often can't tell).
    ctx.options_mut(|o| o.fallback_theme = Theme::Light);
    ctx.set_visuals_of(Theme::Light, light());
    ctx.set_visuals_of(Theme::Dark, dark());
    ctx.set_theme(pref);
}

/// UI text in Montserrat. Markup text keeps its own Helvetica-metric family,
/// so this only changes the app's chrome.
pub fn add_font(fonts: &mut egui::FontDefinitions) {
    let data = egui::FontData::from_static(include_bytes!("../assets/fonts/Montserrat-Regular.ttf"));
    fonts.font_data.insert("montserrat".into(), std::sync::Arc::new(data));
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "montserrat".into());
    // Symbols none of those have (● on an unsaved tab, say): egui's monospace font has many.
    if !prop.iter().any(|f| f == "Hack") {
        prop.push("Hack".into());
    }
}

/// Brand green for marks on the UI (current page, markup counts), stepped
/// lighter on dark backgrounds.
pub fn accent(v: &Visuals) -> Color32 {
    if v.dark_mode {
        GREEN_MID
    } else {
        GREEN
    }
}

/// Behind page thumbnails (sidebar, overview): a shade off white in light
/// mode so the sheets' edges show.
pub fn sheets(v: &Visuals) -> Color32 {
    if v.dark_mode {
        D_SHEETS
    } else {
        Color32::from_rgb(0xE9, 0xEB, 0xE5)
    }
}

/// Behind the pages.
pub fn canvas(v: &Visuals) -> Color32 {
    if v.dark_mode {
        D_CANVAS
    } else {
        Color32::from_rgb(0xC6, 0xC8, 0xC3)
    }
}

/// A sheet's soft drop shadow on the canvas.
pub fn page_shadow(v: &Visuals) -> Shadow {
    Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if v.dark_mode { 120 } else { 70 }) }
}

/// A faint shade along the canvas's top and sides, as if the bars and panes
/// sat a little above it.
pub fn canvas_inset(p: &Painter, r: Rect, v: &Visuals) {
    let dark = Color32::from_black_alpha(if v.dark_mode { 70 } else { 34 });
    let depth = 8.0;
    let mut mesh = Mesh::default();
    let mut quad = |a: Pos2, b: Pos2, c: Pos2, d: Pos2| {
        // a–b: the edge (shaded); d–c: faded out.
        let i = mesh.vertices.len() as u32;
        for (pos, col) in [(a, dark), (b, dark), (c, Color32::TRANSPARENT), (d, Color32::TRANSPARENT)] {
            mesh.colored_vertex(pos, col);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    quad(r.left_top(), r.right_top(), r.right_top() + egui::vec2(0.0, depth), r.left_top() + egui::vec2(0.0, depth));
    let s = depth * 0.6;
    quad(r.left_top(), r.left_bottom(), r.left_bottom() + egui::vec2(s, 0.0), r.left_top() + egui::vec2(s, 0.0));
    quad(r.right_top(), r.right_bottom(), r.right_bottom() - egui::vec2(s, 0.0), r.right_top() - egui::vec2(s, 0.0));
    p.add(mesh);
}

/// Soft shadows under popups and dialogs, so they lift off what's below.
fn depth(v: &mut Visuals, alpha: u8) {
    v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(alpha) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(alpha) };
}

fn light() -> Visuals {
    let mut v = Visuals::light();
    let border = Color32::from_gray(0xCC);
    v.panel_fill = Color32::WHITE;
    v.window_fill = Color32::WHITE;
    v.window_stroke = Stroke::new(1.0, border);
    v.faint_bg_color = Color32::from_rgb(0xF4, 0xF7, 0xEF);
    v.extreme_bg_color = Color32::WHITE;
    v.code_bg_color = GREEN_LIGHT;
    v.hyperlink_color = GREEN;
    v.selection.bg_fill = GREEN_PALE;
    v.selection.stroke = Stroke::new(1.0, GREEN);
    v.text_cursor.stroke.color = GREEN;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::WHITE;
    w.noninteractive.weak_bg_fill = Color32::WHITE;
    w.noninteractive.bg_stroke.color = border;
    w.noninteractive.fg_stroke.color = GRAY;
    w.inactive.weak_bg_fill = Color32::from_gray(0xF0);
    w.inactive.bg_fill = Color32::from_gray(0xE2);
    w.inactive.fg_stroke.color = GRAY;
    // A hairline edge, so buttons read as raised off the bar.
    w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(0xD8));
    w.hovered.weak_bg_fill = GREEN_LIGHT;
    w.hovered.bg_fill = GREEN_LIGHT;
    w.hovered.bg_stroke = Stroke::new(1.0, GREEN_MID);
    w.hovered.fg_stroke.color = GREEN;
    w.active.weak_bg_fill = GREEN_PALE;
    w.active.bg_fill = GREEN_PALE;
    w.active.bg_stroke = Stroke::new(1.0, GREEN);
    w.active.fg_stroke.color = GREEN;
    w.open.weak_bg_fill = GREEN_LIGHT;
    w.open.bg_fill = GREEN_LIGHT;
    w.open.bg_stroke = Stroke::new(1.0, border);
    w.open.fg_stroke.color = GREEN;
    depth(&mut v, 38);
    v
}

fn dark() -> Visuals {
    let mut v = Visuals::dark();
    let border = D_BORDER;
    let text = D_TEXT;
    let hover = Color32::from_rgb(0x36, 0x42, 0x2B);
    v.panel_fill = D_PANEL;
    v.window_fill = D_WINDOW;
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(0x48, 0x4B, 0x51));
    v.faint_bg_color = Color32::from_rgb(0x30, 0x32, 0x37);
    v.extreme_bg_color = D_FIELD;
    v.code_bg_color = Color32::from_rgb(0x3A, 0x3C, 0x42);
    v.hyperlink_color = GREEN_MID;
    v.selection.bg_fill = GREEN;
    v.selection.stroke = Stroke::new(1.0, GREEN_LIGHT);
    v.text_cursor.stroke.color = GREEN_MID;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = D_PANEL;
    w.noninteractive.weak_bg_fill = D_PANEL;
    w.noninteractive.bg_stroke.color = border;
    w.noninteractive.fg_stroke.color = text;
    w.inactive.weak_bg_fill = Color32::from_rgb(0x3A, 0x3C, 0x42);
    w.inactive.bg_fill = Color32::from_rgb(0x44, 0x47, 0x4D);
    w.inactive.fg_stroke.color = text;
    // A hairline a shade lighter than the button: it reads as lit from above.
    w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4A, 0x4D, 0x53));
    w.hovered.weak_bg_fill = hover;
    w.hovered.bg_fill = hover;
    w.hovered.bg_stroke = Stroke::new(1.0, GREEN_MID);
    w.hovered.fg_stroke.color = Color32::WHITE;
    w.active.weak_bg_fill = GREEN;
    w.active.bg_fill = GREEN;
    w.active.bg_stroke = Stroke::new(1.0, GREEN_MID);
    w.active.fg_stroke.color = Color32::WHITE;
    w.open.weak_bg_fill = Color32::from_rgb(0x3A, 0x3C, 0x42);
    w.open.bg_fill = Color32::from_rgb(0x3A, 0x3C, 0x42);
    w.open.bg_stroke = Stroke::new(1.0, border);
    w.open.fg_stroke.color = text;
    depth(&mut v, 110);
    v
}
