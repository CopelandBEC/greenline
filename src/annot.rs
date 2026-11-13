//! Markup model and geometry.
//!
//! Annotations live in page-local *view points*: the page as displayed (after
//! /Rotate and CropBox), origin top-left, y down, 1 unit = 1 PDF point.
//! Every annotation is reduced to a list of `Prim`s; the screen painter and
//! the PDF exporter both draw from those, so export matches the screen.

use egui::{pos2, vec2, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

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
    Text { origin: Pos2, text: String, size: f32, color: Rgb },
    /// Library image stretched over `rect` (it keeps its aspect when resized).
    Image { rect: Rect, image: String },
}

pub const WHITE: Rgb = [255, 255, 255];

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
    let w = match c as u32 {
        32..=126 => HELV[(c as u32 - 32) as usize] as f32,
        _ => crate::fonts::advance(c).unwrap_or(556.0),
    };
    w * size / 1000.0
}

pub fn text_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| char_width(c, size)).sum()
}

/// Greedy word wrap. Explicit newlines are kept; over-long words are broken.
pub fn wrap(text: &str, size: f32, max_w: f32) -> Vec<String> {
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
            // Break a word that alone is too wide.
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
    wrap(text, size, (box_w - 2.0 * pad(size)).max(size))
}

pub fn box_rect(pos: Pos2, box_w: f32, text: &str, size: f32) -> Rect {
    let n = text_lines(text, size, box_w).len().max(1) as f32;
    Rect::from_min_size(pos, vec2(box_w, 2.0 * pad(size) + n * line_h(size)))
}

pub fn default_box_w(size: f32) -> f32 {
    16.0 * size
}

// ---------------------------------------------------------------------------

impl Annot {
    /// Rect of the text box for Callout/Text kinds.
    pub fn text_box(&self) -> Option<Rect> {
        match &self.kind {
            Kind::Callout { pos, box_w, text, size, .. } | Kind::Text { pos, box_w, text, size } => {
                Some(box_rect(*pos, *box_w, text, *size))
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
        let (Some(r), Some(size)) = (self.text_box(), self.text_size()) else { return };
        let text = match &self.kind {
            Kind::Callout { text, .. } | Kind::Text { text, .. } => text.clone(),
            _ => return,
        };
        let widest = text_lines(&text, size, r.width()).iter().map(|l| text_width(l.trim_end(), size)).fold(0.0, f32::max);
        // A little slack so float rounding never re-wraps the last word.
        let fit = (widest + 2.0 * pad(size) + 0.5).max(4.0 * size);
        if fit < r.width() {
            self.set_box_w(fit);
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

    pub fn bounds(&self) -> Rect {
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
            Kind::Pen { pts } => {
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
            Kind::Pen { pts } | Kind::Measure { pts, .. } => pts.iter_mut().for_each(|p| *p += d),
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
        let mut best = f32::INFINITY;
        for prim in self.prims() {
            let (pts, closed) = match &prim {
                Prim::Stroke { pts, closed, .. } => (pts, *closed),
                Prim::Fill { pts, .. } => (pts, true),
                Prim::Image { rect, .. } => {
                    best = best.min(polyline_dist(&rect_pts(*rect), true, p));
                    continue;
                }
                Prim::Text { .. } => continue,
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
        let r = r.expand(self.width / 2.0);
        self.prims().iter().any(|prim| match prim {
            Prim::Stroke { pts, closed, .. } => polyline_hits(pts, *closed, r),
            Prim::Fill { pts, .. } => polyline_hits(pts, true, r),
            Prim::Text { .. } | Prim::Image { .. } => false,
        })
    }

    pub fn prims(&self) -> Vec<Prim> {
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
                out.push(Prim::Fill { pts: rect_pts(r), color: WHITE, alpha: 255, outline: Some((c, w)) });
                text_prims(&mut out, r, text, *size, c);
            }
            Kind::Text { text, size, .. } => {
                let r = self.text_box().unwrap();
                text_prims(&mut out, r, text, *size, c);
            }
            Kind::Cloud { rect } => out.push(Prim::Stroke { pts: cloud_pts(*rect), closed: true, width: w, color: c }),
            Kind::Rect { rect } => out.push(Prim::Stroke { pts: rect_pts(*rect), closed: true, width: w, color: c }),
            Kind::Highlight { rect } => out.push(Prim::Fill { pts: rect_pts(*rect), color: c, alpha: 90, outline: None }),
            Kind::Arrow { a, b } => arrow_prims(&mut out, *a, *b, w, c),
            Kind::Line { a, b } => out.push(Prim::Stroke { pts: vec![*a, *b], closed: false, width: w, color: c }),
            Kind::Pen { pts } => out.push(Prim::Stroke { pts: pts.clone(), closed: false, width: w, color: c }),
            Kind::Image { rect, image, .. } => out.push(Prim::Image { rect: *rect, image: image.clone() }),
            Kind::Measure { pts, area, value, size, .. } => {
                if *area && pts.len() >= 3 {
                    out.push(Prim::Fill { pts: pts.clone(), color: c, alpha: 40, outline: Some((c, w)) });
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
                    out.push(Prim::Text { origin, text: value.clone(), size: *size, color: c });
                }
            }
        }
        out
    }
}

fn text_prims(out: &mut Vec<Prim>, r: Rect, text: &str, size: f32, color: Rgb) {
    let x = r.min.x + pad(size);
    for (i, line) in text_lines(text, size, r.width()).into_iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let y = r.min.y + pad(size) + i as f32 * line_h(size) + ascent(size);
        out.push(Prim::Text { origin: pos2(x, y), text: line, size, color });
    }
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

    #[test]
    fn fit_box_shrinks_and_keeps_callout_edge() {
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, kind: Kind::Callout { anchor: pos2(500.0, 0.0), pos: pos2(100.0, 0.0), box_w: 300.0, text: "Short".into(), size: 12.0, leaders: vec![] } };
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
            id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None,
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
        let ann = |kind| Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, kind };
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
        let a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, group: None, kind: Kind::Image { rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 50.0)), image: "x.png".into(), name: "NC seal".into() } };
        assert_eq!(a.distance(pos2(30.0, 30.0)), 0.0);
        assert!((a.distance(pos2(100.0, 30.0)) - 39.0).abs() < 1e-3, "outline distance less half the line width");
        assert!(a.touches(Rect::from_min_size(pos2(20.0, 20.0), vec2(5.0, 5.0))));
        assert_eq!(a.bounds(), Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 50.0)));
        assert_eq!(a.label(), "NC seal");
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
