//! Measuring: drawing scales, real lengths and areas, and their text.
//!
//! A scale belongs to a page or to a region of one (a detail drawn at its own
//! scale). It is stored as real-world inches per view point, so a PDF exported
//! at its printed size needs no calibration: 1/4" = 1'-0" is 48 real inches per
//! paper inch, i.e. 48/72 in per point. Measurements look their scale up by
//! where they sit, so changing a scale relabels everything it covers.

use crate::annot::{Annot, Kind};
use egui::{Pos2, Rect};
use serde::{Deserialize, Serialize};

/// Points per paper inch.
const PT_PER_IN: f64 = 72.0;
const MM_PER_IN: f64 = 25.4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Scale {
    pub page: usize,
    /// `None`: the whole page. `Some`: a region (view points) drawn at its own scale.
    #[serde(default)]
    pub region: Option<Rect>,
    /// Real-world inches per view point.
    pub per_pt: f64,
    /// How it was set, as shown: `1/4" = 1'-0"` or `Calibrated: 12'-0"`.
    pub label: String,
}

impl Scale {
    /// A drawing scale given as real length per paper length (48 for 1/4" = 1'-0").
    pub fn from_ratio(page: usize, region: Option<Rect>, ratio: f64, label: &str) -> Self {
        Self { page, region, per_pt: ratio / PT_PER_IN, label: label.to_string() }
    }

    /// Real length per paper length.
    #[allow(dead_code)] // for the command line
    pub fn ratio(&self) -> f64 {
        self.per_pt * PT_PER_IN
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Units {
    /// Feet-inches to 1/8", square feet.
    #[default]
    Imperial,
    /// Millimetres or metres, square metres.
    Metric,
}

/// Common drawing scales: label and real length per paper length.
pub const PRESETS: &[(&str, f64)] = &[
    ("1/16\" = 1'-0\"", 192.0),
    ("3/32\" = 1'-0\"", 128.0),
    ("1/8\" = 1'-0\"", 96.0),
    ("3/16\" = 1'-0\"", 64.0),
    ("1/4\" = 1'-0\"", 48.0),
    ("3/8\" = 1'-0\"", 32.0),
    ("1/2\" = 1'-0\"", 24.0),
    ("3/4\" = 1'-0\"", 16.0),
    ("1\" = 1'-0\"", 12.0),
    ("1 1/2\" = 1'-0\"", 8.0),
    ("3\" = 1'-0\"", 4.0),
    ("6\" = 1'-0\"", 2.0),
    ("Full size", 1.0),
    ("1\" = 10'", 120.0),
    ("1\" = 20'", 240.0),
    ("1\" = 30'", 360.0),
    ("1\" = 40'", 480.0),
    ("1\" = 50'", 600.0),
    ("1\" = 100'", 1200.0),
    ("1:5", 5.0),
    ("1:10", 10.0),
    ("1:20", 20.0),
    ("1:25", 25.0),
    ("1:50", 50.0),
    ("1:100", 100.0),
    ("1:200", 200.0),
    ("1:500", 500.0),
];

/// Ratios accepted from `1:N` text on a sheet: the usual metric scales only,
/// so slopes ("1:12") and times aren't taken for scales.
const METRIC_RATIOS: &[u32] = &[5, 10, 20, 25, 50, 100, 200, 250, 500, 1000];

/// The scale that applies at `p` on `page`: the smallest region containing it, else the page's.
pub fn scale_at(scales: &[Scale], page: usize, p: Pos2) -> Option<&Scale> {
    let area = |s: &Scale| s.region.map(|r| r.area()).unwrap_or(f32::INFINITY);
    scales
        .iter()
        .filter(|s| s.page == page && s.region.is_none_or(|r| r.contains(p)))
        .min_by(|a, b| area(a).total_cmp(&area(b)))
}

/// The scale a measurement uses: the one at the centre of its points' bounds.
pub fn scale_for<'s>(scales: &'s [Scale], page: usize, pts: &[Pos2]) -> Option<&'s Scale> {
    let r = pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)));
    scale_at(scales, page, r.center())
}

pub fn length_pt(pts: &[Pos2], closed: bool) -> f64 {
    let mut l: f64 = pts.windows(2).map(|w| (w[1] - w[0]).length() as f64).sum();
    if closed && pts.len() > 2 {
        l += (pts[0] - pts[pts.len() - 1]).length() as f64;
    }
    l
}

/// Shoelace area in square points (a self-crossing outline nets its lobes).
pub fn area_pt2(pts: &[Pos2]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        s += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    (s / 2.0).abs()
}

/// A measurement's real values, in inches and square inches.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// Length of an open line; perimeter of an area.
    pub length: f64,
    /// `Some` for areas.
    pub area: Option<f64>,
    /// Width and height of the points' bounds (a rectangle's sides).
    pub width: f64,
    pub height: f64,
    pub scale: String,
}

/// Real values of a measurement markup, if it has a scale.
pub fn measured(a: &Annot, scales: &[Scale]) -> Option<Measured> {
    let Kind::Measure { pts, area, .. } = &a.kind else { return None };
    let s = scale_for(scales, a.page, pts)?;
    let k = s.per_pt;
    let r = pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)));
    Some(Measured {
        length: length_pt(pts, *area) * k,
        area: area.then(|| area_pt2(pts) * k * k),
        width: r.width() as f64 * k,
        height: r.height() as f64 * k,
        scale: s.label.clone(),
    })
}

/// The value text a measurement shows: its name, then length or area.
pub fn value_text(name: &str, m: Option<&Measured>, units: Units) -> String {
    let v = match m {
        None => "no scale".to_string(),
        Some(m) => match m.area {
            Some(a) => fmt_area(a, units),
            None => fmt_len(m.length, units),
        },
    };
    if name.trim().is_empty() {
        v
    } else {
        format!("{}: {v}", name.trim())
    }
}

/// Refreshes every measurement's value text from the scales. Returns whether any changed.
pub fn relabel(annots: &mut [Annot], scales: &[Scale], units: Units) -> bool {
    let mut changed = false;
    for a in annots.iter_mut() {
        if !matches!(a.kind, Kind::Measure { .. }) {
            continue;
        }
        let m = measured(a, scales);
        if let Kind::Measure { name, value, .. } = &mut a.kind {
            let v = value_text(name, m.as_ref(), units);
            if *value != v {
                *value = v;
                changed = true;
            }
        }
    }
    changed
}

/// Measurements as CSV (UTF-8 with a byte-order mark, so Excel reads "m²"):
/// each value as text and as a number in feet / square feet or metres /
/// square metres. Width and height are of the points' bounds: a rectangle's sides.
pub fn csv(annots: &[&Annot], scales: &[Scale], units: Units) -> String {
    let (lu, au, lk, ak) = match units {
        Units::Imperial => ("ft", "sf", 1.0 / 12.0, 1.0 / 144.0),
        Units::Metric => ("m", "m²", MM_PER_IN / 1000.0, (MM_PER_IN / 1000.0) * (MM_PER_IN / 1000.0)),
    };
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = format!("\u{feff}No,Page,Name,Type,Length or perimeter,Area,Width,Height,Length ({lu}),Area ({au}),Width ({lu}),Height ({lu}),Scale\n");
    for (n, a) in annots.iter().enumerate() {
        let Kind::Measure { name, .. } = &a.kind else { continue };
        let head = format!("{},{},{},{}", n + 1, a.page + 1, q(name.trim()), a.kind_name());
        out += &match measured(a, scales) {
            Some(m) => {
                let area = m.area.map(|v| fmt_area(v, units)).unwrap_or_default();
                let area_n = m.area.map(|v| format!("{:.4}", v * ak)).unwrap_or_default();
                format!(
                    "{head},{},{},{},{},{:.4},{area_n},{:.4},{:.4},{}\n",
                    q(&fmt_len(m.length, units)), q(&area), q(&fmt_len(m.width, units)), q(&fmt_len(m.height, units)),
                    m.length * lk, m.width * lk, m.height * lk, q(&m.scale)
                )
            }
            None => format!("{head},no scale,,,,,,,,\n"),
        };
    }
    out
}

/// Feet-inches to the nearest 1/8" (`12'-6 1/2"`, `3 1/4"`), or mm / m.
pub fn fmt_len(inches: f64, units: Units) -> String {
    match units {
        Units::Metric => {
            let mm = inches * MM_PER_IN;
            if mm.abs() >= 1000.0 {
                format!("{:.3} m", mm / 1000.0)
            } else {
                format!("{mm:.0} mm")
            }
        }
        Units::Imperial => {
            let eighths = (inches * 8.0).round() as i64;
            let (ft, rem) = (eighths / 96, eighths % 96);
            let (whole, frac) = (rem / 8, rem % 8);
            let frac = match frac {
                0 => String::new(),
                f => {
                    let g = gcd(f, 8);
                    format!(" {}/{}", f / g, 8 / g)
                }
            };
            if ft == 0 {
                format!("{whole}{frac}\"")
            } else {
                format!("{ft}'-{whole}{frac}\"")
            }
        }
    }
}

pub fn fmt_area(sq_in: f64, units: Units) -> String {
    match units {
        Units::Imperial => format!("{:.2} sf", sq_in / 144.0),
        Units::Metric => format!("{:.2} m²", sq_in * MM_PER_IN * MM_PER_IN / 1e6),
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Straight quotes and primes, so `1/4″ = 1′-0″` and `1/4'' = 1'-0''` read alike.
fn normalise(s: &str) -> String {
    s.replace("''", "\"").replace(['″', '“', '”'], "\"").replace(['′', '‘', '’'], "'").replace(['–', '—'], "-")
}

/// `6`, `6.5`, `1/2`, `6 1/2`, `6-1/2`.
fn mixed_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let frac = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
        (d != 0.0).then(|| n / d).filter(|v| v.is_finite())
    };
    if let Some(i) = s.rfind([' ', '-']).filter(|_| s.contains('/')) {
        let whole: f64 = s[..i].trim().parse().ok().filter(|v: &f64| v.is_finite())?;
        return Some(whole + frac(&s[i + 1..])?);
    }
    if s.contains('/') {
        return frac(s);
    }
    s.parse().ok().filter(|v: &f64| v.is_finite())
}

/// A typed length in inches: `12'-6"`, `12' 6 1/2"`, `12'`, `150"`, `12.5 ft`,
/// `3.2 m`, `3200 mm`. A bare number is feet (Imperial) or metres (Metric).
pub fn parse_length(s: &str, units: Units) -> Option<f64> {
    parse_length_any(s, units).filter(|v| v.is_finite() && *v > 0.0)
}

fn parse_length_any(s: &str, units: Units) -> Option<f64> {
    let s = normalise(s).to_lowercase().replace("feet", "'").replace("foot", "'").replace("ft", "'").replace("inches", "\"").replace("inch", "\"").replace("in", "\"");
    let s = s.trim();
    for (suffix, per) in [("mm", 1.0 / MM_PER_IN), ("cm", 10.0 / MM_PER_IN), ("m", 1000.0 / MM_PER_IN)] {
        if let Some(n) = s.strip_suffix(suffix) {
            return mixed_number(n).map(|v| v * per).filter(|v| *v > 0.0);
        }
    }
    let (feet, rest) = match s.split_once('\'') {
        Some((f, rest)) => (Some(mixed_number(f)?), rest.trim().trim_start_matches('-').trim()),
        None => (None, s),
    };
    let inches = if rest.is_empty() {
        0.0
    } else if let Some(n) = rest.strip_suffix('"') {
        mixed_number(n)?
    } else if feet.is_none() {
        // Bare number.
        let v = mixed_number(rest)?;
        return Some(match units {
            Units::Imperial => v * 12.0,
            Units::Metric => v * 1000.0 / MM_PER_IN,
        })
        .filter(|v| *v > 0.0);
    } else {
        // 12'-6 (inches without their mark).
        mixed_number(rest)?
    };
    if feet.is_none() && rest.is_empty() {
        return None;
    }
    Some(feet.unwrap_or(0.0) * 12.0 + inches).filter(|v| *v > 0.0)
}

/// Scales written on a sheet (`1/4" = 1'-0"`, `1" = 20'`, `1:50`), each once,
/// as its label and real length per paper length.
pub fn find_scales(text: &str) -> Vec<(String, f64)> {
    let text = normalise(text);
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut push = |label: String, ratio: f64| {
        if (1.0..=20000.0).contains(&ratio) && !out.iter().any(|(_, r)| (r - ratio).abs() < 1e-9) {
            out.push((label, ratio));
        }
    };
    let b = text.as_bytes();
    let paper_ch = |c: u8| c.is_ascii_digit() || b" /-.\"".contains(&c);
    let real_ch = |c: u8| c.is_ascii_digit() || b" /-.'\"".contains(&c);
    for (i, _) in text.match_indices('=') {
        let mut s = i;
        while s > 0 && paper_ch(b[s - 1]) && i - s < 12 {
            s -= 1;
        }
        let mut e = i + 1;
        while e < b.len() && real_ch(b[e]) && e - i < 14 {
            e += 1;
        }
        let (left, right) = (text[s..i].trim(), text[i + 1..e].trim());
        // The paper side is in inches and ends at its inch mark; anything before
        // an earlier mark belongs to a neighbouring scale.
        if !left.ends_with('"') {
            continue;
        }
        let left = left[..left.len() - 1].rsplit_once('"').map_or(left, |(_, l)| &left[left.len() - l.len() - 1..]);
        let left = left.trim_start_matches(['-', ' ']);
        // The real side runs to its inch mark (`1'-0"`), or ends at its foot mark (`20'`).
        let right = match (right.find('"'), right.find('\'')) {
            (Some(q), _) => &right[..=q],
            (None, Some(f)) => &right[..=f],
            (None, None) => continue,
        };
        if let (Some(paper), Some(real)) = (parse_length(left, Units::Imperial), parse_length(right, Units::Imperial)) {
            push(format!("{left} = {right}"), real / paper);
        }
    }
    for (i, _) in text.match_indices(':') {
        let before = text[..i].trim_end();
        // `~1:50` is an approximation written beside the real scale.
        let one = before.ends_with('1') && !before[..before.len() - 1].ends_with(|c: char| c.is_ascii_digit() || c == '.' || c == '~');
        let after = text[i + 1..].trim_start();
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let next = after[digits.len()..].chars().next();
        if one && !next.is_some_and(|c| c.is_ascii_digit() || c == ':' || c == '.') {
            if let Ok(n) = digits.parse::<u32>() {
                if METRIC_RATIOS.contains(&n) {
                    push(format!("1:{n}"), n as f64);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn formats_feet_inches_and_metric() {
        assert_eq!(fmt_len(150.0, Units::Imperial), "12'-6\"");
        assert_eq!(fmt_len(150.5, Units::Imperial), "12'-6 1/2\"");
        assert_eq!(fmt_len(3.25, Units::Imperial), "3 1/4\"");
        assert_eq!(fmt_len(11.97, Units::Imperial), "1'-0\"", "rounds up into the next foot");
        assert_eq!(fmt_len(36.0 + 0.375, Units::Imperial), "3'-0 3/8\"");
        assert_eq!(fmt_len(10.0, Units::Metric), "254 mm");
        assert_eq!(fmt_len(100.0, Units::Metric), "2.540 m");
        assert_eq!(fmt_area(144.0 * 24.5, Units::Imperial), "24.50 sf");
        assert_eq!(fmt_area(1550.0031, Units::Metric), "1.00 m²");
    }

    #[test]
    fn parses_typed_lengths() {
        let p = |s| parse_length(s, Units::Imperial);
        assert_eq!(p("12'-6\""), Some(150.0));
        assert_eq!(p("12' 6 1/2\""), Some(150.5));
        assert_eq!(p("12'-6-1/2\""), Some(150.5));
        assert_eq!(p("12'"), Some(144.0));
        assert_eq!(p("12'-6"), Some(150.0));
        assert_eq!(p("150\""), Some(150.0));
        assert_eq!(p("1/4\""), Some(0.25));
        assert_eq!(p("12.5 ft"), Some(150.0));
        assert_eq!(p("12.5"), Some(150.0), "bare number is feet");
        assert_eq!(p("12′-6″"), Some(150.0));
        assert!((p("3200 mm").unwrap() - 3200.0 / 25.4).abs() < 1e-9);
        assert!((parse_length("3.2", Units::Metric).unwrap() - 3200.0 / 25.4).abs() < 1e-9);
        assert_eq!(p(""), None);
        assert_eq!(p("abc"), None);
        assert_eq!(p("0'"), None);
        assert_eq!(p("inf/1"), None);
        assert_eq!(p("1e400"), None);
        assert_eq!(p("NaN'"), None);
    }

    #[test]
    fn finds_scales_written_on_a_sheet() {
        let text = "DETAIL 3\nSCALE: 1 1/2\" = 1'-0\"\nROOF PLAN SCALE: 1/8\"=1'-0\"  SITE 1\" = 20'-0\"\nWALL 3/4'' = 1' - 0''\nPLAN 1:50  RAMP SLOPE 1:12  MEETING 10:30  SECTION 1 : 100";
        let found = find_scales(text);
        let ratios: Vec<f64> = found.iter().map(|(_, r)| *r).collect();
        assert_eq!(ratios, vec![8.0, 96.0, 240.0, 16.0, 50.0, 100.0], "{found:?}");
        assert_eq!(found[0].0, "1 1/2\" = 1'-0\"");
        assert_eq!(found[1].0, "1/8\" = 1'-0\"");
        assert!(find_scales("NOT TO SCALE  A = B  x=5").is_empty());
        let found = find_scales("SCALE: 1/4” = 1’-0” [~1:50]");
        assert_eq!(found, vec![("1/4\" = 1'-0\"".to_string(), 48.0)], "curly quotes; the approximate metric note is skipped");
    }

    #[test]
    fn scale_lookup_prefers_the_smallest_region() {
        let detail = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 300.0));
        let scales = vec![
            Scale::from_ratio(0, None, 96.0, "1/8\" = 1'-0\""),
            Scale::from_ratio(0, Some(detail), 8.0, "1 1/2\" = 1'-0\""),
            Scale::from_ratio(1, None, 48.0, "1/4\" = 1'-0\""),
        ];
        assert_eq!(scale_at(&scales, 0, pos2(200.0, 200.0)).unwrap().ratio(), 8.0);
        assert_eq!(scale_at(&scales, 0, pos2(500.0, 200.0)).unwrap().ratio(), 96.0);
        assert_eq!(scale_at(&scales, 1, pos2(200.0, 200.0)).unwrap().ratio(), 48.0);
        assert!(scale_at(&scales, 2, pos2(0.0, 0.0)).is_none());
    }

    #[test]
    fn measures_lengths_and_areas_at_scale() {
        // 1/4" = 1'-0": 18 pt on paper (1/4") is 1 ft.
        let scales = vec![Scale::from_ratio(0, None, 48.0, "1/4\" = 1'-0\"")];
        let mk = |pts: Vec<Pos2>, area| Annot {
            id: 1, page: 0, color: [0, 0, 0], width: 1.0, wrap_w: None, group: None,
            kind: Kind::Measure { pts, area, name: String::new(), value: String::new(), size: 10.0 },
        };
        let line = mk(vec![pos2(0.0, 0.0), pos2(18.0 * 3.0, 0.0), pos2(18.0 * 3.0, 18.0 * 4.0)], false);
        let m = measured(&line, &scales).unwrap();
        assert!((m.length - 84.0).abs() < 1e-6, "3 ft + 4 ft: {m:?}");
        // A 3 ft by 5 ft window.
        let window = mk(vec![pos2(100.0, 100.0), pos2(154.0, 100.0), pos2(154.0, 190.0), pos2(100.0, 190.0)], true);
        let m = measured(&window, &scales).unwrap();
        assert!((m.area.unwrap() - 15.0 * 144.0).abs() < 1e-3, "{m:?}");
        assert!((m.length - 16.0 * 12.0).abs() < 1e-6 && (m.width - 36.0).abs() < 1e-6 && (m.height - 60.0).abs() < 1e-6);

        let mut annots = vec![line, window];
        if let Kind::Measure { name, .. } = &mut annots[1].kind {
            *name = "W1".into();
        }
        assert!(relabel(&mut annots, &scales, Units::Imperial));
        assert_eq!(annots[0].label(), "7'-0\"");
        assert_eq!(annots[1].label(), "W1: 15.00 sf");
        assert!(!relabel(&mut annots, &scales, Units::Imperial), "nothing new");
        let rows = csv(&annots.iter().collect::<Vec<_>>(), &scales, Units::Imperial);
        let lines: Vec<&str> = rows.lines().collect();
        assert_eq!(lines[2], "2,1,\"W1\",Area,\"16'-0\"\"\",\"15.00 sf\",\"3'-0\"\"\",\"5'-0\"\"\",16.0000,15.0000,3.0000,5.0000,\"1/4\"\" = 1'-0\"\"\"");
        relabel(&mut annots, &[], Units::Imperial);
        assert_eq!(annots[0].label(), "no scale");
    }

    #[test]
    fn adjacent_scales_stay_apart() {
        let found = find_scales("1/8\"=1'-0\" 1/4\"=1'-0\"  1\" = 20' 3 BAYS");
        let ratios: Vec<f64> = found.iter().map(|(_, r)| *r).collect();
        assert_eq!(ratios, vec![96.0, 48.0, 240.0], "{found:?}");
    }
}
