//! Reading a drawing for the command line: page text and vector lines in view
//! points (the coordinates markups use: displayed page, origin top-left, y
//! down, 1 unit = 1 PDF point), region renders, and snapping a rough
//! rectangle to the lines drawn round it.

use crate::render::{find_pdfium, ViewToUser};
use egui::{pos2, Pos2, Rect};
use pdfium_render::prelude::*;
use serde::Serialize;

/// PDFium for this process. It can be bound only once, so the command line
/// and the tests share this one; the app's worker thread binds its own.
pub fn pdfium() -> anyhow::Result<&'static Pdfium> {
    static PDFIUM: std::sync::OnceLock<Result<Pdfium, String>> = std::sync::OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let lib = find_pdfium().map_err(|e| e.to_string())?;
            Pdfium::bind_to_library(&lib).map(Pdfium::new).map_err(|e| format!("loading {}: {e}", lib.display()))
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[derive(Serialize, Clone, Debug)]
pub struct TextRun {
    pub text: String,
    /// `[x0, y0, x1, y1]` in view points.
    #[serde(rename = "box")]
    pub rect: [f64; 4],
}

/// `[x0, y0, x1, y1]` to the hundredth of a point.
pub fn rect4(r: Rect) -> [f64; 4] {
    let k = |v: f32| (v as f64 * 100.0).round() / 100.0;
    [k(r.min.x), k(r.min.y), k(r.max.x), k(r.max.y)]
}

fn view_rect(m: &ViewToUser, r: &PdfRect) -> Rect {
    let (l, b, rt, t) = (r.left().value, r.bottom().value, r.right().value, r.top().value);
    [pos2(l, b), pos2(rt, b), pos2(rt, t), pos2(l, t)].iter().fold(Rect::NOTHING, |acc, p| {
        let v = m.unmap(*p);
        acc.union(Rect::from_min_max(v, v))
    })
}

/// The page's text as PDFium groups it (runs on one line in one font), each with its box.
pub fn text_runs(page: &PdfPage) -> anyhow::Result<Vec<TextRun>> {
    let m = ViewToUser::for_page(page)?;
    let text = page.text()?;
    let mut out = Vec::new();
    for seg in text.segments().iter() {
        let t = seg.text();
        if t.trim().is_empty() {
            continue;
        }
        out.push(TextRun { text: t.trim().to_string(), rect: rect4(view_rect(&m, &seg.bounds())) });
    }
    Ok(out)
}

/// Straight line segments drawn on the page (curves skipped), in view points,
/// from paths on the page and inside form XObjects.
pub fn segments(page: &PdfPage) -> anyhow::Result<Vec<[Pos2; 2]>> {
    let m = ViewToUser::for_page(page)?;
    let mut out = Vec::new();
    for obj in page.objects().iter() {
        collect(&obj, &[], &m, &mut out, 0);
    }
    Ok(out)
}

/// `chain` is the matrices of the forms the object sits in, innermost first.
fn collect(obj: &PdfPageObject, chain: &[PdfMatrix], m: &ViewToUser, out: &mut Vec<[Pos2; 2]>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(own) = obj.matrix() else { return };
    let mut chain2 = vec![own];
    chain2.extend_from_slice(chain);
    let to_view = |x: f32, y: f32| {
        let (mut x, mut y) = (PdfPoints::new(x), PdfPoints::new(y));
        for mat in &chain2 {
            (x, y) = mat.apply_to_points(x, y);
        }
        m.unmap(pos2(x.value, y.value))
    };
    if let Some(path) = obj.as_path_object() {
        let segs = path.segments().raw();
        let (mut start, mut cur): (Option<Pos2>, Option<Pos2>) = (None, None);
        let mut bezier = 0;
        for seg in segs.iter() {
            let (x, y) = seg.point();
            let p = to_view(x.value, y.value);
            match seg.segment_type() {
                PdfPathSegmentType::MoveTo => {
                    start = Some(p);
                    cur = Some(p);
                    bezier = 0;
                }
                PdfPathSegmentType::LineTo => {
                    if let Some(c) = cur {
                        out.push([c, p]);
                    }
                    cur = Some(p);
                }
                // Three points per curve: two controls, then the end.
                PdfPathSegmentType::BezierTo => {
                    bezier += 1;
                    if bezier % 3 == 0 {
                        cur = Some(p);
                    }
                }
                PdfPathSegmentType::Unknown => {}
            }
            if seg.is_close() {
                if let (Some(c), Some(s)) = (cur, start) {
                    if (c - s).length() > 1e-3 {
                        out.push([c, s]);
                    }
                    cur = Some(s);
                }
            }
        }
    } else if let Some(form) = obj.as_x_object_form_object() {
        for child in form.iter() {
            collect(&child, &chain2, m, out, depth + 1);
        }
    }
}

/// Whether segment `s` passes through `r`.
pub fn crosses(s: &[Pos2; 2], r: Rect) -> bool {
    if r.contains(s[0]) || r.contains(s[1]) {
        return true;
    }
    let ann = crate::annot::Annot {
        id: 0, page: 0, color: [0; 3], width: 0.0, wrap_w: None, group: None,
        kind: crate::annot::Kind::Line { a: s[0], b: s[1] },
    };
    ann.touches(r)
}

/// Fits a rough rectangle to the lines drawn round it: each edge moves to the
/// nearest drawn line within `tol` points that runs along it (parallel within
/// about 1°, overlapping at least half its length). Returns the rectangle and
/// which edges (left, top, right, bottom) found a line.
pub fn snap(rough: Rect, segs: &[[Pos2; 2]], tol: f32) -> (Rect, [bool; 4]) {
    let mut r = rough;
    let mut hit = [false; 4];
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
    // Vertical edges: x = left / right, spanning top..bottom.
    for (i, x) in [(0, rough.min.x), (2, rough.max.x)] {
        let best = segs.iter().filter_map(|s| {
            let d = s[1] - s[0];
            if d.y.abs() < 1.0 || d.x.abs() > d.y.abs() * 0.02 {
                return None;
            }
            let sx = (s[0].x + s[1].x) / 2.0;
            let ov = overlap(s[0].y.min(s[1].y), s[0].y.max(s[1].y), rough.min.y, rough.max.y);
            ((sx - x).abs() <= tol && ov >= 0.5 * rough.height()).then_some(sx)
        }).min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()));
        if let Some(sx) = best {
            if i == 0 { r.min.x = sx } else { r.max.x = sx }
            hit[i] = true;
        }
    }
    for (i, y) in [(1, rough.min.y), (3, rough.max.y)] {
        let best = segs.iter().filter_map(|s| {
            let d = s[1] - s[0];
            if d.x.abs() < 1.0 || d.y.abs() > d.x.abs() * 0.02 {
                return None;
            }
            let sy = (s[0].y + s[1].y) / 2.0;
            let ov = overlap(s[0].x.min(s[1].x), s[0].x.max(s[1].x), rough.min.x, rough.max.x);
            ((sy - y).abs() <= tol && ov >= 0.5 * rough.width()).then_some(sy)
        }).min_by(|a, b| (a - y).abs().total_cmp(&(b - y).abs()));
        if let Some(sy) = best {
            if i == 1 { r.min.y = sy } else { r.max.y = sy }
            hit[i] = true;
        }
    }
    (r, hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_moves_edges_to_nearby_lines() {
        let window = Rect::from_min_max(pos2(100.0, 200.0), pos2(154.0, 290.0));
        let mut segs: Vec<[Pos2; 2]> = crate::annot::rect_pts(window).windows(2).map(|w| [w[0], w[1]]).collect();
        segs.push([window.left_bottom(), window.left_top()]);
        // A short tick near the top edge doesn't count; a far line doesn't either.
        segs.push([pos2(120.0, 203.0), pos2(124.0, 203.0)]);
        segs.push([pos2(0.0, 240.0), pos2(400.0, 240.0)]);
        let rough = Rect::from_min_max(pos2(103.0, 196.0), pos2(151.0, 293.0));
        let (r, hit) = snap(rough, &segs, 6.0);
        assert_eq!(hit, [true; 4]);
        assert_eq!(r, window);
        let (r, hit) = snap(Rect::from_min_max(pos2(300.0, 300.0), pos2(350.0, 350.0)), &segs, 6.0);
        assert_eq!(hit, [false; 4]);
        assert_eq!(r.min, pos2(300.0, 300.0));
    }
}
