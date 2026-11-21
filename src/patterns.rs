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

impl LineType {
    /// How far the pattern can reach to either side of the path at width `w`
    /// (offset lines and symbols), beyond half the stroke.
    pub fn reach(&self, w: f32) -> f32 {
        let unit = if self.scale_with_width { w } else { 1.0 };
        let mut r = 0.0f32;
        for l in &self.lines {
            // A symbol can turn any way at a corner: its farthest point from its origin.
            let mut side = 0.0f32;
            for c in &l.comps {
                if let Comp::Symbol { stretch, paths, .. } = c {
                    for p in paths.iter().flatten() {
                        side = side.max(vec2(p.x * stretch, p.y).length());
                    }
                }
            }
            // Offset corners are mitred out to 4x the offset (`offset_path`).
            r = r.max(4.0 * l.offset.abs() + side);
        }
        r * unit
    }
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
    /// Just the built-ins (while the rest is read).
    pub fn builtins() -> Self {
        let mut lib = Library::default();
        lib.line_types.extend(builtin_line_types().into_iter().map(|l| (String::new(), Arc::new(l))));
        lib.hatches.extend(builtin_hatches().into_iter().map(|h| (String::new(), Arc::new(h))));
        lib
    }

    /// Built-ins, the user's own library, then the shared folder (if set).
    pub fn load(shared: Option<&Path>) -> Self {
        let mut lib = Self::builtins();
        let mut dirs = vec![(own_dir(), "")];
        dirs.extend(shared.map(|p| (p.to_path_buf(), " (shared)")));
        for (dir, tag) in dirs {
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
                lib.add_file(&f, tag);
            }
        }
        lib
    }

    /// Adds a library file's patterns under its file name; `tag` tells a shared
    /// folder's file from one of the same name in the user's own library.
    fn add_file(&mut self, f: &Path, tag: &str) {
        let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext != "blx" && ext != "bhx" {
            return;
        }
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        // Case-insensitively, as agents name them.
        let taken = |s: &str| self.line_types.iter().any(|(x, _)| x.eq_ignore_ascii_case(s)) || self.hatches.iter().any(|(x, _)| x.eq_ignore_ascii_case(s));
        let mut set = stem.clone();
        if !tag.is_empty() && taken(&set) {
            set = format!("{stem}{tag}");
        }
        // A file in a subfolder can share a name with another: number it.
        let mut n = 2;
        while taken(&set) {
            set = format!("{stem}{tag} {n}");
            n += 1;
        }
        let read = std::fs::read(f).map_err(anyhow::Error::from).map(|b| String::from_utf8_lossy(&b).into_owned());
        let res = read.and_then(|xml| {
            // The same pattern already offered (a file imported and also in the shared folder): once is enough.
            // Names repeat within a file sometimes (Bluebeam doesn't mind): number the repeats.
            let unique = |name: &str, used: &mut Vec<String>| {
                let base = name.trim().to_string();
                let mut name = base.clone();
                let mut k = 2;
                while used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
                    name = format!("{base} {k}");
                    k += 1;
                }
                used.push(name.clone());
                name
            };
            let mut used = Vec::new();
            if ext == "blx" {
                let lts = parse_blx(&xml)?;
                let n = lts.len();
                for mut l in lts {
                    if !self.line_types.iter().any(|(_, x)| x.lines == l.lines && x.scale_with_width == l.scale_with_width && x.name.trim() == l.name.trim()) {
                        l.name = unique(&l.name, &mut used);
                        self.line_types.push((set.clone(), Arc::new(l)));
                    }
                }
                Ok(n)
            } else {
                let hs = parse_bhx(&xml)?;
                let n = hs.len();
                for mut h in hs {
                    if !self.hatches.iter().any(|(_, x)| x.name.trim() == h.name.trim() && x.tile == h.tile && x.line_width == h.line_width && x.lines == h.lines && x.dots == h.dots) {
                        h.name = unique(&h.name, &mut used);
                        self.hatches.push((set.clone(), Arc::new(h)));
                    }
                }
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
        // A different file of the same name gets a numbered copy rather than replacing it.
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).ok_or_else(|| anyhow::anyhow!("{}: no file name", p.display()))?;
        let bytes = std::fs::read(p)?;
        let mut dest = dir.join(format!("{stem}.{ext}"));
        let mut k = 2;
        while dest.exists() && std::fs::read(&dest).ok().as_deref() != Some(&bytes[..]) {
            dest = dir.join(format!("{stem} {k}.{ext}"));
            k += 1;
        }
        std::fs::write(&dest, &bytes)?;
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
    // The corners strictly between s0 and s1 (cum is sorted).
    let (i0, i1) = (cum.partition_point(|&c| c <= s0), cum.partition_point(|&c| c < s1));
    v.extend_from_slice(&path[i0..i1.max(i0)]);
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

/// `hatch_segments` for `outline`, remembered (per thread) by its shape relative
/// to its top-left, the pattern and the scale: clipping a big hatch every frame
/// would slow painting, and moving a hatched markup reuses its hatch. Returns
/// the segments relative to the top-left, and that top-left.
pub fn cached_hatch(outline: &[Pos2], h: &Hatch, scale: f32) -> (Arc<HatchSegs>, Vec2) {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<(HashMap<u64, Cached>, u64)> = RefCell::new((HashMap::new(), 0));
    }
    let origin = outline.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p))).min.to_vec2();
    let rel: Vec<Pos2> = outline.iter().map(|p| *p - origin).collect();
    let mut bytes: Vec<u8> = rel.iter().flat_map(|p| [p.x.to_bits().to_le_bytes(), p.y.to_bits().to_le_bytes()]).flatten().collect();
    bytes.extend(scale.to_bits().to_le_bytes());
    // The pattern by content, so an edited library file with the same name isn't confused with the old one.
    bytes.extend(serde_json::to_vec(h).unwrap_or_default());
    let key = crate::sidecar::fnv(&bytes);
    let segs = CACHE.with(|c| {
        let (map, clock) = &mut *c.borrow_mut();
        *clock += 1;
        if let Some(e) = map.get_mut(&key) {
            e.2 = *clock;
            return e.0.clone();
        }
        let segs = Arc::new(hatch_segments(&rel, h, scale));
        let size: usize = segs.iter().map(|(_, s)| s.len()).sum();
        // Keep about 4 million segments: drop the least recently used beyond that.
        let mut total: usize = map.values().map(|e| e.1).sum::<usize>() + size;
        while total > 4_000_000 || map.len() >= 2000 {
            let Some((&old, e)) = map.iter().min_by_key(|(_, e)| e.2) else { break };
            total -= e.1;
            map.remove(&old);
        }
        map.insert(key, (segs.clone(), size, *clock));
        segs
    });
    (segs, origin)
}

/// A cached hatch: its segments, how many there are, and when it was last used.
type Cached = (Arc<HatchSegs>, usize, u64);

/// The most hatch elements one markup draws: past this the pattern is too
/// dense for its size and is left out (the picker says so) rather than coarsened.
pub const MAX_HATCH: f32 = 1_500_000.0;

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
    let tile = h.tile * scale.max(0.01);
    if too_dense(bounds, h, scale) {
        return Vec::new();
    }
    let edges = Edges::new(outline, bounds, tile.y);
    let (nx, ny) = ((bounds.width() / tile.x).ceil() as i32, (bounds.height() / tile.y).ceil() as i32);
    for iy in -1..=ny {
        for ix in -1..=nx {
            let o = bounds.min + vec2(ix as f32 * tile.x, iy as f32 * tile.y);
            let map = |p: Pos2| o + vec2(p.x * tile.x, p.y * tile.y);
            for [a, b] in &h.lines {
                clip_segment(map(*a), map(*b), &edges, bounds, &mut groups[0].1);
            }
            for (p, d) in &h.dots {
                // Dots grow and shrink with the pattern; line weights stay put.
                let d = &(d * scale);
                let p = map(*p);
                if bounds.contains(p) && edges.inside(p) {
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

/// Whether hatching `bounds` with `h` at `scale` would take more than `MAX_HATCH` elements.
pub fn too_dense(bounds: Rect, h: &Hatch, scale: f32) -> bool {
    let tile = h.tile * scale.max(0.01);
    (bounds.width() / tile.x + 2.0) * (bounds.height() / tile.y + 2.0) * (h.lines.len() + h.dots.len()).max(1) as f32 > MAX_HATCH
}

/// A polygon's edges filed by horizontal band, so clipping and inside tests
/// look only at the edges near a point (a cloud has thousands).
struct Edges {
    top: f32,
    band: f32,
    rows: Vec<Vec<(Pos2, Pos2)>>,
}

impl Edges {
    fn new(poly: &[Pos2], bounds: Rect, tile_h: f32) -> Self {
        let n = ((bounds.height() / tile_h.max(1.0)).ceil() as usize).clamp(1, 4096);
        let band = bounds.height().max(1e-3) / n as f32;
        let mut e = Edges { top: bounds.min.y, band, rows: vec![Vec::new(); n] };
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let (r0, r1) = (e.row(p.y.min(q.y)), e.row(p.y.max(q.y)));
            for row in &mut e.rows[r0..=r1] {
                row.push((p, q));
            }
        }
        e
    }

    fn row(&self, y: f32) -> usize {
        (((y - self.top) / self.band).floor().max(0.0) as usize).min(self.rows.len().max(1) - 1)
    }

    /// Edges that may meet the band of heights `y0..=y1` (a few may repeat).
    fn near(&self, y0: f32, y1: f32) -> impl Iterator<Item = &(Pos2, Pos2)> {
        self.rows[self.row(y0)..=self.row(y1)].iter().flatten()
    }

    /// Even-odd test, along a ray through `p`'s band only.
    fn inside(&self, p: Pos2) -> bool {
        let mut odd = false;
        for &(a, b) in &self.rows[self.row(p.y)] {
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                odd = !odd;
            }
        }
        odd
    }
}

/// Pushes the parts of segment `a`–`b` inside the polygon.
fn clip_segment(a: Pos2, b: Pos2, edges: &Edges, bounds: Rect, out: &mut Vec<[Pos2; 2]>) {
    let sb = Rect::from_two_pos(a, b);
    if !sb.intersects(bounds) {
        return;
    }
    let d = b - a;
    let mut ts = vec![0.0f32, 1.0];
    for &(p, q) in edges.near(sb.min.y, sb.max.y) {
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
        if edges.inside(mid) {
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
    fn moved_outlines_reuse_their_hatch_and_dense_ones_are_left_out() {
        let sq = |x: f32| [pos2(x, 0.0), pos2(x + 100.0, 0.0), pos2(x + 100.0, 100.0), pos2(x, 100.0)];
        let h = &builtin_hatches()[0];
        let (a, oa) = cached_hatch(&sq(0.0), h, 1.0);
        let (b, ob) = cached_hatch(&sq(50.0), h, 1.0);
        assert!(Arc::ptr_eq(&a, &b), "same shape elsewhere: same segments");
        assert_eq!(ob - oa, vec2(50.0, 0.0));
        // Exact spacing or nothing: a sheet-sized outline at a tiny scale isn't coarsened.
        let huge = [pos2(0.0, 0.0), pos2(3000.0, 0.0), pos2(3000.0, 3000.0), pos2(0.0, 3000.0)];
        assert!(too_dense(Rect::from_min_max(huge[0], huge[2]), h, 0.1));
        assert!(hatch_segments(&huge, h, 0.1).is_empty());
        assert!(!hatch_segments(&huge, h, 1.0).is_empty());
    }

    #[test]
    fn long_dashed_paths_stay_quick_and_symbols_widen_the_reach() {
        let lt = &builtin_line_types()[0];
        let pts: Vec<Pos2> = (0..20_000).map(|i| pos2(i as f32 * 0.5, (i % 2) as f32)).collect();
        let t = std::time::Instant::now();
        let mut out = Vec::new();
        pattern_prims(&mut out, &pts, false, lt, 1.0, [0; 3]);
        assert!(out.len() > 1000);
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        let lts = parse_blx(BLX).unwrap();
        assert_eq!(lt.reach(2.0), 0.0, "plain dashes stay on the line");
        // Rigid insulation's cross reaches 8.8 along and 8 across: about 11.9 turned any way.
        assert!((lts[1].reach(1.0) - 11.89).abs() < 0.01, "rigid insulation: {}", lts[1].reach(1.0));
        // An offset line's mitred corners stay inside the reach.
        let off = LineType { name: "o".into(), scale_with_width: false, lines: vec![LinePattern { offset: -10.0, comps: vec![Comp::Dash(1000.0)] }] };
        let mut out = Vec::new();
        pattern_prims(&mut out, &[pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(0.0, 10.0)], false, &off, 1.0, [0; 3]);
        let far = out.iter().flat_map(|p| match p { Prim::Stroke { pts, .. } => pts.clone(), _ => vec![] }).map(|p| p.x - 100.0).fold(0.0, f32::max);
        assert!(far <= off.reach(1.0), "{far} > {}", off.reach(1.0));
    }

    #[test]
    fn a_big_hatched_cloud_clips_quickly() {
        let pts = crate::annot::cloud_pts(Rect::from_min_max(pos2(0.0, 0.0), pos2(3000.0, 3000.0)));
        assert!(pts.len() > 4000);
        let t = std::time::Instant::now();
        let segs = hatch_segments(&pts, &builtin_hatches()[0], 1.0);
        assert!(!segs.is_empty());
        assert!(t.elapsed().as_millis() < 1500, "{:?}", t.elapsed());
    }

    #[test]
    fn library_files_of_the_same_name_stay_apart_and_repeats_show_once() {
        let mut lib = Library::default();
        let dir = std::env::temp_dir().join(format!("greenline-pat-{}", std::process::id()));
        let (own, shared) = (dir.join("own"), dir.join("shared"));
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(own.join("Std.blx"), BLX).unwrap();
        std::fs::write(shared.join("Std.blx"), BLX).unwrap();
        std::fs::write(shared.join("Std.bhx"), BHX).unwrap();
        lib.add_file(&own.join("Std.blx"), "");
        lib.add_file(&shared.join("Std.blx"), " (shared)");
        lib.add_file(&shared.join("Std.bhx"), " (shared)");
        assert_eq!(lib.line_types.len(), 3, "identical line types once");
        assert_eq!(lib.hatches[0].0, "Std (shared)", "a different file of the same name is told apart");
        std::fs::remove_dir_all(&dir).ok();
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
