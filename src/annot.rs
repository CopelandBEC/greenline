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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Kind {
    /// Text box with a leader arrow pointing at `anchor`. `pos` is the box's
    /// top-left; height follows the wrapped text.
    Callout { anchor: Pos2, pos: Pos2, box_w: f32, text: String, size: f32 },
    Text { pos: Pos2, box_w: f32, text: String, size: f32 },
    Cloud { rect: Rect },
    Rect { rect: Rect },
    Highlight { rect: Rect },
    Arrow { a: Pos2, b: Pos2 },
    Line { a: Pos2, b: Pos2 },
    Pen { pts: Vec<Pos2> },
}

#[derive(Clone, Debug)]
pub enum Prim {
    Stroke { pts: Vec<Pos2>, closed: bool, width: f32, color: Rgb },
    /// Closed polygon fill with optional outline. `alpha` 0..=255.
    Fill { pts: Vec<Pos2>, color: Rgb, alpha: u8, outline: Option<(Rgb, f32)> },
    /// One line of Helvetica text; `origin` is the left end of the baseline.
    Text { origin: Pos2, text: String, size: f32, color: Rgb },
}

pub const WHITE: Rgb = [255, 255, 255];

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
            // The leader lies between box and anchor; the arrowhead spreads up to arrow_len around the anchor.
            Kind::Callout { anchor, .. } => self.text_box().unwrap().union(Rect::from_center_size(*anchor, Vec2::splat(2.0 * arrow_len(w)))),
            Kind::Text { .. } => self.text_box().unwrap(),
            Kind::Cloud { rect } => rect.expand(cloud_radius(*rect) + w),
            Kind::Rect { rect } | Kind::Highlight { rect } => rect.expand(w),
            Kind::Arrow { a, b } | Kind::Line { a, b } => Rect::from_two_pos(*a, *b).expand(arrow_len(w) + w),
            Kind::Pen { pts } => {
                let mut r = Rect::NOTHING;
                for p in pts {
                    r.extend_with(*p);
                }
                r.expand(w)
            }
        }
    }

    pub fn translate(&mut self, d: Vec2) {
        match &mut self.kind {
            Kind::Callout { pos, .. } | Kind::Text { pos, .. } => *pos += d,
            Kind::Cloud { rect } | Kind::Rect { rect } | Kind::Highlight { rect } => *rect = rect.translate(d),
            Kind::Arrow { a, b } | Kind::Line { a, b } => {
                *a += d;
                *b += d;
            }
            Kind::Pen { pts } => pts.iter_mut().for_each(|p| *p += d),
        }
    }

    /// Distance (view points) from `p` to the annotation; 0 when inside a filled area.
    pub fn distance(&self, p: Pos2) -> f32 {
        if let Some(r) = self.text_box() {
            if r.contains(p) {
                return 0.0;
            }
        }
        if let Kind::Highlight { rect } = &self.kind {
            if rect.contains(p) {
                return 0.0;
            }
        }
        let mut best = f32::INFINITY;
        for prim in self.prims() {
            let (pts, closed) = match &prim {
                Prim::Stroke { pts, closed, .. } => (pts, *closed),
                Prim::Fill { pts, .. } => (pts, true),
                Prim::Text { .. } => continue,
            };
            best = best.min(polyline_dist(pts, closed, p));
        }
        (best - self.width / 2.0).max(0.0)
    }

    pub fn prims(&self) -> Vec<Prim> {
        let c = self.color;
        let w = self.width;
        let mut out = Vec::new();
        match &self.kind {
            Kind::Callout { anchor, text, size, .. } => {
                let r = self.text_box().unwrap();
                if !r.contains(*anchor) {
                    // Leader leaves the box square to its side for a short landing, then bends to the target.
                    let (start, normal) = leader_start(r, *anchor);
                    let room = (*anchor - start).dot(normal) - arrow_len(w) - 1.0;
                    let knee = start + normal * landing(*size).min(room.max(0.0));
                    arrow_path_prims(&mut out, &[start, knee], *anchor, w, c);
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
        let mut a = Annot { id: 1, page: 0, color: [0, 0, 0], width: 2.0, wrap_w: None, kind: Kind::Callout { anchor: pos2(500.0, 0.0), pos: pos2(100.0, 0.0), box_w: 300.0, text: "Short".into(), size: 12.0 } };
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
