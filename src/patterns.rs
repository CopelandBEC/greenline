//! Line types and hatch patterns: a few built in, plus Bluebeam line type
//! (`.blx`) and hatch (`.bhx`) files from the user's own library and from a
//! shared library folder (a firm's standards, say).
//!
//! A markup carries its whole pattern (not just a name), so its markups file
//! draws the same on a machine without that library.

use crate::annot::{Prim, Rgb};
use egui::{pos2, vec2, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineType {
    pub name: String,
    /// Lengths are multiples of the line width (otherwise points).
    pub scale_with_width: bool,
    /// Parallel copies of the line, each with its own pattern.
    pub lines: Vec<LinePattern>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinePattern {
    /// Distance to the side of the path (positive: right of the direction drawn).
    pub offset: f32,
    pub comps: Vec<Comp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Comp {
    Dash(f32),
    Space(f32),
    Dot,
    /// A little drawing repeated along the line: `left` and `right` are the room
    /// it takes before and after its origin, `stretch` scales it along the line.
    /// Each path is a polyline in units across (x) and to the right (y) of the line.
    Symbol { left: f32, right: f32, stretch: f32, fill: bool, paths: Vec<Vec<Pos2>> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hatch {
    pub name: String,
    /// One tile, in points.
    pub tile: Vec2,
    /// Stroke width, in points.
    pub line_width: f32,
    /// Segments within the unit tile (0..1, y down); they may run over its edge.
    pub lines: Vec<[Pos2; 2]>,
    /// Dots within the unit tile, with their diameter in points.
    pub dots: Vec<(Pos2, f32)>,
}

/// A hatch on a markup, at a scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HatchFill {
    pub pattern: Arc<Hatch>,
    #[serde(default = "one")]
    pub scale: f32,
}

fn one() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------
// Built-ins

pub fn builtin_line_types() -> Vec<LineType> {
    let lt = |name: &str, comps: Vec<Comp>| LineType { name: name.into(), scale_with_width: true, lines: vec![LinePattern { offset: 0.0, comps }] };
    vec![
        lt("Dashed", vec![Comp::Dash(6.0), Comp::Space(3.5)]),
        lt("Dotted", vec![Comp::Dot, Comp::Space(2.5)]),
        lt("Dash-dot", vec![Comp::Dash(8.0), Comp::Space(2.5), Comp::Dot, Comp::Space(2.5)]),
    ]
}

pub fn builtin_hatches() -> Vec<Hatch> {
    let h = |name: &str, lines: Vec<[Pos2; 2]>| Hatch { name: name.into(), tile: Vec2::splat(8.0), line_width: 0.5, lines, dots: Vec::new() };
    let diag = [pos2(0.0, 1.0), pos2(1.0, 0.0)];
    vec![
        h("Diagonal", vec![diag]),
        h("Crosshatch", vec![diag, [pos2(0.0, 0.0), pos2(1.0, 1.0)]]),
        h("Horizontal", vec![[pos2(0.0, 0.5), pos2(1.0, 0.5)]]),
    ]
}

// ---------------------------------------------------------------------------
// Bluebeam files

/// The attributes of an element, unescaped.
fn attrs(e: &quick_xml::events::BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| (String::from_utf8_lossy(a.key.as_ref()).into_owned(), a.normalized_value(quick_xml::XmlVersion::default()).map(|v| v.into_owned()).unwrap_or_default()))
        .collect()
}

fn get<'a>(a: &'a [(String, String)], k: &str) -> Option<&'a str> {
    a.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

fn num(a: &[(String, String)], k: &str) -> f32 {
    get(a, k).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0)
}

/// Line types in a Bluebeam `.blx` file.
pub fn parse_blx(xml: &str) -> anyhow::Result<Vec<LineType>> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut out: Vec<LineType> = Vec::new();
    let mut seen_root = false;
    // The symbol being read, and its current subpath; curve points come in threes.
    let mut curve: Vec<Pos2> = Vec::new();
    loop {
        let ev = r.read_event()?;
        let (e, empty) = match &ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) if e.name().as_ref() == b"Component" => {
                curve.clear();
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let a = attrs(e);
        match e.name().as_ref() {
            b"BluebeamRevuLineStyleSet" => seen_root = true,
            b"LineStyle" => out.push(LineType {
                name: get(&a, "Name").unwrap_or("Line type").trim().to_string(),
                scale_with_width: get(&a, "ScaleWithLineWidth").is_some_and(|v| v.eq_ignore_ascii_case("true")),
                lines: Vec::new(),
            }),
            b"Line" => {
                if let Some(lt) = out.last_mut() {
                    lt.lines.push(LinePattern { offset: num(&a, "Offset"), comps: Vec::new() });
                }
            }
            b"Component" => {
                let Some(line) = out.last_mut().and_then(|lt| lt.lines.last_mut()) else { continue };
                let comp = match get(&a, "Type").unwrap_or("") {
                    "Dash" => Comp::Dash(num(&a, "Width").max(0.0)),
                    "Space" => Comp::Space(num(&a, "Width").max(0.0)),
                    "Dot" => Comp::Dot,
                    "Vector" => {
                        let stretch = if num(&a, "Scale") > 0.0 { num(&a, "Scale") } else { 1.0 };
                        let fill = get(&a, "Fill").is_some_and(|v| v.eq_ignore_ascii_case("true"));
                        Comp::Symbol { left: num(&a, "Left").max(0.0), right: num(&a, "Right").max(0.0), stretch, fill, paths: Vec::new() }
                    }
                    _ => continue,
                };
                line.comps.push(comp);
                curve.clear();
                let _ = empty;
            }
            b"Point" => {
                let Some(Comp::Symbol { paths, .. }) = out.last_mut().and_then(|lt| lt.lines.last_mut()).and_then(|l| l.comps.last_mut()) else { continue };
                let p = pos2(num(&a, "X"), num(&a, "Y"));
                match get(&a, "Type").unwrap_or("") {
                    "Start" => {
                        paths.push(vec![p]);
                        curve.clear();
                    }
                    "Curve" => {
                        curve.push(p);
                        if curve.len() == 3 {
                            if let Some(path) = paths.last_mut() {
                                let p0 = *path.last().unwrap();
                                for k in 1..=8 {
                                    path.push(bezier(p0, curve[0], curve[1], curve[2], k as f32 / 8.0));
                                }
                            }
                            curve.clear();
                        }
                    }
                    _ => {
                        if let Some(path) = paths.last_mut() {
                            path.push(p);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !seen_root {
        anyhow::bail!("not a Bluebeam line type file");
    }
    out.retain(|lt| lt.lines.iter().any(|l| l.comps.iter().any(|c| !matches!(c, Comp::Space(_)))));
    Ok(out)
}

fn bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let v = p0.to_vec2() * (u * u * u) + p1.to_vec2() * (3.0 * u * u * t) + p2.to_vec2() * (3.0 * u * t * t) + p3.to_vec2() * (t * t * t);
    v.to_pos2()
}

/// Hatches in a Bluebeam `.bhx` file.
pub fn parse_bhx(xml: &str) -> anyhow::Result<Vec<Hatch>> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml.trim_start_matches('\u{feff}'));
    struct Raw {
        name: String,
        sx: f32,
        sy: f32,
        size: f32,
        width: f32,
        lines: Vec<[Pos2; 2]>,
        dots: Vec<(Pos2, bool)>,
    }
    let mut raw: Vec<Raw> = Vec::new();
    let mut seen_root = false;
    let mut field: Option<String> = None;
    loop {
        match r.read_event()? {
            Event::Start(e) | Event::Empty(e) => {
                let a = attrs(&e);
                match e.name().as_ref() {
                    b"BluebeamRevuHatchSet" => seen_root = true,
                    b"Hatch" => raw.push(Raw {
                        name: get(&a, "Name").unwrap_or("Hatch").trim().to_string(),
                        sx: 1.0,
                        sy: 1.0,
                        size: 72.0,
                        width: 0.5,
                        lines: Vec::new(),
                        dots: Vec::new(),
                    }),
                    b"Element" => {
                        let Some(h) = raw.last_mut() else { continue };
                        // Bluebeam's tile has y up; ours has y down.
                        match get(&a, "Type").unwrap_or("") {
                            "Line" => h.lines.push([pos2(num(&a, "X1"), 1.0 - num(&a, "Y1")), pos2(num(&a, "X2"), 1.0 - num(&a, "Y2"))]),
                            "Dot" => h.dots.push((pos2(num(&a, "X"), 1.0 - num(&a, "Y")), get(&a, "Size").is_some_and(|s| s.eq_ignore_ascii_case("large")))),
                            _ => {}
                        }
                    }
                    name => field = Some(String::from_utf8_lossy(name).into_owned()),
                }
            }
            Event::Text(t) => {
                let (Some(f), Some(h)) = (field.as_deref(), raw.last_mut()) else { continue };
                let v = t.decode().map(|s| s.trim().to_string()).unwrap_or_default();
                let n = v.parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0);
                match f {
                    "ScaleX" => h.sx = n.unwrap_or(1.0),
                    "ScaleY" => h.sy = n.unwrap_or(1.0),
                    "LineWidth" => h.width = n.unwrap_or(0.5),
                    "Size" => {
                        h.size = match v.as_str() {
                            "Inch" => 72.0,
                            "HalfInch" => 36.0,
                            "QuarterInch" => 18.0,
                            "EighthInch" => 9.0,
                            "Centimeter" => 72.0 / 2.54,
                            "Millimeter" => 72.0 / 25.4,
                            "Foot" => 864.0,
                            _ => 72.0,
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => field = None,
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_root {
        anyhow::bail!("not a Bluebeam hatch file");
    }
    Ok(raw
        .into_iter()
        .filter(|h| !h.lines.is_empty() || !h.dots.is_empty())
        .map(|h| {
            let w = h.width.clamp(0.1, 4.0);
            Hatch {
                name: h.name,
                tile: vec2(h.size * h.sx, h.size * h.sy),
                line_width: w,
                lines: h.lines,
                dots: h.dots.into_iter().map(|(p, large)| (p, if large { 2.5 * w.max(0.5) } else { 1.2 * w.max(0.5) })).collect(),
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Library

/// What the pickers offer: built-ins first, then each library file's
/// patterns under its file name.
#[derive(Default)]
pub struct Library {
    pub line_types: Vec<(String, Arc<LineType>)>,
    pub hatches: Vec<(String, Arc<Hatch>)>,
    /// Files that couldn't be read, with why.
    pub problems: Vec<String>,
}

/// The user's own library: files imported in the app.
pub fn own_dir() -> PathBuf {
    crate::sidecar::dir().join("patterns")
}

impl Library {
    /// Built-ins, the user's own library, then the shared folder (if set).
    pub fn load(shared: Option<&Path>) -> Self {
        let mut lib = Library::default();
        lib.line_types.extend(builtin_line_types().into_iter().map(|l| (String::new(), Arc::new(l))));
        lib.hatches.extend(builtin_hatches().into_iter().map(|h| (String::new(), Arc::new(h))));
        let mut dirs = vec![own_dir()];
        dirs.extend(shared.map(Path::to_path_buf));
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            // A firm's folder may sort its files into subfolders (Hatches, Line Types).
            for sub in files.clone().iter().filter(|p| p.is_dir()) {
                if let Ok(rd) = std::fs::read_dir(sub) {
                    files.extend(rd.flatten().map(|e| e.path()));
                }
            }
            files.sort();
            for f in files {
                lib.add_file(&f);
            }
        }
        lib
    }

    fn add_file(&mut self, f: &Path) {
        let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext != "blx" && ext != "bhx" {
            return;
        }
        let set = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let read = std::fs::read(f).map_err(anyhow::Error::from).map(|b| String::from_utf8_lossy(&b).into_owned());
        let res = read.and_then(|xml| {
            if ext == "blx" {
                let lts = parse_blx(&xml)?;
                let n = lts.len();
                self.line_types.extend(lts.into_iter().map(|l| (set.clone(), Arc::new(l))));
                Ok(n)
            } else {
                let hs = parse_bhx(&xml)?;
                let n = hs.len();
                self.hatches.extend(hs.into_iter().map(|h| (set.clone(), Arc::new(h))));
                Ok(n)
            }
        });
        match res {
            Ok(0) => self.problems.push(format!("{}: nothing in it Greenline can draw", f.display())),
            Ok(_) => {}
            Err(e) => self.problems.push(format!("{}: {e}", f.display())),
        }
    }
}

/// Copies Bluebeam pattern files into the user's own library, after checking
/// each parses. Returns how many patterns they hold.
pub fn import(paths: &[PathBuf]) -> anyhow::Result<usize> {
    let dir = own_dir();
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;
    for p in paths {
        let xml = String::from_utf8_lossy(&std::fs::read(p)?).into_owned();
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let count = match ext.as_str() {
            "blx" => parse_blx(&xml)?.len(),
            "bhx" => parse_bhx(&xml)?.len(),
            _ => anyhow::bail!("{}: not a .blx or .bhx file", p.display()),
        };
        if count == 0 {
            anyhow::bail!("{}: nothing in it Greenline can draw", p.display());
        }
        let name = p.file_name().ok_or_else(|| anyhow::anyhow!("{}: no file name", p.display()))?;
        std::fs::copy(p, dir.join(name))?;
        n += count;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Geometry

/// The path drawn with line type `lt` at width `w`, as prims.
pub fn pattern_prims(out: &mut Vec<Prim>, pts: &[Pos2], closed: bool, lt: &LineType, w: f32, color: Rgb) {
    let unit = if lt.scale_with_width { w } else { 1.0 };
    let mut path = pts.to_vec();
    if closed && pts.len() > 2 {
        path.push(pts[0]);
    }
    path.dedup_by(|a, b| (*a - *b).length() < 1e-3);
    if path.len() < 2 {
        return;
    }
    for line in &lt.lines {
        let p = if line.offset.abs() > 1e-3 { offset_path(&path, line.offset * unit, closed) } else { path.clone() };
        walk(out, &p, line, unit, w, color);
    }
}

/// The path moved `d` to its right, corners mitred (capped at 4x).
fn offset_path(path: &[Pos2], d: f32, closed: bool) -> Vec<Pos2> {
    let n = path.len();
    let normal = |a: Pos2, b: Pos2| {
        let t = (b - a).normalized();
        vec2(-t.y, t.x)
    };
    (0..n)
        .map(|i| {
            let prev = if i > 0 { Some(normal(path[i - 1], path[i])) } else if closed { Some(normal(path[n - 2], path[0])) } else { None };
            let next = if i + 1 < n { Some(normal(path[i], path[i + 1])) } else if closed { Some(normal(path[0], path[1])) } else { None };
            let m = match (prev, next) {
                (Some(a), Some(b)) => {
                    let s = (a + b).normalized();
                    s / s.dot(a).max(0.25)
                }
                (Some(a), None) | (None, Some(a)) => a,
                _ => Vec2::ZERO,
            };
            path[i] + m * d
        })
        .collect()
}

/// Point and unit direction at arc length `s` along `path` (with its cumulative lengths `cum`).
fn at(path: &[Pos2], cum: &[f32], s: f32) -> (Pos2, Vec2) {
    let i = cum.partition_point(|&c| c <= s).clamp(1, path.len() - 1);
    let (a, b) = (path[i - 1], path[i]);
    let len = cum[i] - cum[i - 1];
    let t = if len > 0.0 { ((s - cum[i - 1]) / len).clamp(0.0, 1.0) } else { 0.0 };
    (a + (b - a) * t, (b - a).normalized())
}

/// The part of `path` between arc lengths `s0` and `s1`.
fn sub_path(path: &[Pos2], cum: &[f32], s0: f32, s1: f32) -> Vec<Pos2> {
    let mut v = vec![at(path, cum, s0).0];
    for (i, c) in cum.iter().enumerate() {
        if *c > s0 && *c < s1 {
            v.push(path[i]);
        }
    }
    v.push(at(path, cum, s1).0);
    v
}

fn walk(out: &mut Vec<Prim>, path: &[Pos2], line: &LinePattern, unit: f32, w: f32, color: Rgb) {
    let mut cum = vec![0.0f32];
    for s in path.windows(2) {
        cum.push(cum.last().unwrap() + (s[1] - s[0]).length());
    }
    let total = *cum.last().unwrap();
    let len = |c: &Comp| match c {
        Comp::Dash(l) | Comp::Space(l) => l * unit,
        Comp::Dot => 0.0,
        Comp::Symbol { left, right, stretch, .. } => (left + right) * stretch * unit,
    };
    let period: f32 = line.comps.iter().map(len).sum();
    if period < 0.5 * w.max(0.2) || line.comps.is_empty() {
        // Too fine to draw as a pattern: a plain line.
        out.push(Prim::Stroke { pts: path.to_vec(), closed: false, width: w, color });
        return;
    }
    // Far too many repeats (a huge outline at a tiny width) would stall drawing.
    if total / period > 20_000.0 {
        out.push(Prim::Stroke { pts: path.to_vec(), closed: false, width: w, color });
        return;
    }
    let mut s = 0.0;
    'outer: loop {
        for c in &line.comps {
            if s >= total {
                break 'outer;
            }
            let l = len(c);
            match c {
                Comp::Dash(_) if l > 0.0 => out.push(Prim::Stroke { pts: sub_path(path, &cum, s, (s + l).min(total)), closed: false, width: w, color }),
                Comp::Dot => {
                    let p = at(path, &cum, s).0;
                    out.push(Prim::Stroke { pts: vec![p, p], closed: false, width: w, color });
                }
                Comp::Symbol { left, stretch, fill, paths, .. } => {
                    let centre = s + left * stretch * unit;
                    if centre <= total {
                        let (o, t) = at(path, &cum, centre);
                        let n = vec2(-t.y, t.x);
                        for sp in paths {
                            let pts: Vec<Pos2> = sp.iter().map(|q| o + t * (q.x * stretch * unit) + n * (q.y * unit)).collect();
                            if pts.len() < 2 {
                                continue;
                            }
                            if *fill && pts.len() >= 3 {
                                out.push(Prim::Fill { pts, color, alpha: 255, outline: Some((color, w)) });
                            } else {
                                out.push(Prim::Stroke { pts, closed: false, width: w, color });
                            }
                        }
                    }
                }
                _ => {}
            }
            s += l;
        }
    }
}

/// `hatch_segments`, remembered (per thread) by outline, pattern and scale:
/// clipping a big hatch every frame would slow painting.
pub fn cached_hatch(outline: &[Pos2], h: &Hatch, scale: f32) -> Arc<HatchSegs> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<u64, Arc<HatchSegs>>> = RefCell::new(HashMap::new());
    }
    let mut bytes: Vec<u8> = outline.iter().flat_map(|p| [p.x.to_bits().to_le_bytes(), p.y.to_bits().to_le_bytes()]).flatten().collect();
    bytes.extend(scale.to_bits().to_le_bytes());
    // The pattern by content, so an edited library file with the same name isn't confused with the old one.
    bytes.extend(serde_json::to_vec(h).unwrap_or_default());
    let key = crate::sidecar::fnv(&bytes);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 500 {
            c.clear();
        }
        c.entry(key).or_insert_with(|| Arc::new(hatch_segments(outline, h, scale))).clone()
    })
}

/// A clipped hatch: batches of segments by stroke width (lines, then each dot size).
pub type HatchSegs = Vec<(f32, Arc<Vec<[Pos2; 2]>>)>;

/// Hatch `h` at `scale` clipped to the closed `outline`, batched by width.
/// The pattern is anchored at the outline's top-left, so it moves with the markup.
pub fn hatch_segments(outline: &[Pos2], h: &Hatch, scale: f32) -> HatchSegs {
    let bounds = outline.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)));
    if outline.len() < 3 || !bounds.is_positive() {
        return Vec::new();
    }
    let mut groups: Vec<(f32, Vec<[Pos2; 2]>)> = vec![(h.line_width, Vec::new())];
    let mut tile = h.tile * scale.max(0.01);
    // Keep the work bounded on a huge outline with a tiny tile.
    while (bounds.width() / tile.x).ceil() * (bounds.height() / tile.y).ceil() * (h.lines.len() + h.dots.len()).max(1) as f32 > 60_000.0 {
        tile *= 2.0;
    }
    let edges: Vec<(Pos2, Pos2)> = (0..outline.len()).map(|i| (outline[i], outline[(i + 1) % outline.len()])).collect();
    let (nx, ny) = ((bounds.width() / tile.x).ceil() as i32, (bounds.height() / tile.y).ceil() as i32);
    for iy in -1..=ny {
        for ix in -1..=nx {
            let o = bounds.min + vec2(ix as f32 * tile.x, iy as f32 * tile.y);
            let map = |p: Pos2| o + vec2(p.x * tile.x, p.y * tile.y);
            for [a, b] in &h.lines {
                clip_segment(map(*a), map(*b), outline, &edges, bounds, &mut groups[0].1);
            }
            for (p, d) in &h.dots {
                let p = map(*p);
                if bounds.contains(p) && crate::annot::inside(outline, p) {
                    let i = match groups.iter().position(|(w, _)| w == d) {
                        Some(i) => i,
                        None => {
                            groups.push((*d, Vec::new()));
                            groups.len() - 1
                        }
                    };
                    groups[i].1.push([p, p]);
                }
            }
        }
    }
    groups.into_iter().filter(|(_, s)| !s.is_empty()).map(|(w, s)| (w, Arc::new(s))).collect()
}

/// Pushes the parts of segment `a`–`b` inside the polygon.
fn clip_segment(a: Pos2, b: Pos2, poly: &[Pos2], edges: &[(Pos2, Pos2)], bounds: Rect, out: &mut Vec<[Pos2; 2]>) {
    let sb = Rect::from_two_pos(a, b);
    if !sb.intersects(bounds) {
        return;
    }
    let d = b - a;
    let mut ts = vec![0.0f32, 1.0];
    for &(p, q) in edges {
        if p.x.max(q.x) < sb.min.x || p.x.min(q.x) > sb.max.x || p.y.max(q.y) < sb.min.y || p.y.min(q.y) > sb.max.y {
            continue;
        }
        let e = q - p;
        let den = d.x * e.y - d.y * e.x;
        if den.abs() < 1e-9 {
            continue;
        }
        let w = p - a;
        let t = (w.x * e.y - w.y * e.x) / den;
        let u = (w.x * d.y - w.y * d.x) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            ts.push(t);
        }
    }
    ts.sort_by(f32::total_cmp);
    for w in ts.windows(2) {
        if w[1] - w[0] < 1e-5 {
            continue;
        }
        let mid = a + d * ((w[0] + w[1]) / 2.0);
        if crate::annot::inside(poly, mid) {
            out.push([a + d * w[0], a + d * w[1]]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLX: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>
<BluebeamRevuLineStyleSet Version=\"1\">
  <Title>789cf3c9cc4b2d060005ac01fc</Title>
  <LineStyle Name=\"Long Dash\" ScaleWithLineWidth=\"True\">
    <Line Offset=\"0\">
      <Component Type=\"Space\" Index=\"0\" Width=\"4\" />
      <Component Type=\"Dash\" Index=\"1\" Width=\"15\" />
    </Line>
  </LineStyle>
  <LineStyle Name=\"Rigid Insulation\" ScaleWithLineWidth=\"True\">
    <Line Offset=\"0\">
      <Component Type=\"Vector\" Index=\"0\" Left=\"5\" Right=\"5\" Scale=\"1.1\" Fill=\"False\">
        <Point Type=\"Start\" Index=\"0\" X=\"-8\" Y=\"-8\" />
        <Point Type=\"Line\" Index=\"1\" X=\"8\" Y=\"8\" />
        <Point Type=\"Start\" Index=\"2\" X=\"8\" Y=\"-8\" />
        <Point Type=\"Line\" Index=\"3\" X=\"-8\" Y=\"8\" />
      </Component>
    </Line>
  </LineStyle>
  <LineStyle Name=\"Bubble\" ScaleWithLineWidth=\"False\">
    <Line Offset=\"0\">
      <Component Type=\"Vector\" Index=\"0\" Left=\"4\" Right=\"4\" Scale=\"1\" Fill=\"False\">
        <Point Type=\"Start\" Index=\"0\" X=\"0\" Y=\"-4\" />
        <Point Type=\"Curve\" Index=\"1\" X=\"1.66\" Y=\"-4\" />
        <Point Type=\"Curve\" Index=\"2\" X=\"3\" Y=\"-2.66\" />
        <Point Type=\"Curve\" Index=\"3\" X=\"3\" Y=\"-1\" />
      </Component>
      <Component Type=\"Dot\" Index=\"1\" />
    </Line>
  </LineStyle>
</BluebeamRevuLineStyleSet>";

    const BHX: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>
<BluebeamRevuHatchSet Version=\"1\">
  <Title>789c</Title>
  <Hatch Name=\"BRICK &amp; BLOCK\">
    <Description>COLOR - LT GRAY</Description>
    <ScaleX>1</ScaleX>
    <ScaleY>2</ScaleY>
    <Size>HalfInch</Size>
    <LineWidth>0.5</LineWidth>
    <Element Type=\"Line\" X1=\"0\" Y1=\"0\" X2=\"1\" Y2=\"1\" />
    <Element Type=\"Dot\" X=\"0.25\" Y=\"0.75\" Size=\"Large\" />
  </Hatch>
</BluebeamRevuHatchSet>";

    #[test]
    fn reads_bluebeam_line_types() {
        let lts = parse_blx(BLX).unwrap();
        assert_eq!(lts.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Long Dash", "Rigid Insulation", "Bubble"]);
        assert!(lts[0].scale_with_width && !lts[2].scale_with_width);
        assert_eq!(lts[0].lines[0].comps, vec![Comp::Space(4.0), Comp::Dash(15.0)]);
        let Comp::Symbol { left, right, stretch, paths, .. } = &lts[1].lines[0].comps[0] else { panic!() };
        assert_eq!((*left, *right, *stretch), (5.0, 5.0, 1.1));
        assert_eq!(paths.len(), 2, "two strokes of the X");
        let Comp::Symbol { paths, .. } = &lts[2].lines[0].comps[0] else { panic!() };
        assert_eq!(paths[0].len(), 9, "a curve is sampled");
        assert!((*paths[0].last().unwrap() - pos2(3.0, -1.0)).length() < 1e-4);
        assert!(parse_blx("<Other/>").is_err());
    }

    #[test]
    fn reads_bluebeam_hatches() {
        let hs = parse_bhx(BHX).unwrap();
        assert_eq!(hs.len(), 1);
        let h = &hs[0];
        assert_eq!(h.name, "BRICK & BLOCK");
        assert_eq!(h.tile, vec2(36.0, 72.0));
        assert_eq!(h.lines, vec![[pos2(0.0, 1.0), pos2(1.0, 0.0)]], "y flipped to point down");
        assert_eq!(h.dots[0].0, pos2(0.25, 0.25));
        assert!(h.dots[0].1 > 1.0);
        assert!(parse_bhx(BLX).is_err());
    }

    #[test]
    fn dashes_follow_the_path_round_corners() {
        let lt = LineType { name: "d".into(), scale_with_width: true, lines: vec![LinePattern { offset: 0.0, comps: vec![Comp::Dash(4.0), Comp::Space(2.0)] }] };
        let mut out = Vec::new();
        // 30 long at width 1: dashes of 4 every 6 → 5 dashes; one bends round the corner at 20.
        pattern_prims(&mut out, &[pos2(0.0, 0.0), pos2(20.0, 0.0), pos2(20.0, 10.0)], false, &lt, 1.0, [0; 3]);
        assert_eq!(out.len(), 5);
        let Prim::Stroke { pts, .. } = &out[3] else { panic!() };
        assert_eq!(pts, &vec![pos2(18.0, 0.0), pos2(20.0, 0.0), pos2(20.0, 2.0)]);
        // At width 2 everything doubles.
        let mut out2 = Vec::new();
        pattern_prims(&mut out2, &[pos2(0.0, 0.0), pos2(30.0, 0.0)], false, &lt, 2.0, [0; 3]);
        assert_eq!(out2.len(), 3);
    }

    #[test]
    fn symbols_repeat_at_their_spacing() {
        let lts = parse_blx(BLX).unwrap();
        let mut out = Vec::new();
        pattern_prims(&mut out, &[pos2(0.0, 0.0), pos2(110.0, 0.0)], false, &lts[1], 1.0, [0; 3]);
        // Every 11 points (10 stretched by 1.1): 10 crosses of two strokes each.
        assert_eq!(out.len(), 20);
        let Prim::Stroke { pts, .. } = &out[0] else { panic!() };
        assert!((pts[0] - pos2(5.5 - 8.8, -8.0)).length() < 1e-3, "{pts:?}");
    }

    #[test]
    fn hatch_stays_inside_the_outline() {
        let tri = [pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(0.0, 100.0)];
        let h = &builtin_hatches()[2]; // horizontal lines every 8 points
        let segs = hatch_segments(&tri, h, 1.0)[0].1.clone();
        // Lines 8 apart over a triangle of 5000 square points: about 625 points of line.
        let len: f32 = segs.iter().map(|[a, b]| (*b - *a).length()).sum();
        assert!((len - 625.0).abs() < 60.0, "{len}");
        for [a, b] in segs.iter() {
            for p in [*a, *b] {
                assert!(p.x >= -1e-3 && p.y >= -1e-3 && p.x + p.y <= 100.0 + 1e-3, "{p:?}");
            }
        }
        let len2: f32 = hatch_segments(&tri, h, 2.0)[0].1.iter().map(|[a, b]| (*b - *a).length()).sum();
        assert!((len2 - len / 2.0).abs() < 60.0, "doubling the scale halves the line: {len2}");
        // Dots keep their own size.
        let hs = parse_bhx(BHX).unwrap();
        let groups = hatch_segments(&[pos2(0.0, 0.0), pos2(200.0, 0.0), pos2(200.0, 200.0), pos2(0.0, 200.0)], &hs[0], 1.0);
        assert_eq!(groups.len(), 2);
        assert!(groups[1].0 > groups[0].0 && groups[1].1.iter().all(|[a, b]| a == b));
    }
}
