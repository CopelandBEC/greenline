//! Markups and scales as JSON ops: what an agent writes for `greenline-cli
//! apply` and `render --ops`. Coordinates are view points on the page as
//! displayed (origin top-left, y down, 72 per paper inch); pages count from 1.
//! A batch is checked whole: one bad op and nothing is proposed.

use crate::annot::{self, Annot, Kind, Rgb, COLORS};
use crate::measure::{self, Scale, Units};
use egui::{pos2, Pos2, Rect, Vec2};
use serde_json::{Map, Value};

/// The op reference `greenline-cli guide` prints.
pub const REFERENCE: &str = r##"Ops are a JSON array (or {"ops": [...]}) of objects, each with "op" and "page" (1-based).
Points are [x, y] and rectangles [x0, y0, x1, y1] in page points: origin at the
top-left of the page as displayed, y down, 72 points per paper inch (the same
coordinates `text`, `paths`, `snap` and `render` report).

Common fields: "color" (green, red, blue, black, orange, yellow or "#rrggbb";
default green, highlight yellow), "width" (line width in points, default 2),
"size" (text size in points, default 12; 10 for measurement labels),
"group" (any string: ops on one page with the same group select and move together).

  {"op": "cloud",     "page": 4, "rect": [x0, y0, x1, y1]}
  {"op": "rect",      "page": 4, "rect": [...]}
  {"op": "highlight", "page": 4, "rect": [...]}
  {"op": "arrow",     "page": 4, "from": [x, y], "to": [x, y]}       arrowhead at "to"
  {"op": "line",      "page": 4, "from": [x, y], "to": [x, y]}
  {"op": "callout",   "page": 4, "target": [x, y], "at": [x, y], "text": "...",
                      "targets": [[x, y], ...], "box_width": 200}
        leader arrow to "target" (and to each of "targets"); "at" is the note box's
        top-left; the box wraps at "box_width" (default 16 x size) and shrinks to fit
  {"op": "text",      "page": 4, "at": [x, y], "text": "...", "box_width": 200}
  {"op": "length",    "page": 4, "points": [[x, y], [x, y], ...], "name": "Sill"}
  {"op": "area",      "page": 4, "rect": [...] | "points": [[x, y], ...], "name": "W1"}
        measurements: the value (12'-6", 15.00 sf) comes from the page's scale and is
        drawn after "name"
  {"op": "scale",     "page": 4, "preset": "1/4\" = 1'-0\""}
  {"op": "scale",     "page": 4, "ratio": 48, "label": "1/4\" = 1'-0\""}
  {"op": "scale",     "page": 4, "calibrate": {"from": [x, y], "to": [x, y], "length": "12'-6\""}}
        a drawing scale for measurements on that page; add "region": [x0, y0, x1, y1]
        for a detail drawn at its own scale. "ratio" is real length per paper length
        (48 for 1/4" = 1'-0", 50 for 1:50). Lengths: 12'-6", 12' 6 1/2", 150", 3.2 m, 3200 mm.
"##;

/// What a batch of ops adds.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    pub annots: Vec<Annot>,
    pub scales: Vec<Scale>,
}

/// Parses `ops` (an array, or an object with "ops") against the document's
/// page sizes (view points). Markup ids count up from `first_id`; groups from `first_group`.
pub fn parse(ops: &Value, pages: &[Vec2], first_id: u64, first_group: u64) -> Result<Batch, String> {
    let list = match ops {
        Value::Array(a) => a,
        Value::Object(o) => o.get("ops").and_then(Value::as_array).ok_or("expected a JSON array of ops, or {\"ops\": [...]}")?,
        _ => return Err("expected a JSON array of ops".into()),
    };
    if list.is_empty() {
        return Err("no ops".into());
    }
    let mut batch = Batch::default();
    let mut groups: Vec<(usize, String, u64)> = Vec::new();
    for (i, op) in list.iter().enumerate() {
        let o = op.as_object().ok_or_else(|| format!("op {}: not an object", i + 1))?;
        let name = o.get("op").and_then(Value::as_str).unwrap_or("?").to_string();
        let ctx = |e: String| format!("op {} ({name}): {e}", i + 1);
        let f = Fields { o, pages };
        let page = f.page().map_err(ctx)?;
        if name == "scale" {
            let sc = f.scale(page).map_err(ctx)?;
            if batch.scales.iter().any(|s| s.page == sc.page && s.region == sc.region) {
                let what = if sc.region.is_some() { "that region" } else { "the page" };
                return Err(ctx(format!("{what} already has a scale earlier in this batch")));
            }
            batch.scales.push(sc);
            continue;
        }
        let kind = f.kind(&name, page).map_err(ctx)?;
        let default_color = if name == "highlight" { COLORS[5].0 } else { COLORS[0].0 };
        let color = f.color(default_color).map_err(ctx)?;
        let width = f.num_or("width", 2.0).map_err(ctx)?;
        if !(0.25..=12.0).contains(&width) {
            return Err(ctx("\"width\" must be 0.25 to 12 points".into()));
        }
        let group = match o.get("group") {
            None | Some(Value::Null) => None,
            Some(g) => {
                let key = g.as_str().map(str::to_string).unwrap_or_else(|| g.to_string());
                let next = first_group + groups.len() as u64;
                Some(match groups.iter().find(|(p, k, _)| *p == page && *k == key) {
                    Some((_, _, id)) => *id,
                    None => {
                        groups.push((page, key, next));
                        next
                    }
                })
            }
        };
        let mut a = Annot { id: first_id + batch.annots.len() as u64, page, color, width, kind, wrap_w: None, group };
        if let Kind::Callout { box_w, .. } | Kind::Text { box_w, .. } = a.kind {
            a.wrap_w = Some(box_w);
            a.fit_box_w();
        }
        batch.annots.push(a);
    }
    Ok(batch)
}

/// Relabels `batch`'s measurements against `existing` scales plus its own.
pub fn relabel(batch: &mut Batch, existing: &[Scale], units: Units) {
    let scales = merged_scales(existing, &batch.scales);
    measure::relabel(&mut batch.annots, &scales, units);
}

/// `existing` with `new` replacing any scale for the same page and region.
pub fn merged_scales(existing: &[Scale], new: &[Scale]) -> Vec<Scale> {
    let mut out = existing.to_vec();
    for n in new {
        out.retain(|s| !(s.page == n.page && s.region == n.region));
        out.push(n.clone());
    }
    out
}

struct Fields<'a> {
    o: &'a Map<String, Value>,
    pages: &'a [Vec2],
}

impl Fields<'_> {
    fn page(&self) -> Result<usize, String> {
        let n = self.o.get("page").and_then(Value::as_u64).ok_or("needs \"page\" (1-based)")? as usize;
        if n == 0 || n > self.pages.len() {
            return Err(format!("page {n} out of range: the document has {} pages", self.pages.len()));
        }
        Ok(n - 1)
    }

    fn num(&self, key: &str) -> Result<Option<f32>, String> {
        match self.o.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().filter(|v| v.is_finite()).map(|v| Some(v as f32)).ok_or(format!("\"{key}\" must be a number")),
        }
    }

    fn num_or(&self, key: &str, default: f32) -> Result<f32, String> {
        Ok(self.num(key)?.unwrap_or(default))
    }

    fn text(&self, key: &str) -> Result<String, String> {
        let t = self.o.get(key).and_then(Value::as_str).ok_or(format!("needs \"{key}\" (text)"))?;
        if t.trim().is_empty() {
            return Err(format!("\"{key}\" is empty"));
        }
        Ok(t.trim_end().to_string())
    }

    fn opt_text(&self, key: &str) -> String {
        self.o.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
    }

    fn point_of(v: &Value, key: &str, page: Vec2) -> Result<Pos2, String> {
        let a = v.as_array().filter(|a| a.len() == 2).ok_or(format!("\"{key}\" must be [x, y]"))?;
        let (x, y) = (a[0].as_f64().filter(|v| v.is_finite()), a[1].as_f64().filter(|v| v.is_finite()));
        let (Some(x), Some(y)) = (x, y) else { return Err(format!("\"{key}\" must be [x, y] numbers")) };
        let p = pos2(x as f32, y as f32);
        // A little slack for points placed on the page edge.
        if !Rect::from_min_size(Pos2::ZERO, page).expand(1.0).contains(p) {
            return Err(format!("\"{key}\" [{x}, {y}] is off the page ({} x {} points)", page.x, page.y));
        }
        Ok(p)
    }

    fn point(&self, key: &str, page: usize) -> Result<Pos2, String> {
        Self::point_of(self.o.get(key).ok_or(format!("needs \"{key}\": [x, y]"))?, key, self.pages[page])
    }

    fn points(&self, key: &str, page: usize, min: usize) -> Result<Vec<Pos2>, String> {
        let a = self.o.get(key).and_then(Value::as_array).ok_or(format!("needs \"{key}\": [[x, y], ...]"))?;
        if a.len() < min {
            return Err(format!("\"{key}\" needs at least {min} points"));
        }
        a.iter().map(|v| Self::point_of(v, key, self.pages[page])).collect()
    }

    fn rect(&self, key: &str, page: usize) -> Result<Rect, String> {
        let a = self.o.get(key).and_then(Value::as_array).filter(|a| a.len() == 4).ok_or(format!("needs \"{key}\": [x0, y0, x1, y1]"))?;
        let v: Vec<f64> = a.iter().filter_map(|v| v.as_f64().filter(|v| v.is_finite())).collect();
        if v.len() != 4 {
            return Err(format!("\"{key}\" must be four numbers"));
        }
        let (p, q) = (pos2(v[0] as f32, v[1] as f32), pos2(v[2] as f32, v[3] as f32));
        let bounds = Rect::from_min_size(Pos2::ZERO, self.pages[page]).expand(1.0);
        if !bounds.contains(p) || !bounds.contains(q) {
            return Err(format!("\"{key}\" is off the page ({} x {} points)", self.pages[page].x, self.pages[page].y));
        }
        let r = Rect::from_two_pos(p, q);
        if r.width() < 1.0 || r.height() < 1.0 {
            return Err(format!("\"{key}\" is empty"));
        }
        Ok(r)
    }

    fn color(&self, default: Rgb) -> Result<Rgb, String> {
        let Some(v) = self.o.get("color").filter(|v| !v.is_null()) else { return Ok(default) };
        let s = v.as_str().ok_or("\"color\" must be a name or \"#rrggbb\"")?.trim();
        if let Some(hex) = s.strip_prefix('#') {
            let n = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6).ok_or("\"color\" hex must be #rrggbb")?;
            return Ok([(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        }
        COLORS.iter().find(|(_, name)| name.eq_ignore_ascii_case(s)).map(|(c, _)| *c).ok_or_else(|| {
            let names: Vec<String> = COLORS.iter().map(|(_, n)| n.to_lowercase()).collect();
            format!("unknown colour \"{s}\": use {} or #rrggbb", names.join(", "))
        })
    }

    fn size(&self, default: f32) -> Result<f32, String> {
        let s = self.num_or("size", default)?;
        if !(4.0..=72.0).contains(&s) {
            return Err("\"size\" must be 4 to 72 points".into());
        }
        Ok(s)
    }

    fn box_w(&self, size: f32) -> Result<f32, String> {
        let w = self.num_or("box_width", annot::default_box_w(size))?;
        if w < 4.0 * size || w > 2000.0 {
            return Err(format!("\"box_width\" must be at least {} points (4 x size)", 4.0 * size));
        }
        Ok(w)
    }

    fn kind(&self, name: &str, page: usize) -> Result<Kind, String> {
        Ok(match name {
            "cloud" => Kind::Cloud { rect: self.rect("rect", page)? },
            "rect" => Kind::Rect { rect: self.rect("rect", page)? },
            "highlight" => Kind::Highlight { rect: self.rect("rect", page)? },
            "arrow" => Kind::Arrow { a: self.point("from", page)?, b: self.point("to", page)? },
            "line" => Kind::Line { a: self.point("from", page)?, b: self.point("to", page)? },
            "callout" => {
                let size = self.size(12.0)?;
                let leaders = match self.o.get("targets") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(_) => self.points("targets", page, 1)?,
                };
                Kind::Callout { anchor: self.point("target", page)?, pos: self.point("at", page)?, box_w: self.box_w(size)?, text: self.text("text")?, size, leaders }
            }
            "text" => {
                let size = self.size(12.0)?;
                Kind::Text { pos: self.point("at", page)?, box_w: self.box_w(size)?, text: self.text("text")?, size }
            }
            "length" => Kind::Measure { pts: self.points("points", page, 2)?, area: false, name: self.opt_text("name"), value: String::new(), size: self.size(10.0)? },
            "area" => {
                let pts = if self.o.contains_key("rect") { annot::rect_pts(self.rect("rect", page)?) } else { self.points("points", page, 3)? };
                if measure::area_pt2(&pts) < 1.0 {
                    return Err("the area is empty".into());
                }
                Kind::Measure { pts, area: true, name: self.opt_text("name"), value: String::new(), size: self.size(10.0)? }
            }
            "" | "?" => return Err("needs \"op\"".into()),
            other => return Err(format!("unknown op \"{other}\": cloud, rect, highlight, arrow, line, callout, text, length, area or scale")),
        })
    }

    fn scale(&self, page: usize) -> Result<Scale, String> {
        let region = if self.o.contains_key("region") { Some(self.rect("region", page)?) } else { None };
        let label = self.opt_text("label");
        if let Some(p) = self.o.get("preset").and_then(Value::as_str) {
            let norm = |s: &str| s.replace([' ', '-'], "").replace(['″', '”'], "\"").replace(['′', '’'], "'").to_lowercase();
            let (l, r) = measure::PRESETS.iter().find(|(l, _)| norm(l) == norm(p)).ok_or_else(|| {
                format!("unknown preset \"{p}\": use one of {} or give \"ratio\"", measure::PRESETS.iter().map(|(l, _)| *l).collect::<Vec<_>>().join(", "))
            })?;
            return Ok(Scale::from_ratio(page, region, *r, if label.is_empty() { l } else { &label }));
        }
        if let Some(r) = self.num("ratio")? {
            if !(r > 0.0 && r <= 100_000.0) {
                return Err("\"ratio\" must be positive (real length per paper length)".into());
            }
            let label = if label.is_empty() { format!("1:{}", trim(r as f64)) } else { label };
            return Ok(Scale::from_ratio(page, region, r as f64, &label));
        }
        if let Some(c) = self.o.get("calibrate").and_then(Value::as_object) {
            let sub = Fields { o: c, pages: self.pages };
            let (a, b) = (sub.point("from", page)?, sub.point("to", page)?);
            let len = (b - a).length() as f64;
            if len < 5.0 {
                return Err("the calibration line is too short (under 5 points)".into());
            }
            let text = sub.text("length")?;
            let real = measure::parse_length(&text, Units::Imperial).ok_or(format!("can't read the length \"{text}\": try 12'-6\", 150\", 3.2 m"))?;
            let label = if label.is_empty() { format!("Calibrated: {}", measure::fmt_len(real, Units::Imperial)) } else { label };
            return Ok(Scale { page, region, per_pt: real / len, label });
        }
        Err("a scale needs \"preset\", \"ratio\" or \"calibrate\"".into())
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A labelled grid every `step` points for `render --grid`, so a model reading
/// the image can tell where things are.
pub fn grid(page: usize, size: Vec2, step: f32) -> Vec<Annot> {
    let mut out = Vec::new();
    let c: Rgb = [0, 150, 255];
    let mk = |kind| Annot { id: 0, page, color: c, width: 0.3, kind, wrap_w: None, group: None };
    let mut x = step;
    while x < size.x {
        out.push(mk(Kind::Line { a: pos2(x, 0.0), b: pos2(x, size.y) }));
        out.push(mk(Kind::Text { pos: pos2(x + 1.0, 1.0), box_w: 40.0, text: format!("{x}"), size: 6.0 }));
        x += step;
    }
    let mut y = step;
    while y < size.y {
        out.push(mk(Kind::Line { a: pos2(0.0, y), b: pos2(size.x, y) }));
        out.push(mk(Kind::Text { pos: pos2(1.0, y + 1.0), box_w: 40.0, text: format!("{y}"), size: 6.0 }));
        y += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;
    use serde_json::json;

    fn pages() -> Vec<Vec2> {
        vec![vec2(792.0, 612.0), vec2(1224.0, 792.0)]
    }

    #[test]
    fn parses_a_takeoff_batch() {
        let ops = json!([
            {"op": "scale", "page": 2, "preset": "1/4\" = 1'-0\""},
            {"op": "area", "page": 2, "rect": [100, 100, 154, 190], "name": "W1", "group": "w1"},
            {"op": "callout", "page": 2, "target": [154, 100], "at": [200, 50], "text": "W1 3'-0\" x 5'-0\"", "group": "w1", "color": "red"},
            {"op": "length", "page": 1, "points": [[10, 10], [82, 10]]},
            {"op": "cloud", "page": 1, "rect": [300, 300, 200, 250], "color": "#112233"}
        ]);
        let mut b = parse(&ops, &pages(), 100, 7).unwrap();
        assert_eq!(b.scales.len(), 1);
        assert_eq!(b.annots.len(), 4);
        assert_eq!(b.annots[0].id, 100);
        assert_eq!((b.annots[0].group, b.annots[1].group), (Some(7), Some(7)));
        assert_eq!(b.annots[1].color, COLORS[1].0);
        assert_eq!(b.annots[3].color, [0x11, 0x22, 0x33]);
        assert_eq!(b.annots[0].page, 1, "pages count from 1");
        relabel(&mut b, &[], Units::Imperial);
        assert_eq!(b.annots[0].label(), "W1: 15.00 sf");
        assert_eq!(b.annots[2].label(), "no scale", "page 1 has none");
    }

    #[test]
    fn scales_by_ratio_and_calibration() {
        let ops = json!([
            {"op": "scale", "page": 1, "ratio": 50},
            {"op": "scale", "page": 1, "region": [0, 0, 300, 300], "calibrate": {"from": [10, 10], "to": [82, 10], "length": "4'"}}
        ]);
        let b = parse(&ops, &pages(), 1, 1).unwrap();
        assert_eq!(b.scales[0].label, "1:50");
        assert!((b.scales[0].ratio() - 50.0).abs() < 1e-9);
        assert!((b.scales[1].ratio() - 48.0).abs() < 1e-6, "{:?}", b.scales[1]);
        assert!(b.scales[1].region.is_some());
    }

    #[test]
    fn rejects_bad_ops_with_their_number() {
        let bad = |ops: Value| parse(&ops, &pages(), 1, 1).unwrap_err();
        assert!(bad(json!([{"op": "cloud", "page": 3, "rect": [0, 0, 10, 10]}])).contains("out of range"));
        assert!(bad(json!([{"op": "cloud", "page": 1, "rect": [0, 0, 10, 10]}, {"op": "cloud", "page": 1, "rect": [0, 0, 900, 10]}])).starts_with("op 2 (cloud): "));
        assert!(bad(json!([{"op": "callout", "page": 1, "target": [5, 5], "at": [50, 50], "text": " "}])).contains("empty"));
        assert!(bad(json!([{"op": "circle", "page": 1}])).contains("unknown op"));
        assert!(bad(json!([{"op": "rect", "page": 1, "rect": [0, 0, 10, 10], "color": "teal"}])).contains("unknown colour"));
        assert!(bad(json!([{"op": "scale", "page": 1, "preset": "1/5\" = 1'"}])).contains("unknown preset"));
        assert!(bad(json!([])).contains("no ops"));
        assert!(bad(json!([{"op": "scale", "page": 1, "ratio": 48}, {"op": "scale", "page": 1, "ratio": 96}])).contains("already has a scale"));
        // Later scales win when merging, wherever they come from.
        let s = |r: f64| Scale::from_ratio(0, None, r, "x");
        let m = merged_scales(&[s(12.0)], &[s(48.0), s(96.0)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].ratio(), 96.0);
    }
}
