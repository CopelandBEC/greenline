//! Copeland brand look: palette, light and dark visuals, Montserrat for UI text.

use egui::{Color32, FontFamily, Stroke, Theme, ThemePreference, Visuals};

/// Brand palette (CopelandBEC brand standards).
pub const GREEN: Color32 = Color32::from_rgb(0x44, 0x69, 0x04);
const GREEN_MID: Color32 = Color32::from_rgb(0x6B, 0x9A, 0x1E);
const GREEN_LIGHT: Color32 = Color32::from_rgb(0xE8, 0xF0, 0xDE);
/// Between light and medium green: pressed buttons and selected items in light mode.
const GREEN_PALE: Color32 = Color32::from_rgb(0xD2, 0xE3, 0xBA);
const GRAY: Color32 = Color32::from_rgb(0x44, 0x43, 0x43);

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
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "montserrat".into());
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
        v.panel_fill
    } else {
        Color32::from_rgb(0xE9, 0xEB, 0xE5)
    }
}

/// Behind the pages.
pub fn canvas(v: &Visuals) -> Color32 {
    if v.dark_mode {
        Color32::from_gray(0x30)
    } else {
        Color32::from_rgb(0xC6, 0xC8, 0xC3)
    }
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
    v
}

fn dark() -> Visuals {
    let mut v = Visuals::dark();
    let border = Color32::from_gray(0x44);
    let text = GREEN_LIGHT;
    v.panel_fill = Color32::from_gray(0x1F);
    v.window_fill = Color32::from_gray(0x2A);
    v.window_stroke = Stroke::new(1.0, border);
    v.faint_bg_color = Color32::from_gray(0x27);
    v.extreme_bg_color = Color32::from_gray(0x16);
    v.code_bg_color = Color32::from_gray(0x33);
    v.hyperlink_color = GREEN_MID;
    v.selection.bg_fill = GREEN;
    v.selection.stroke = Stroke::new(1.0, text);
    v.text_cursor.stroke.color = GREEN_MID;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::from_gray(0x1F);
    w.noninteractive.weak_bg_fill = Color32::from_gray(0x1F);
    w.noninteractive.bg_stroke.color = border;
    w.noninteractive.fg_stroke.color = text;
    w.inactive.weak_bg_fill = Color32::from_gray(0x33);
    w.inactive.bg_fill = Color32::from_gray(0x3A);
    w.inactive.fg_stroke.color = text;
    w.hovered.weak_bg_fill = Color32::from_rgb(0x2F, 0x3D, 0x1C);
    w.hovered.bg_fill = Color32::from_rgb(0x2F, 0x3D, 0x1C);
    w.hovered.bg_stroke = Stroke::new(1.0, GREEN_MID);
    w.hovered.fg_stroke.color = Color32::WHITE;
    w.active.weak_bg_fill = GREEN;
    w.active.bg_fill = GREEN;
    w.active.bg_stroke = Stroke::new(1.0, GREEN_MID);
    w.active.fg_stroke.color = Color32::WHITE;
    w.open.weak_bg_fill = Color32::from_gray(0x33);
    w.open.bg_fill = Color32::from_gray(0x33);
    w.open.bg_stroke = Stroke::new(1.0, border);
    w.open.fg_stroke.color = text;
    v
}
