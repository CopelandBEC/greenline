//! Markup model and geometry.
//!
//! Annotations live in page-local *view points*: the page as displayed (after
//! /Rotate and CropBox), origin top-left, y down, 1 unit = 1 PDF point.
//! Every annotation is reduced to a list of `Prim`s; the screen painter and
//! the PDF exporter both draw from those, so export matches the screen.

use crate::patterns::{HatchFill, LineType};
use egui::{pos2, vec2, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub type Rgb = [u8; 3];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annot {
    pub id: u64,
    pub page: usize,
    pub color: Rgb,
    pub width: f32,
    pub kind: Kind,
    /// Text-box width the user chose (drag or handle); the box widens back to
    /// this while editing and is shrunk to fit the text on commit.
    #[serde(default)]
    pub wrap_w: Option<f32>,
    /// Markups sharing a group id on one page select and move together.
    #[serde(default)]
    pub group: Option<u64>,
    /// Fill and other looks beyond colour and width; files from before 1.6 have none.
    #[serde(default, skip_serializing_if = "Style::is_plain")]
    pub style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    /// Colour inside a closed shape or text box. `None`: shapes stay open,
    /// callouts keep their white box, text has no box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    /// Dashes or symbols along the outline; solid when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<Arc<LineType>>,
    /// Hatch inside a closed shape, in the markup's colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hatch: Option<HatchFill>,
    /// Text in a callout or text box: bold, italic, underlined (the whole note).
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    /// Formatting word by word: runs of characters through the note, in
    /// order. Empty: the whole note as the flags above say. When there are
    /// runs, the flags say what the whole note has (all of it bold, say), which
    /// is also how a version without runs shows it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
    /// Where a note's lines sit across its box, and its text up and down it.
    #[serde(skip_serializing_if = "is_default")]
    pub align: HAlign,
    #[serde(skip_serializing_if = "is_default")]
    pub valign: VAlign,
    /// A note box's height as the user set it (the box is never shorter than
    /// its text); `None`: as tall as the text. A box with a height keeps its width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub box_h: Option<f32>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A note's lines: against the box's left, centred, or against its right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// A note's text in a box taller than it: at the top, in the middle, or at the bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// How a character of a note is set: bold, italic, underlined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Fmt {
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
}

impl Fmt {
    /// Bold (0), italic (1) or underline (2).
    pub fn get(&self, which: usize) -> bool {
        [self.bold, self.italic, self.underline][which]
    }

    pub fn set(&mut self, which: usize, on: bool) {
        *[&mut self.bold, &mut self.italic, &mut self.underline][which] = on;
    }
}

/// `len` characters of a note set alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Run {
    pub len: usize,
    #[serde(flatten)]
    pub fmt: Fmt,
}

impl Style {
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }

    /// The whole note's flags.
    pub fn whole(&self) -> Fmt {
        Fmt { bold: self.bold, italic: self.italic, underline: self.underline }
    }

    /// Each character's formatting, for a note of `n` characters (any beyond
    /// the runs take the whole note's flags).
    pub fn fmts(&self, n: usize) -> Vec<Fmt> {
        let mut v = Vec::with_capacity(n);
        for r in &self.runs {
            let k = r.len.min(n - v.len());
            v.extend(std::iter::repeat_n(r.fmt, k));
        }
        v.resize(n, self.whole());
        v
    }

    /// Sets each character's formatting: one throughout goes in the flags, and
    /// a mix in runs, with the flags then saying what all of it has.
    pub fn set_fmts(&mut self, f: &[Fmt]) {
        self.runs.clear();
        let Some(&first) = f.first() else { return };
        if f.iter().all(|x| *x == first) {
            (self.bold, self.italic, self.underline) = (first.bold, first.italic, first.underline);
            return;
        }
        for x in f {
            match self.runs.last_mut() {
                Some(r) if r.fmt == *x => r.len += 1,
                _ => self.runs.push(Run { len: 1, fmt: *x }),
            }
        }
        self.bold = f.iter().all(|x| x.bold);
        self.italic = f.iter().all(|x| x.italic);
        self.underline = f.iter().all(|x| x.underline);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub color: Rgb,
    /// 0..=255.
    pub alpha: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Kind {
    /// Text box with a leader arrow pointing at `anchor`; `leaders` are extra
    /// arrowheads whose tails meet the first leader's bend. `pos` is the box's
    /// top-left; height follows the wrapped text.
    Callout { anchor: Pos2, pos: Pos2, box_w: f32, text: String, size: f32, #[serde(default)] leaders: Vec<Pos2> },
    Text { pos: Pos2, box_w: f32, text: String, size: f32 },
    Cloud { rect: Rect },
    Rect { rect: Rect },
    Highlight { rect: Rect },
    Arrow { a: Pos2, b: Pos2 },
    Line { a: Pos2, b: Pos2 },
    Pen { pts: Vec<Pos2> },
    /// Straight segments through `pts`: a polyline, or a polygon when `closed`.
    Poly { pts: Vec<Pos2>, closed: bool },
    /// Stamp or signature from the image library (`stamps`), drawn into `rect`.
    /// `image` is the stored file's name; `name` is what the markup list shows.
    Image { rect: Rect, image: String, name: String },
    /// Measurement: a length along open `pts`, or the area inside them when
    /// `area`. `value` is the text drawn on the sheet (`name: 12'-6"`), kept up
    /// to date from the page's scale by `measure::relabel`.
    Measure { pts: Vec<Pos2>, area: bool, #[serde(default)] name: String, #[serde(default)] value: String, size: f32 },
}

#[derive(Clone, Debug)]
pub enum Prim {
    Stroke { pts: Vec<Pos2>, closed: bool, width: f32, color: Rgb },
    /// Closed polygon fill with optional outline. `alpha` 0..=255.
    Fill { pts: Vec<Pos2>, color: Rgb, alpha: u8, outline: Option<(Rgb, f32)> },
    /// One line of Helvetica text; `origin` is the left end of the baseline.
    Text { origin: Pos2, text: String, size: f32, color: Rgb, bold: bool, italic: bool },
    /// Library image stretched over `rect` (it keeps its aspect when resized).
    Image { rect: Rect, image: String },
    /// Many separate segments of one width and colour (a hatch), placed at
    /// `offset`; a zero-length one is a dot. `tile` is the pattern's repeat:
    /// when that's too small to make out on screen, the screen shows a tint
    /// of `tint` (the outline, with an opacity for the ink) instead.
    Segments { segs: Arc<Vec<[Pos2; 2]>>, offset: Vec2, width: f32, color: Rgb, tile: f32, tint: Option<(Arc<Vec<Pos2>>, u8)> },
}

/// A markup's prims and bounds, worked out once per look and position (`Annot::geom`).
pub struct Geom {
    pub prims: Vec<Prim>,
    pub bounds: Rect,
    /// The patterns it was made with, kept alive so their addresses (part of
    /// the cache key) can't be reused by different ones.
    _pins: (Option<Arc<LineType>>, Option<Arc<crate::patterns::Hatch>>),
}

/// Markups remembered per generation of the geometry cache.
const GEOM_GENERATION: usize = 4096;

pub const WHITE: Rgb = [255, 255, 255];

/// Opacity of an area measurement's tint when it has no fill of its own.
pub const AREA_TINT: u8 = 40;

/// Markup colours, by name. The first is the default for new markups: CopelandBEC medium green.
pub const COLORS: [(Rgb, &str); 6] = [
    ([0x6B, 0x9A, 0x1E], "Green"),
    ([228, 0, 0], "Red"),
    ([0, 88, 220], "Blue"),
    ([20, 20, 20], "Black"),
    ([240, 120, 0], "Orange"),
    ([255, 214, 0], "Yellow"),
];

// ---------------------------------------------------------------------------
// Text layout (Helvetica metrics; Liberation Sans on screen is metric-compatible)

/// Helvetica-Bold AFM advance widths for ASCII 32..=126, in 1/1000 em.
const HELV_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    333, 333, 584, 584, 584, 611, 975, // :;<=>?@
    722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, // A-M
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // N-Z
    333, 278, 333, 584, 556, 333, // [\]^_`
    556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, // a-m
    611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, // n-z
    389, 280, 389, 584, // {|}~
];

/// Helvetica AFM advance widths for ASCII 32..=126, in 1/1000 em.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // :;<=>?@
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, // A-M
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, // N-Z
    278, 278, 278, 469, 556, 333, // [\]^_`
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, // a-m
    556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, // n-z
    334, 260, 334, 584, // {|}~
];

pub fn char_width(c: char, size: f32) -> f32 {
    char_width_b(c, size, false)
}

/// Advance of `c` in Helvetica, or Helvetica-Bold when `bold` (italic doesn't change widths).
pub fn char_width_b(c: char, size: f32, bold: bool) -> f32 {
    let w = match c as u32 {
        32..=126 if bold => HELV_BOLD[(c as u32 - 32) as usize] as f32,
        32..=126 => HELV[(c as u32 - 32) as usize] as f32,
        _ if bold => crate::fonts::advance_bold(c).unwrap_or(611.0),
        _ => crate::fonts::advance(c).unwrap_or(556.0),
    };
    w * size / 1000.0
}

pub fn text_width(s: &str, size: f32) -> f32 {
    text_width_b(s, size, false)
}

pub fn text_width_b(s: &str, size: f32, bold: bool) -> f32 {
    s.chars().map(|c| char_width_b(c, size, bold)).sum()
}

/// Greedy word wrap. Explicit newlines are kept; over-long words are broken.
pub fn wrap(text: &str, size: f32, max_w: f32) -> Vec<String> {
    wrap_b(text, size, max_w, false)
}

pub fn wrap_b(text: &str, size: f32, max_w: f32, bold: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let w: Vec<f32> = chars.iter().map(|&c| char_width_b(c, size, bold)).collect();
    wrap_ranges(&chars, &w, max_w).into_iter().map(|r| chars[r].iter().collect()).collect()
}

/// Greedy word wrap into lines given as ranges of `chars`; `w` is each
/// character's width. Lines break at spaces (the space belongs to neither
/// line) and at newlines (kept as breaks); a word too wide alone is broken.
pub fn wrap_ranges(chars: &[char], w: &[f32], max_w: f32) -> Vec<std::ops::Range<usize>> {
    // Summed in order, character by character, as the lines' text would be.
    let width = |r: std::ops::Range<usize>| -> f32 { w[r].iter().sum() };
    let mut out = Vec::new();
    let mut start = 0;
    for para in chars.split(|&c| c == '\n') {
        let end = start + para.len();
        let mut line = start..start;
        let mut ws = start;
        for word in para.split(|&c| c == ' ') {
            let word_r = ws..ws + word.len();
            ws = word_r.end + 1;
            let candidate = if line.is_empty() { word_r.clone() } else { line.start..word_r.end };
            if width(candidate.clone()) <= max_w || line.is_empty() && width(word_r.clone()) <= max_w {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::replace(&mut line, word_r.start..word_r.start));
            }
            // Break a word that alone is too wide.
            for i in word_r {
                if !line.is_empty() && width(line.clone()) + w[i] > max_w {
                    out.push(std::mem::replace(&mut line, i..i));
                }
                line = if line.is_empty() { i..i + 1 } else { line.start..i + 1 };
            }
        }
        out.push(line);
        start = end + 1;
    }
    out
}

pub fn pad(size: f32) -> f32 {
    0.45 * size
}
pub fn line_h(size: f32) -> f32 {
    1.2 * size
}
/// Distance from a line's top to its baseline.
pub fn ascent(size: f32) -> f32 {
    0.88 * size
}

pub fn text_lines(text: &str, size: f32, box_w: f32) -> Vec<String> {
    text_lines_b(text, size, box_w, false)
}

pub fn text_lines_b(text: &str, size: f32, box_w: f32, bold: bool) -> Vec<String> {
    wrap_b(text, size, (box_w - 2.0 * pad(size)).max(size), bold)
}

pub fn box_rect(pos: Pos2, box_w: f32, text: &str, size: f32) -> Rect {
    box_rect_b(pos, box_w, text, size, false)
}

pub fn box_rect_b(pos: Pos2, box_w: f32, text: &str, size: f32, bold: bool) -> Rect {
    let n = text_lines_b(text, size, box_w, bold).len().max(1) as f32;
    box_of(pos, box_w, n, size)
}

fn box_of(pos: Pos2, box_w: f32, lines: f32, size: f32) -> Rect {
    Rect::from_min_size(pos, vec2(box_w, 2.0 * pad(size) + lines * line_h(size)))
}

/// A note's characters, their formatting and widths, and its lines in a box `box_w` wide.
pub struct NoteLayout {
    pub chars: Vec<char>,
    pub fmts: Vec<Fmt>,
    pub widths: Vec<f32>,
    pub lines: Vec<std::ops::Range<usize>>,
}

impl NoteLayout {
    pub fn new(text: &str, size: f32, box_w: f32, style: &Style) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let fmts = style.fmts(chars.len());
        let widths: Vec<f32> = chars.iter().zip(&fmts).map(|(&c, f)| char_width_b(c, size, f.bold)).collect();
        let lines = wrap_ranges(&chars, &widths, (box_w - 2.0 * pad(size)).max(size));
        Self { chars, fmts, widths, lines }
    }

    /// Width of line `i` without the spaces at its end.
    pub fn line_width(&self, i: usize) -> f32 {
        let r = self.lines[i].clone();
        let end = r.start + self.chars[r.clone()].iter().rposition(|c| !c.is_whitespace()).map_or(0, |k| k + 1);
        self.widths[r.start..end].iter().sum()
    }
}

/// The text sizes a note or label may have, in points.
pub const TEXT_SIZE_MIN: f32 = 4.0;
pub const TEXT_SIZE_MAX: f32 = 72.0;

pub fn default_box_w(size: f32) -> f32 {
    16.0 * size
}

// ---------------------------------------------------------------------------

impl Annot {
    /// Rect of the text box for Callout/Text kinds.
    pub fn text_box(&self) -> Option<Rect> {
        match &self.kind {
            Kind::Callout { pos, box_w, text, size, .. } | Kind::Text { pos, box_w, text, size } => {
                let r = box_of(*pos, *box_w, NoteLayout::new(text, *size, *box_w, &self.style).lines.len().max(1) as f32, *size);
                Some(match self.style.box_h {
                    Some(h) if h > r.height() => Rect::from_min_size(r.min, vec2(r.width(), h)),
                    _ => r,
                })
            }
            _ => None,
        }
    }

    /// Change a text box's width, keeping the edge nearest a callout's target fixed.
    pub fn set_box_w(&mut self, new_w: f32) {
        match &mut self.kind {
            Kind::Callout { anchor, pos, box_w, .. } => {
                if anchor.x > pos.x + *box_w {
                    pos.x += *box_w - new_w;
                }
                *box_w = new_w;
            }
            Kind::Text { box_w, .. } => *box_w = new_w,
            _ => {}
        }
    }

    /// Shrink a text box to its widest wrapped line.
    pub fn fit_box_w(&mut self) {
        // A box sized by hand keeps its size.
        if self.style.box_h.is_some() {
            return;
        }
        let (Some(r), Some(size)) = (self.text_box(), self.text_size()) else { return };
        let text = match &self.kind {
            Kind::Callout { text, .. } | Kind::Text { text, .. } => text.clone(),
            _ => return,
        };
        let layout = NoteLayout::new(&text, size, r.width(), &self.style);
        let widest = (0..layout.lines.len()).map(|i| layout.line_width(i)).fold(0.0, f32::max);
        // A little slack so float rounding never re-wraps the last word.
        let fit = (widest + 2.0 * pad(size) + 0.5).max(4.0 * size);
        if fit < r.width() {
            self.set_box_w(fit);
        }
    }

    /// Sizes a note's text to the biggest that fits its box. A box with a
    /// height set: the text wraps in its width and fits its height. Otherwise
    /// it fills the width it was given (else the box as it is), each line as
    /// typed on one line. False if it isn't a note with text.
    pub fn fit_text_to_box(&mut self) -> bool {
        let (Some(r), Some(size)) = (self.text_box(), self.text_size()) else { return false };
        let text = match &self.kind {
            Kind::Callout { text, .. } | Kind::Text { text, .. } if !text.trim().is_empty() => text.clone(),
            _ => return false,
        };
        let fit = if let Some(h) = self.style.box_h {
            let w = r.width();
            // Fits: as many lines as wrap at size s, none wider than the box (a long word can't wrap).
            let fits = |s: f32| {
                let l = NoteLayout::new(&text, s, w, &self.style);
                let widest = (0..l.lines.len()).map(|i| l.line_width(i)).fold(0.0, f32::max);
                2.0 * pad(s) + l.lines.len().max(1) as f32 * line_h(s) <= h + 0.01 && widest + 2.0 * pad(s) <= w + 0.01
            };
            // Half points, from the largest down: wrapping makes it no simple formula.
            let mut s = TEXT_SIZE_MAX;
            while s > TEXT_SIZE_MIN && !fits(s) {
                s -= 0.5;
            }
            s
        } else {
            let target = self.wrap_w.unwrap_or(r.width());
            // Widths go with the size: measure at this one, unwrapped, and scale.
            let layout = NoteLayout::new(&text, size, 1e7, &self.style);
            let widest = (0..layout.lines.len()).map(|i| layout.line_width(i)).fold(0.0, f32::max);
            if widest <= 0.0 {
                return false;
            }
            let per_pt = widest / size + 2.0 * pad(1.0);
            self.set_box_w(target);
            self.wrap_w = Some(target);
            ((((target - 0.5) / per_pt) * 2.0).floor() / 2.0).clamp(TEXT_SIZE_MIN, TEXT_SIZE_MAX)
        };
        if let Kind::Callout { size, .. } | Kind::Text { size, .. } = &mut self.kind {
            *size = fit;
        }
        self.fit_box_w();
        true
    }

    /// How far down its box a note's first line starts, past the padding (its
    /// text set top, middle or bottom in a box taller than the text).
    pub fn text_drop(&self) -> f32 {
        let (Some(r), Some(size)) = (self.text_box(), self.text_size()) else { return 0.0 };
        let (Kind::Callout { text, box_w, .. } | Kind::Text { text, box_w, .. }) = &self.kind else { return 0.0 };
        let lines = NoteLayout::new(text, size, *box_w, &self.style).lines.len().max(1) as f32;
        let room = (r.height() - 2.0 * pad(size) - lines * line_h(size)).max(0.0);
        match self.style.valign {
            VAlign::Top => 0.0,
            VAlign::Middle => room / 2.0,
            VAlign::Bottom => room,
        }
    }

    /// Give a box room to grow while typing (it is shrunk to fit again on commit).
    pub fn widen_for_edit(&mut self) {
        if let (Some(r), Some(size)) = (self.text_box(), self.text_size()) {
            let w = self.wrap_w.unwrap_or_else(|| default_box_w(size));
            if r.width() < w {
                self.set_box_w(w);
            }
        }
    }

    /// What lists and exported comments call it: the note, the image name, or the kind.
    pub fn label(&self) -> &str {
        match &self.kind {
            Kind::Image { name, .. } if !name.trim().is_empty() => name,
            Kind::Measure { value, .. } => value,
            _ if !self.text().trim().is_empty() => self.text(),
            _ => self.kind_name(),
        }
    }

    /// Note text ("" for shapes).
    pub fn text(&self) -> &str {
        match &self.kind {
            Kind::Callout { text, .. } | Kind::Text { text, .. } => text,
            _ => "",
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::Callout { .. } => "Callout",
            Kind::Text { .. } => "Text",
            Kind::Cloud { .. } => "Cloud",
            Kind::Rect { .. } => "Rectangle",
            Kind::Highlight { .. } => "Highlight",
            Kind::Arrow { .. } => "Arrow",
            Kind::Line { .. } => "Line",
            Kind::Pen { .. } => "Pen",
            Kind::Poly { closed: false, .. } => "Polyline",
            Kind::Poly { closed: true, .. } => "Polygon",
            Kind::Image { .. } => "Stamp",
            Kind::Measure { area: false, .. } => "Length",
            Kind::Measure { area: true, .. } => "Area",
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut String> {
        match &mut self.kind {
            Kind::Callout { text, .. } | Kind::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn text_size(&self) -> Option<f32> {
        match &self.kind {
            Kind::Callout { size, .. } | Kind::Text { size, .. } => Some(*size),
            _ => None,
        }
    }

    /// Everything the markup draws, with the box round it, remembered by content
    /// (see `Geom`): painting, hit-testing and lists ask every frame.
    pub fn geom(&self) -> Rc<Geom> {
        thread_local! {
            static CACHE: RefCell<[HashMap<u64, Rc<Geom>>; 2]> = RefCell::new([HashMap::new(), HashMap::new()]);
        }
        let key = self.fingerprint();
        if let Some(g) = CACHE.with(|c| {
            let mut c = c.borrow_mut();
            let hit = c[0].get(&key).cloned().or_else(|| c[1].remove(&key));
            if let Some(g) = &hit {
                c[0].entry(key).or_insert_with(|| g.clone());
            }
            hit
        }) {
            return g;
        }
        let g = Rc::new(Geom {
            prims: self.prims(),
            bounds: self.bounds_uncached(),
            _pins: (self.style.line.clone(), self.style.hatch.as_ref().map(|h| h.pattern.clone())),
        });
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            // Two generations: the one filling up, and the last, whose entries
            // move back over when used. Whatever went unused for a whole
            // generation is let go.
            if c[0].len() >= GEOM_GENERATION {
                c[1] = std::mem::take(&mut c[0]);
            }
            c[0].insert(key, g.clone());
        });
        g
    }

    /// A hash of everything `prims` and `bounds` depend on. Patterns count by
    /// identity: a `Geom` holds on to the ones it was made with, so a pattern
    /// it saw can't be freed and another put in its place.
    fn fingerprint(&self) -> u64 {
        use std::hash::{BuildHasher, Hash, Hasher};
        type H = foldhash::quality::FoldHasher<'static>;
        // A fast, well-mixed 64-bit hash, seeded the same every run (it's asked
        // for several times a frame per markup, so SipHash showed in profiles).
        static SEED: foldhash::quality::FixedState = foldhash::quality::FixedState::with_seed(0x9e37_79b9_7f4a_7c15);
        let mut h: H = SEED.build_hasher();
        let f = |h: &mut H, v: f32| h.write_u32(v.to_bits());
        let p = |h: &mut H, q: Pos2| {
            f(h, q.x);
            f(h, q.y);
        };
        let r = |h: &mut H, r: Rect| {
            p(h, r.min);
            p(h, r.max);
        };
        let pts = |h: &mut H, v: &[Pos2]| {
            h.write_usize(v.len());
            v.iter().for_each(|q| p(h, *q));
        };
        self.color.hash(&mut h);
        f(&mut h, self.width);
        std::mem::discriminant(&self.kind).hash(&mut h);
        match &self.kind {
            Kind::Callout { anchor, pos, box_w, text, size, leaders } => {
                p(&mut h, *anchor);
                p(&mut h, *pos);
                f(&mut h, *box_w);
                text.hash(&mut h);
                f(&mut h, *size);
                pts(&mut h, leaders);
            }
            Kind::Text { pos, box_w, text, size } => {
                p(&mut h, *pos);
                f(&mut h, *box_w);
                text.hash(&mut h);
                f(&mut h, *size);
            }
            Kind::Cloud { rect } | Kind::Rect { rect } | Kind::Highlight { rect } => r(&mut h, *rect),
            Kind::Arrow { a, b } | Kind::Line { a, b } => {
                p(&mut h, *a);
                p(&mut h, *b);
            }
            Kind::Pen { pts: v } => pts(&mut h, v),
            Kind::Poly { pts: v, closed } => {
                pts(&mut h, v);
                closed.hash(&mut h);
            }
            Kind::Image { rect, image, name } => {
                r(&mut h, *rect);
                image.hash(&mut h);
                name.hash(&mut h);
            }
            Kind::Measure { pts: v, area, name, value, size } => {
                pts(&mut h, v);
                area.hash(&mut h);
                name.hash(&mut h);
                value.hash(&mut h);
                f(&mut h, *size);
            }
        }
        let s = &self.style;
        s.fill.map(|x| (x.color, x.alpha)).hash(&mut h);
        s.line.as_ref().map(Arc::as_ptr).hash(&mut h);
        s.hatch.as_ref().map(|x| Arc::as_ptr(&x.pattern)).hash(&mut h);
        s.hatch.as_ref().map(|x| x.scale.to_bits()).hash(&mut h);
        (s.bold, s.italic, s.underline).hash(&mut h);
        s.runs.hash(&mut h);
        (s.align, s.valign).hash(&mut h);
        s.box_h.map(f32::to_bits).hash(&mut h);
        h.finish()
    }

    pub fn bounds(&self) -> Rect {
        self.geom().bounds
    }

    fn bounds_uncached(&self) -> Rect {
        let b = self.shape_bounds();
        match self.style.line.as_deref().filter(|_| self.takes_line_type()) {
            Some(lt) => b.expand(lt.reach(self.width)),
            None => b,
        }
    }

    fn shape_bounds(&self) -> Rect {
        let w = self.width.max(1.0);
        match &self.kind {
            // The leaders lie between box, anchor and extra tips; each arrowhead spreads up to arrow_len around its tip.
            Kind::Callout { anchor, leaders, .. } => {
                let mut b = self.text_box().unwrap().union(Rect::from_center_size(*anchor, Vec2::splat(2.0 * arrow_len(w))));
                for tip in leaders {
                    b = b.union(Rect::from_center_size(*tip, Vec2::splat(2.0 * arrow_len(w))));
                }
                b
            }
            Kind::Text { .. } => self.text_box().unwrap(),
            Kind::Cloud { rect } => rect.expand(cloud_radius(*rect) + w),
            Kind::Rect { rect } | Kind::Highlight { rect } => rect.expand(w),
            Kind::Image { rect, .. } => *rect,
            Kind::Arrow { a, b } | Kind::Line { a, b } => Rect::from_two_pos(*a, *b).expand(arrow_len(w) + w),
            Kind::Pen { pts } | Kind::Poly { pts, .. } => {
                let mut r = Rect::NOTHING;
                for p in pts {
                    r.extend_with(*p);
                }
                r.expand(w)
            }
            Kind::Measure { pts, .. } => {
                let r = pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p))).expand(tick_len(w) + w);
                self.value_box().map_or(r, |(b, _)| r.union(b))
            }
        }
    }

    /// A measurement's value label: its box and the text's baseline origin.
    pub fn value_box(&self) -> Option<(Rect, Pos2)> {
        let Kind::Measure { pts, area, value, size, .. } = &self.kind else { return None };
        if value.is_empty() || pts.len() < 2 {
            return None;
        }
        let at = if *area {
            centroid(pts)
        } else {
            // Middle of the longest segment.
            let w = pts.windows(2).max_by(|a, b| (a[1] - a[0]).length().total_cmp(&(b[1] - b[0]).length()))?;
            w[0] + (w[1] - w[0]) * 0.5
        };
        let p = 0.3 * size;
        let half = vec2(text_width(value, *size) / 2.0 + p, line_h(*size) / 2.0 + p);
        let r = Rect::from_center_size(at, half * 2.0);
        Some((r, pos2(r.min.x + p, r.min.y + p + ascent(*size))))
    }

    pub fn translate(&mut self, d: Vec2) {
        match &mut self.kind {
            Kind::Callout { pos, .. } | Kind::Text { pos, .. } => *pos += d,
            Kind::Cloud { rect } | Kind::Rect { rect } | Kind::Highlight { rect } | Kind::Image { rect, .. } => *rect = rect.translate(d),
            Kind::Arrow { a, b } | Kind::Line { a, b } => {
                *a += d;
                *b += d;
            }
            Kind::Pen { pts } | Kind::Poly { pts, .. } | Kind::Measure { pts, .. } => pts.iter_mut().for_each(|p| *p += d),
        }
    }

    /// Distance (view points) from `p` to the annotation; 0 when inside a filled area.
    pub fn distance(&self, p: Pos2) -> f32 {
        if let Some(r) = self.text_box() {
            if r.contains(p) {
                return 0.0;
            }
        }
        if let Kind::Highlight { rect } | Kind::Image { rect, .. } = &self.kind {
            if rect.contains(p) {
                return 0.0;
            }
        }
        if self.value_box().is_some_and(|(r, _)| r.contains(p)) {
            return 0.0;
        }
        if let Some(o) = self.filled_outline() {
            if inside(&o, p) {
                return 0.0;
            }
        }
        let mut best = f32::INFINITY;
        for prim in self.geom().prims.iter() {
            let (pts, closed) = match prim {
                Prim::Stroke { pts, closed, .. } => (pts, *closed),
                Prim::Fill { pts, .. } => (pts, true),
                Prim::Image { rect, .. } => {
                    best = best.min(polyline_dist(&rect_pts(*rect), true, p));
                    continue;
                }
                Prim::Text { .. } | Prim::Segments { .. } => continue,
            };
            best = best.min(polyline_dist(pts, closed, p));
        }
        (best - self.width / 2.0).max(0.0)
    }

    /// Whether a marquee `r` crosses the markup: its drawn lines, text box or
    /// highlight. Like clicking, the empty inside of a rectangle doesn't count.
    pub fn touches(&self, r: Rect) -> bool {
        if !self.bounds().intersects(r) {
            return false;
        }
        if self.text_box().is_some_and(|b| b.intersects(r)) {
            return true;
        }
        if let Kind::Highlight { rect } | Kind::Image { rect, .. } = &self.kind {
            if rect.intersects(r) {
                return true;
            }
        }
        if self.value_box().is_some_and(|(b, _)| b.intersects(r)) {
            return true;
        }
        if let Some(o) = self.filled_outline() {
            if inside(&o, r.center()) || o.iter().any(|p| r.contains(*p)) {
                return true;
            }
        }
        let r = r.expand(self.width / 2.0);
        self.geom().prims.iter().any(|prim| match prim {
            Prim::Stroke { pts, closed, .. } => polyline_hits(pts, *closed, r),
            Prim::Fill { pts, .. } => polyline_hits(pts, true, r),
            Prim::Text { .. } | Prim::Image { .. } | Prim::Segments { .. } => false,
        })
    }

    pub fn prims(&self) -> Vec<Prim> {
        let mut out = self.shape_prims();
        if let Some(lt) = self.style.line.as_deref().filter(|_| self.takes_line_type()) {
            out = out
                .into_iter()
                .flat_map(|p| match p {
                    Prim::Stroke { pts, closed, width, color } => {
                        let mut v = Vec::new();
                        crate::patterns::pattern_prims(&mut v, &pts, closed, lt, width, color);
                        v
                    }
                    // A callout's box: its fill, then the outline in the pattern.
                    Prim::Fill { pts, color, alpha, outline: Some((oc, ow)) } => {
                        let mut v = vec![Prim::Fill { pts: pts.clone(), color, alpha, outline: None }];
                        crate::patterns::pattern_prims(&mut v, &pts, true, lt, ow, oc);
                        v
                    }
                    p => vec![p],
                })
                .collect();
        }
        out
    }

    /// Whether the outline can be dashed or patterned.
    pub fn takes_line_type(&self) -> bool {
        matches!(self.kind, Kind::Callout { .. } | Kind::Cloud { .. } | Kind::Rect { .. } | Kind::Arrow { .. } | Kind::Line { .. } | Kind::Pen { .. } | Kind::Poly { .. })
    }

    /// Whether the markup can be hatched: closed shapes.
    pub fn hatchable(&self) -> bool {
        match &self.kind {
            Kind::Cloud { .. } | Kind::Rect { .. } => true,
            Kind::Poly { closed, .. } => *closed,
            Kind::Measure { area, .. } => *area,
            _ => false,
        }
    }

    /// The closed outline a fill or hatch goes inside.
    fn outline(&self) -> Option<Vec<Pos2>> {
        match &self.kind {
            Kind::Cloud { rect } => Some(cloud_pts(*rect)),
            Kind::Rect { rect } => Some(rect_pts(*rect)),
            Kind::Poly { pts, closed: true } | Kind::Measure { pts, area: true, .. } if pts.len() >= 3 => Some(pts.clone()),
            _ => None,
        }
    }

    fn shape_prims(&self) -> Vec<Prim> {
        let c = self.color;
        let w = self.width;
        let mut out = Vec::new();
        match &self.kind {
            Kind::Callout { anchor, text, size, leaders, .. } => {
                let r = self.text_box().unwrap();
                // The first leader leaves the box square to its side for a short
                // landing, then bends to the target; extra leaders run from that
                // bend to their own arrowheads.
                let (start, normal) = leader_start(r, *anchor);
                let room = (*anchor - start).dot(normal) - arrow_len(w) - 1.0;
                let knee = start + normal * landing(*size).min(room.max(0.0));
                if !r.contains(*anchor) {
                    arrow_path_prims(&mut out, &[start, knee], *anchor, w, c);
                }
                for tip in leaders {
                    arrow_path_prims(&mut out, &[knee], *tip, w, c);
                }
                let fill = self.style.fill.unwrap_or(Fill { color: WHITE, alpha: 255 });
                out.push(Prim::Fill { pts: rect_pts(r), color: fill.color, alpha: fill.alpha, outline: Some((c, w)) });
                text_prims(&mut out, r, self.text_drop(), text, *size, c, &self.style);
            }
            Kind::Text { text, size, .. } => {
                let r = self.text_box().unwrap();
                self.fill_prims(&mut out, rect_pts(r));
                text_prims(&mut out, r, self.text_drop(), text, *size, c, &self.style);
            }
            Kind::Cloud { rect } => {
                let pts = cloud_pts(*rect);
                self.fill_prims(&mut out, pts.clone());
                out.push(Prim::Stroke { pts, closed: true, width: w, color: c });
            }
            Kind::Rect { rect } => {
                self.fill_prims(&mut out, rect_pts(*rect));
                out.push(Prim::Stroke { pts: rect_pts(*rect), closed: true, width: w, color: c });
            }
            Kind::Poly { pts, closed } => {
                if *closed && pts.len() >= 3 {
                    self.fill_prims(&mut out, pts.clone());
                }
                out.push(Prim::Stroke { pts: pts.clone(), closed: *closed, width: w, color: c });
            }
            Kind::Highlight { rect } => out.push(Prim::Fill { pts: rect_pts(*rect), color: c, alpha: 90, outline: None }),
            Kind::Arrow { a, b } => arrow_prims(&mut out, *a, *b, w, c),
            Kind::Line { a, b } => out.push(Prim::Stroke { pts: vec![*a, *b], closed: false, width: w, color: c }),
            Kind::Pen { pts } => out.push(Prim::Stroke { pts: pts.clone(), closed: false, width: w, color: c }),
            Kind::Image { rect, image, .. } => out.push(Prim::Image { rect: *rect, image: image.clone() }),
            Kind::Measure { pts, area, value, size, .. } => {
                if *area && pts.len() >= 3 {
                    let fill = self.style.fill.unwrap_or(Fill { color: c, alpha: AREA_TINT });
                    out.push(Prim::Fill { pts: pts.clone(), color: fill.color, alpha: fill.alpha, outline: None });
                    self.hatch_prims(&mut out, pts);
                    out.push(Prim::Stroke { pts: pts.clone(), closed: true, width: w, color: c });
                } else if pts.len() >= 2 {
                    out.push(Prim::Stroke { pts: pts.clone(), closed: false, width: w, color: c });
                    // Ticks square to the line at both ends, like a dimension.
                    for (end, next) in [(pts[0], pts[1]), (pts[pts.len() - 1], pts[pts.len() - 2])] {
                        let d = next - end;
                        if d.length() > 0.01 {
                            let n = vec2(-d.y, d.x).normalized() * tick_len(w);
                            out.push(Prim::Stroke { pts: vec![end - n, end + n], closed: false, width: w, color: c });
                        }
                    }
                }
                if let Some((r, origin)) = self.value_box() {
                    out.push(Prim::Fill { pts: rect_pts(r), color: WHITE, alpha: 235, outline: None });
                    out.push(Prim::Text { origin, text: value.clone(), size: *size, color: c, bold: false, italic: false });
                }
            }
        }
        out
    }

    /// The fill a markup has when none is chosen: a callout's white box, an area's tint.
    pub fn default_fill(&self) -> Option<Fill> {
        match self.kind {
            Kind::Callout { .. } => Some(Fill { color: WHITE, alpha: 255 }),
            Kind::Measure { area: true, .. } => Some(Fill { color: self.color, alpha: AREA_TINT }),
            _ => None,
        }
    }

    /// The fill that's drawn, if any.
    pub fn effective_fill(&self) -> Option<Fill> {
        self.style.fill.or_else(|| self.default_fill()).filter(|f| f.alpha > 0)
    }

    /// Sets the fill; `None` clears even a default one (a transparent fill is stored).
    pub fn set_fill(&mut self, fill: Option<Fill>) {
        self.style.fill = match fill {
            None if self.default_fill().is_some() => Some(Fill { color: WHITE, alpha: 0 }),
            f => f,
        };
    }

    /// Whether the markup can take a fill: closed shapes and text boxes.
    pub fn fillable(&self) -> bool {
        match &self.kind {
            Kind::Callout { .. } | Kind::Text { .. } | Kind::Cloud { .. } | Kind::Rect { .. } => true,
            Kind::Poly { closed, .. } => *closed,
            Kind::Measure { area, .. } => *area,
            _ => false,
        }
    }

    /// The closed outline whose inside is filled, when the markup has a fill.
    /// (A callout's box always counts, through `text_box`.)
    fn filled_outline(&self) -> Option<Vec<Pos2>> {
        let hatched = self.style.hatch.is_some() && self.hatchable();
        if !self.style.fill.is_some_and(|f| f.alpha > 0) && !hatched {
            return None;
        }
        self.outline()
    }

    /// Fill, then hatch, inside `pts` (the outline is drawn after, on top).
    fn fill_prims(&self, out: &mut Vec<Prim>, pts: Vec<Pos2>) {
        if let Some(f) = self.style.fill.filter(|f| f.alpha > 0) {
            out.push(Prim::Fill { pts: pts.clone(), color: f.color, alpha: f.alpha, outline: None });
        }
        self.hatch_prims(out, &pts);
    }

    fn hatch_prims(&self, out: &mut Vec<Prim>, pts: &[Pos2]) {
        if let Some(h) = self.style.hatch.as_ref().filter(|_| self.hatchable()) {
            let (groups, offset) = crate::patterns::cached_hatch(pts, &h.pattern, h.scale);
            let tile = h.pattern.tile.min_elem() * h.scale;
            // Ink per tile, for the tint shown when zoomed out too far to see the lines.
            let t = h.pattern.tile * h.scale;
            let ink: f32 = h.pattern.lines.iter().map(|[a, b]| (vec2((b.x - a.x) * t.x, (b.y - a.y) * t.y)).length() * h.pattern.line_width).sum::<f32>()
                + h.pattern.dots.iter().map(|(_, d)| d * d).sum::<f32>();
            let alpha = (ink / (t.x * t.y).max(1.0) * 255.0).clamp(20.0, 160.0) as u8;
            let mut tint = Some((Arc::new(pts.to_vec()), alpha));
            for (width, segs) in groups.iter() {
                out.push(Prim::Segments { segs: segs.clone(), offset, width: *width, color: self.color, tile, tint: tint.take() });
            }
        }
    }
}

fn text_prims(out: &mut Vec<Prim>, r: Rect, drop: f32, text: &str, size: f32, color: Rgb, style: &Style) {
    let note = NoteLayout::new(text, size, r.width(), style);
    let inner = r.width() - 2.0 * pad(size);
    for (i, line) in note.lines.iter().enumerate() {
        let chars = &note.chars[line.clone()];
        if chars.iter().all(|c| c.is_whitespace()) {
            continue;
        }
        let x0 = r.min.x
            + pad(size)
            + match style.align {
                HAlign::Left => 0.0,
                HAlign::Center => ((inner - note.line_width(i)) / 2.0).max(0.0),
                HAlign::Right => (inner - note.line_width(i)).max(0.0),
            };
        let y = r.min.y + pad(size) + drop + i as f32 * line_h(size) + ascent(size);
        // Spaces at the end of a line aren't underlined.
        let ink_end = line.start + chars.iter().rposition(|c| !c.is_whitespace()).map_or(0, |k| k + 1);
        // One text prim per stretch set alike.
        let mut x = x0;
        let mut k = line.start;
        while k < line.end {
            let f = note.fmts[k];
            let e = (k..line.end).find(|&j| note.fmts[j] != f).unwrap_or(line.end);
            let w: f32 = note.widths[k..e].iter().sum();
            if f.underline && k < ink_end {
                // Helvetica's underline: 0.1 em below the baseline, 0.05 em thick.
                let uw: f32 = note.widths[k..e.min(ink_end)].iter().sum();
                let uy = y + 0.1 * size;
                out.push(Prim::Stroke { pts: vec![pos2(x, uy), pos2(x + uw, uy)], closed: false, width: 0.05 * size, color });
            }
            let run: String = note.chars[k..e].iter().collect();
            if !run.trim().is_empty() {
                out.push(Prim::Text { origin: pos2(x, y), text: run, size, color, bold: f.bold, italic: f.italic });
            }
            x += w;
            k = e;
        }
    }
}

/// Formatting carried through an edit of a note from `old` (formatted `fmts`)
/// to `new`: characters kept keep theirs, and typed ones take `typed`, or else
/// the formatting of the character before them (at the start, of the first).
/// `cursor` (a character index into `new`, where the cursor ended up) places
/// the edit: typing, deleting and pasting all leave the cursor where the
/// change ends, which tells repeated text apart ("go go" losing its first
/// "go " keeps the second's formatting). The edit is the smallest change
/// from `old` to `new`, keeping what's the same at both ends; the cursor only
/// picks between equally small ones, so a cursor elsewhere (put back by an
/// undo, say) can't make the change look bigger than it is.
pub fn carry_fmts(old: &str, fmts: &[Fmt], new: &str, typed: Option<Fmt>, whole: Fmt, cursor: Option<usize>) -> Vec<Fmt> {
    let (o, n): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let fmt = |i: usize| fmts.get(i).copied().unwrap_or(whole);
    let same = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let inferred = (same, o[same..].iter().rev().zip(n[same..].iter().rev()).take_while(|(a, b)| a == b).count());
    let at_cursor = cursor.filter(|&c| c <= n.len() && n.len() - c <= o.len() && o[o.len() - (n.len() - c)..] == n[c..]).map(|c| {
        let s = n.len() - c;
        (same.min(c).min(o.len() - s), s)
    });
    let (p, s) = at_cursor.filter(|&(p, s)| p + s == inferred.0 + inferred.1).unwrap_or(inferred);
    let at = typed.unwrap_or_else(|| if p > 0 { fmt(p - 1) } else { fmt(0) });
    let mut out: Vec<Fmt> = (0..p).map(fmt).collect();
    out.extend(std::iter::repeat_n(at, n.len() - p - s));
    out.extend((o.len() - s..o.len()).map(fmt));
    out
}

/// Rect with one corner at `fixed`, reaching towards `p`, with width/height
/// `aspect`: the larger of the two drags wins.
pub fn aspect_rect(fixed: Pos2, p: Pos2, aspect: f32) -> Rect {
    let d = p - fixed;
    let (mut w, mut h) = (d.x.abs(), d.y.abs());
    if w > h * aspect {
        h = w / aspect;
    } else {
        w = h * aspect;
    }
    let (w, h) = if w < 4.0 { (4.0, 4.0 / aspect) } else { (w, h) };
    Rect::from_two_pos(fixed, fixed + vec2(d.x.signum() * w, d.y.signum() * h))
}

/// Half-length of a length measurement's end ticks.
pub fn tick_len(width: f32) -> f32 {
    4.0 + 1.5 * width
}

/// Area centroid of a polygon, or the middle of its bounds when that falls
/// outside it (an L or U shape) or the polygon has no area.
pub fn centroid(pts: &[Pos2]) -> Pos2 {
    let bounds = pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)));
    let (mut a, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        let cross = p.x as f64 * q.y as f64 - q.x as f64 * p.y as f64;
        a += cross;
        cx += (p.x + q.x) as f64 * cross;
        cy += (p.y + q.y) as f64 * cross;
    }
    if a.abs() < 1e-6 {
        return bounds.center();
    }
    let c = pos2((cx / (3.0 * a)) as f32, (cy / (3.0 * a)) as f32);
    if inside(pts, c) { c } else { bounds.center() }
}

/// Even-odd point-in-polygon test.
pub fn inside(pts: &[Pos2], p: Pos2) -> bool {
    let mut odd = false;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + pts.len() - 1) % pts.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

pub fn rect_pts(r: Rect) -> Vec<Pos2> {
    vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
}

pub fn arrow_len(width: f32) -> f32 {
    6.0 + 3.5 * width
}

/// Line from `from` to `tip` with a filled head at `tip`.
fn arrow_prims(out: &mut Vec<Prim>, from: Pos2, tip: Pos2, w: f32, c: Rgb) {
    arrow_path_prims(out, &[from], tip, w, c);
}

/// Polyline through `path` ending in a filled arrowhead at `tip`.
fn arrow_path_prims(out: &mut Vec<Prim>, path: &[Pos2], tip: Pos2, w: f32, c: Rgb) {
    // Head direction from the last path point that isn't the tip itself.
    let Some(&from) = path.iter().rev().find(|p| (tip - **p).length() >= 0.01) else { return };
    let d = tip - from;
    let len = d.length();
    let dir = d / len;
    let n = vec2(-dir.y, dir.x);
    let hl = arrow_len(w).min(len);
    let base = tip - dir * hl;
    let mut pts = path.to_vec();
    pts.dedup_by(|a, b| (*a - *b).length() < 0.01);
    pts.push(base + dir * (w * 0.5));
    out.push(Prim::Stroke { pts, closed: false, width: w, color: c });
    let hw = hl * 0.38;
    out.push(Prim::Fill { pts: vec![tip, base + n * hw, base - n * hw], color: c, alpha: 255, outline: None });
}

/// Length of the straight landing segment on a callout leader.
pub fn landing(size: f32) -> f32 {
    2.0 * size
}

/// Middle of the box side facing the anchor, and that side's outward normal.
/// Left/right sides are used unless the anchor sits within the box's width.
fn leader_start(r: Rect, p: Pos2) -> (Pos2, Vec2) {
    let c = r.center();
    if p.x > r.max.x {
        (pos2(r.max.x, c.y), Vec2::X)
    } else if p.x < r.min.x {
        (pos2(r.min.x, c.y), -Vec2::X)
    } else if p.y > c.y {
        (pos2(c.x, r.max.y), Vec2::Y)
    } else {
        (pos2(c.x, r.min.y), -Vec2::Y)
    }
}

pub fn cloud_radius(rect: Rect) -> f32 {
    (rect.width().min(rect.height()) / 6.0).clamp(4.0, 12.0)
}

/// Scalloped outline: perimeter split into chords, each replaced by an outward arc.
pub fn cloud_pts(rect: Rect) -> Vec<Pos2> {
    let r = rect;
    let chord = cloud_radius(r) * 2.0;
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    let mut perim = Vec::new();
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let n = ((b - a).length() / chord).round().max(1.0) as usize;
        for k in 0..n {
            perim.push(a + (b - a) * (k as f32 / n as f32));
        }
    }
    let mut pts = Vec::new();
    let center = r.center();
    for i in 0..perim.len() {
        let (a, b) = (perim[i], perim[(i + 1) % perim.len()]);
        let mid = a + (b - a) * 0.5;
        let half = (b - a).length() / 2.0;
        if half < 0.01 {
            continue;
        }
        let mut n = vec2(-(b - a).y, (b - a).x).normalized();
        if n.dot(mid - center) < 0.0 {
            n = -n;
        }
        // Arc through a and b with sagitta s (bulging outward).
        let s = half * 0.8;
        let rad = (half * half + s * s) / (2.0 * s);
        let cc = mid + n * (s - rad);
        let a0 = (a - cc).angle();
        let mut a1 = (b - cc).angle();
        // Sweep the short way round through the bulge.
        let mut sweep = a1 - a0;
        while sweep > std::f32::consts::PI {
            sweep -= std::f32::consts::TAU;
        }
        while sweep < -std::f32::consts::PI {
            sweep += std::f32::consts::TAU;
        }
        let mid_ang = a0 + sweep / 2.0;
        let probe = cc + Vec2::angled(mid_ang) * rad;
        if (probe - mid).dot(n) < 0.0 {
            sweep = if sweep > 0.0 { sweep - std::f32::consts::TAU } else { sweep + std::f32::consts::TAU };
        }
        a1 = a0 + sweep;
        let steps = 10;
        for k in 0..steps {
            let t = a0 + (a1 - a0) * (k as f32 / steps as f32);
            pts.push(cc + Vec2::angled(t) * rad);
        }
    }
    pts
}

fn seg_dist(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    if l2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn polyline_dist(pts: &[Pos2], closed: bool, p: Pos2) -> f32 {
    let mut best = f32::INFINITY;
    if pts.len() == 1 {
        return (p - pts[0]).length();
    }
    for w in pts.windows(2) {
        best = best.min(seg_dist(w[0], w[1], p));
    }
    if closed && pts.len() > 2 {
        best = best.min(seg_dist(pts[pts.len() - 1], pts[0], p));
    }
    best
}

/// Liang–Barsky: does segment `a`–`b` pass through `r`?
fn seg_hits(a: Pos2, b: Pos2, r: Rect) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-d.x, a.x - r.min.x), (d.x, r.max.x - a.x), (-d.y, a.y - r.min.y), (d.y, r.max.y - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    t0 <= t1
}

fn polyline_hits(pts: &[Pos2], closed: bool, r: Rect) -> bool {
    if pts.len() == 1 {
        return r.contains(pts[0]);
    }
    pts.windows(2).any(|w| seg_hits(w[0], w[1], r)) || (closed && pts.len() > 2 && seg_hits(pts[pts.len() - 1], pts[0], r))
}

/// Ramer–Douglas–Peucker simplification for pen strokes.
pub fn simplify(pts: &[Pos2], eps: f32) -> Vec<Pos2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (mut idx, mut dmax) = (0, 0.0);
    for (i, p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d = seg_dist(a, b, *p);
        if d > dmax {
            idx = i;
            dmax = d;
        }
    }
    if dmax > eps {
        let mut left = simplify(&pts[..=idx], eps);
        let right = simplify(&pts[idx..], eps);
        left.pop();
        left.extend(right);
        left
    } else {
        vec![a, b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_width() {
        let lines = wrap("check this dimension against the structural set please", 12.0, 120.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(text_width(l, 12.0) <= 120.0, "{l}");
        }
        assert_eq!(wrap("a\n\nb", 12.0, 100.0), vec!["a", "", "b"]);
    }

    /// The wrap as it was before lines became character ranges: every note
    /// must still break where it did.
    fn old_wrap(text: &str, size: f32, max_w: f32, bold: bool) -> Vec<String> {
        let text_width = |s: &str, size: f32| text_width_b(s, size, bold);
        let char_width = |c: char, size: f32| char_width_b(c, size, bold);
        let mut out = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if text_width(&candidate, size) <= max_w || line.is_empty() && text_width(word, size) <= max_w {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                for c in word.chars() {
                    if !line.is_empty() && text_width(&line, size) + char_width(c, size) > max_w {
                        out.push(std::mem::take(&mut line));
                    }
                    line.push(c);
                }
            }
            out.push(line);
        }
        out
    }

    #[test]
    fn wrap_breaks_where_it_always_did() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let pieces = ["a", "flashing", "WWWWWWWWWWWWWWWWWWWWWW", " ", " ", "  ", "\n", "sill", "Ø25", "—", "jamb,", "i"];
        for _ in 0..3000 {
            let text: String = (0..rnd(14)).map(|_| pieces[rnd(pieces.len() as u64) as usize]).collect();
            let (size, max_w, bold) = ([8.0, 10.0, 12.0, 18.0][rnd(4) as usize], 5.0 + rnd(300) as f32, rnd(2) == 1);
            assert_eq!(wrap_b(&text, size, max_w, bold), old_wrap(&text, size, max_w, bold), "{text:?} {size} {max_w} {bold}");
        }
    }

    #[test]
    fn fit_box_shrinks_and_keeps_callout_edge() {
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Callout { anchor: pos2(500.0, 0.0), pos: pos2(100.0, 0.0), box_w: 300.0, text: "Short".into(), size: 12.0, leaders: vec![] } };
        let right = a.text_box().unwrap().max.x;
        a.fit_box_w();
        let r = a.text_box().unwrap();
        assert!(r.width() < 100.0);
        assert!((r.max.x - right).abs() < 0.01, "edge facing the anchor stays put");
        assert_eq!(text_lines("Short", 12.0, r.width()).len(), 1);
        a.widen_for_edit();
        assert!((a.text_box().unwrap().width() - default_box_w(12.0)).abs() < 0.01);
    }

    #[test]
    fn extra_leaders_meet_the_first_bend() {
        let a = Annot {
            id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, style: Default::default(),
            kind: Kind::Callout {
                anchor: pos2(500.0, 0.0), pos: pos2(100.0, 0.0), box_w: 300.0,
                text: "Note".into(), size: 12.0, leaders: vec![pos2(200.0, 400.0), pos2(600.0, 300.0)],
            },
        };
        let strokes: Vec<Vec<Pos2>> = a.prims().iter().filter_map(|p| match p {
            Prim::Stroke { pts, closed: false, .. } => Some(pts.clone()),
            _ => None,
        }).collect();
        assert_eq!(strokes.len(), 3, "the first leader plus two extra ones");
        let knee = strokes[0][1];
        for extra in &strokes[1..] {
            assert_eq!(extra[0], knee, "extra leaders start at the first leader's bend");
        }
        // The tips are where the user clicked: bounds, hit-testing and export see them.
        for tip in [pos2(200.0, 400.0), pos2(600.0, 300.0)] {
            assert!(a.bounds().contains(tip));
            assert_eq!(a.distance(tip), 0.0);
        }
        // Sidecar files written before leaders existed still load.
        let old: Kind = serde_json::from_str(r#"{"Callout":{"anchor":[500.0,0.0],"pos":[100.0,0.0],"box_w":300.0,"text":"Note","size":12.0}}"#).unwrap();
        assert!(matches!(old, Kind::Callout { ref leaders, .. } if leaders.is_empty()));
    }

    #[test]
    fn marquee_touches_lines_not_empty_space() {
        let ann = |kind| Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, style: Default::default(), kind };
        let box_at = |x: f32, y: f32| Rect::from_min_size(pos2(x, y), vec2(10.0, 10.0));
        let diag = ann(Kind::Line { a: pos2(0.0, 0.0), b: pos2(100.0, 100.0) });
        assert!(diag.touches(box_at(45.0, 45.0)));
        assert!(!diag.touches(box_at(80.0, 5.0)), "inside the bounding box but off the line");
        assert!(diag.touches(Rect::from_min_max(pos2(-10.0, -10.0), pos2(200.0, 200.0))), "fully enclosed");
        let rect = ann(Kind::Rect { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)) });
        assert!(!rect.touches(box_at(45.0, 45.0)), "empty interior");
        assert!(rect.touches(box_at(95.0, 45.0)), "crosses the edge");
        let hl = ann(Kind::Highlight { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0)) });
        assert!(hl.touches(box_at(45.0, 5.0)));
        let note = ann(Kind::Text { pos: pos2(200.0, 200.0), box_w: 100.0, text: "Check".into(), size: 12.0 });
        assert!(note.touches(box_at(220.0, 205.0)));
        assert!(!note.touches(box_at(0.0, 300.0)));
    }

    #[test]
    fn aspect_rect_keeps_shape_and_corner() {
        let r = aspect_rect(pos2(100.0, 100.0), pos2(40.0, 90.0), 2.0);
        assert_eq!(r.max, pos2(100.0, 100.0), "fixed corner stays");
        assert!((r.width() - 60.0).abs() < 1e-3 && (r.height() - 30.0).abs() < 1e-3, "{r:?}");
        let r = aspect_rect(pos2(0.0, 0.0), pos2(10.0, 50.0), 2.0);
        assert!((r.width() - 100.0).abs() < 1e-3 && (r.height() - 50.0).abs() < 1e-3, "{r:?}");
    }

    #[test]
    fn image_hits_inside_and_labels_by_name() {
        let a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Image { rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 50.0)), image: "x.png".into(), name: "NC seal".into() } };
        assert_eq!(a.distance(pos2(30.0, 30.0)), 0.0);
        assert!((a.distance(pos2(100.0, 30.0)) - 39.0).abs() < 1e-3, "outline distance less half the line width");
        assert!(a.touches(Rect::from_min_size(pos2(20.0, 20.0), vec2(5.0, 5.0))));
        assert_eq!(a.bounds(), Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 50.0)));
        assert_eq!(a.label(), "NC seal");
    }

    #[test]
    fn a_line_types_symbols_count_in_the_bounds() {
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 1.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Line { a: pos2(0.0, 100.0), b: pos2(200.0, 100.0) } };
        let plain = a.bounds();
        let symbol = crate::patterns::Comp::Symbol { left: 5.0, right: 5.0, stretch: 1.0, fill: false, paths: vec![vec![pos2(0.0, -20.0), pos2(0.0, 20.0)]] };
        a.style.line = Some(Arc::new(crate::patterns::LineType { name: "tall".into(), scale_with_width: false, lines: vec![crate::patterns::LinePattern { offset: 0.0, comps: vec![symbol] }] }));
        let b = a.bounds();
        assert!(b.min.y <= 80.0 && b.max.y >= 120.0 && plain.height() < b.height(), "{plain:?} {b:?}");
        for p in a.prims() {
            if let Prim::Stroke { pts, .. } = p {
                assert!(pts.iter().all(|q| b.contains(*q)));
            }
        }
    }

    #[test]
    fn bold_text_is_wider_and_underline_runs_under_each_line() {
        assert!(text_width_b("Window head", 12.0, true) > text_width("Window head", 12.0));
        assert_eq!(text_width_b("ll", 10.0, true), 2.0 * 2.78);
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 1.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Text { pos: pos2(0.0, 0.0), box_w: 120.0, text: "Seal the head flashing to the jamb".into(), size: 12.0 } };
        let plain = a.text_box().unwrap();
        a.style.bold = true;
        a.style.underline = true;
        assert!(a.text_box().unwrap().height() >= plain.height(), "bold wraps sooner");
        let prims = a.prims();
        let lines = prims.iter().filter(|p| matches!(p, Prim::Text { bold: true, .. })).count();
        let unders = prims.iter().filter(|p| matches!(p, Prim::Stroke { .. })).count();
        assert!(lines >= 2 && unders == lines, "{lines} lines, {unders} underlines");
        // Old files without the flags read as plain.
        let s: Style = serde_json::from_str("{}").unwrap();
        assert!(!s.bold && !s.italic && !s.underline && s.is_plain());
    }

    /// The cached geometry follows every change to a markup: text, position,
    /// style, and a pattern swapped for another of the same name.
    #[test]
    fn cached_geometry_follows_changes() {
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 1.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Text { pos: pos2(0.0, 0.0), box_w: 120.0, text: "Short".into(), size: 12.0 } };
        let texts = |a: &Annot| a.geom().prims.iter().filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.clone()) } else { None }).collect::<Vec<_>>();
        assert_eq!(texts(&a), ["Short"]);
        let h1 = a.bounds().height();
        *a.text_mut().unwrap() = "Seal the head flashing to the jamb at every opening".into();
        assert!(texts(&a).len() > 1 && a.bounds().height() > h1, "new text, taller box");
        a.translate(vec2(10.0, 0.0));
        assert_eq!(a.bounds().min.x, 10.0);
        a.style.bold = true;
        assert!(a.geom().prims.iter().any(|p| matches!(p, Prim::Text { bold: true, .. })));
        a.color = [255, 0, 0];
        assert!(a.geom().prims.iter().all(|p| !matches!(p, Prim::Text { color, .. } if *color != [255, 0, 0])));
        let mut l = Annot { kind: Kind::Line { a: pos2(0.0, 100.0), b: pos2(200.0, 100.0) }, ..a.clone() };
        let tall = |h: f32| Arc::new(crate::patterns::LineType { name: "same".into(), scale_with_width: false, lines: vec![crate::patterns::LinePattern { offset: 0.0, comps: vec![crate::patterns::Comp::Symbol { left: 5.0, right: 5.0, stretch: 1.0, fill: false, paths: vec![vec![pos2(0.0, -h), pos2(0.0, h)]] }] }] });
        l.style.line = Some(tall(10.0));
        let short = l.bounds();
        l.style.line = Some(tall(30.0));
        assert!(l.bounds().height() > short.height() + 30.0, "{short:?} {:?}", l.bounds());
    }

    /// Words formatted on their own: laid out run by run (a bold word is wider,
    /// the next run starts where it ends), underlined only where they are,
    /// carried through typing, and saved compactly.
    #[test]
    fn formatting_word_by_word() {
        let b = Fmt { bold: true, ..Default::default() };
        let u = Fmt { underline: true, ..Default::default() };
        let p = Fmt::default();
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 1.0, wrap_w: None, group: None, style: Default::default(), kind: Kind::Text { pos: pos2(0.0, 0.0), box_w: 400.0, text: "Seal the sill".into(), size: 10.0 } };
        let mut f = vec![p; 13];
        f[5..8].iter_mut().for_each(|x| *x = b);
        f[9..13].iter_mut().for_each(|x| *x = u);
        a.style.set_fmts(&f);
        assert_eq!(a.style.runs.len(), 4);
        assert!(!a.style.bold && !a.style.underline, "the flags say what all of it has");
        assert_eq!(a.style.fmts(13), f);
        let texts: Vec<(String, f32, bool)> = a.geom().prims.iter().filter_map(|p| if let Prim::Text { text, origin, bold, .. } = p { Some((text.clone(), origin.x, *bold)) } else { None }).collect();
        // The plain space between two runs needs no text of its own.
        assert_eq!(texts.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["Seal ", "the", "sill"]);
        let x0 = pad(10.0);
        assert!((texts[1].1 - (x0 + text_width("Seal ", 10.0))).abs() < 1e-3 && texts[1].2);
        assert!((texts[2].1 - (texts[1].1 + text_width_b("the", 10.0, true) + text_width(" ", 10.0))).abs() < 1e-3, "after the bold word, by its bold width");
        let unders: Vec<f32> = a.geom().prims.iter().filter_map(|p| if let Prim::Stroke { pts, .. } = p { Some(pts[1].x - pts[0].x) } else { None }).collect();
        assert_eq!(unders.len(), 1);
        assert!((unders[0] - text_width("sill", 10.0)).abs() < 1e-3);
        // All one way: back to the flags, no runs.
        a.style.set_fmts(&[b; 13]);
        assert!(a.style.runs.is_empty() && a.style.bold);
        // Saved and read back; an old file has no runs.
        a.style.set_fmts(&f);
        let json = serde_json::to_string(&a.style).unwrap();
        assert!(json.contains(r#""runs":[{"len":5},{"len":3,"bold":true}"#), "{json}");
        assert_eq!(serde_json::from_str::<Style>(&json).unwrap(), a.style);
    }

    #[test]
    fn typing_carries_formatting() {
        let b = Fmt { bold: true, ..Default::default() };
        let p = Fmt::default();
        let f = [p, p, b, b, p];
        // Typed inside the bold word: bold. At the start: as the first. Deleted: gone.
        assert_eq!(carry_fmts("ab12c", &f, "ab1x2c", None, p, None), [p, p, b, b, b, p]);
        assert_eq!(carry_fmts("ab12c", &f, "zab12c", None, p, None), [p, p, p, b, b, p]);
        assert_eq!(carry_fmts("ab12c", &f, "abc", None, p, None), [p, p, p]);
        // Typing with B turned on at the cursor.
        assert_eq!(carry_fmts("ab12c", &f, "abYY12c", Some(b), p, None), [p, p, b, b, b, b, p]);
        // Replacing a selection takes the formatting before it.
        assert_eq!(carry_fmts("ab12c", &f, "aQc", None, p, None), [p, p, p]);
        assert_eq!(carry_fmts("", &[], "new", None, b, None), [b, b, b]);
        // Repeated text: where the cursor ends up says which copy changed.
        let g = [p, p, p, b, b];
        assert_eq!(carry_fmts("go go", &g, "go", None, p, Some(0)), [b, b], "the first \"go \" deleted");
        assert_eq!(carry_fmts("go go", &g, "go", None, p, Some(2)), [p, p], "the last \" go\" deleted");
        assert_eq!(carry_fmts("go go", &g, "go go go", None, p, Some(6)), [p, p, p, p, p, p, b, b], "\"go \" typed at the start of the second");
        assert_eq!(carry_fmts("go go", &g, "go gogo", None, p, Some(7)), [p, p, p, b, b, b, b], "\"go\" typed at the end");
        assert_eq!(carry_fmts("go go", &g, "go o", None, p, Some(3)), [p, p, p, b], "forward delete: the cursor stays");
        // A cursor away from the change (put back by an undo): "X" taken out of
        // "aXbc", the bold after it kept.
        assert_eq!(carry_fmts("aXbc", &[p, p, b, b], "abc", None, p, Some(3)), [p, b, b]);
        // A cursor that can't be where the edit ended: as without one.
        assert_eq!(carry_fmts("ab12c", &f, "ab1x2c", None, p, Some(0)), [p, p, b, b, b, p]);
        assert_eq!(carry_fmts("ab12c", &f, "abc", None, p, Some(9)), [p, p, p]);
    }

    #[test]
    fn cloud_is_closed_and_outside() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 60.0));
        let pts = cloud_pts(r);
        assert!(pts.len() > 20);
        // Scallops bulge outward: nothing meaningfully inside the rect's interior.
        for p in &pts {
            assert!(!r.shrink(1.0).contains(*p), "{p:?}");
        }
    }
}
