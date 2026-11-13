//! `greenline-cli`: read a drawing set, propose markups for review, render,
//! measure and export, for agents and scripts. Output is JSON on standard
//! output; errors go to standard error with exit code 1.

use crate::annot::{Annot, Kind, COLORS};
use crate::measure::{self, Scale, Units};
use crate::render::{self, ExportMode};
use crate::{ops, pdf, proposals, sidecar};
use egui::{pos2, Rect, Vec2};
use pdfium_render::prelude::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const GUIDE: &str = include_str!("../docs/agents.md");

const USAGE: &str = "greenline-cli: Greenline for agents and scripts (read `greenline-cli guide` first)

  guide                                   how to work with drawings through this tool
  info <pdf>                              pages, sizes, scales, markup and proposal counts
  text <pdf> --page N [--region R] [--contains S]
  paths <pdf> --page N [--region R] [--min L] [--limit N]
  snap <pdf> --page N --rect R [--tol T]  fit a rough rectangle to the drawn lines
  render <pdf> --page N [--dpi D] [--region R] [--out F.png] [--markups] [--proposals] [--ops F.json] [--grid STEP]
  list <pdf> [--page N] [--proposals]     markups (and proposals) with geometry and values
  measurements <pdf> [--proposals] [--csv F.csv]
  apply <pdf> <ops.json | -> [--agent NAME] [--summary TEXT] [--dry-run]
  proposals <pdf>                         batches waiting for review
  withdraw <pdf> <id | all>               take back a proposed batch
  export <pdf> [--out F.pdf] [--editable] [--proposals]
  ops                                     the op reference for apply

Pages count from 1. R is x0,y0,x1,y1 in page points (origin top-left, y down, 72 per inch).
--units metric shows measurements in m / m² (default feet-inches).";

struct Args {
    pos: Vec<String>,
    flags: HashMap<String, String>,
}

const SWITCHES: &[&str] = &["markups", "proposals", "dry-run", "editable", "help"];

impl Args {
    fn parse(v: Vec<String>) -> Result<Self, String> {
        let (mut pos, mut flags) = (Vec::new(), HashMap::new());
        let mut it = v.into_iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if let Some((k, val)) = name.split_once('=') {
                    flags.insert(k.to_string(), val.to_string());
                } else if SWITCHES.contains(&name) {
                    flags.insert(name.to_string(), String::new());
                } else {
                    let val = it.next().ok_or(format!("--{name} needs a value"))?;
                    flags.insert(name.to_string(), val);
                }
            } else {
                pos.push(a);
            }
        }
        Ok(Self { pos, flags })
    }

    fn has(&self, k: &str) -> bool {
        self.flags.contains_key(k)
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(String::as_str)
    }

    fn num(&self, k: &str) -> Result<Option<f32>, String> {
        self.get(k).map(|v| v.parse::<f32>().ok().filter(|v| v.is_finite()).ok_or(format!("--{k} must be a number"))).transpose()
    }

    fn rect(&self, k: &str) -> Result<Option<Rect>, String> {
        let Some(v) = self.get(k) else { return Ok(None) };
        let n: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if n.len() != 4 || n.iter().any(|v| !v.is_finite()) {
            return Err(format!("--{k} must be x0,y0,x1,y1"));
        }
        Ok(Some(Rect::from_two_pos(pos2(n[0], n[1]), pos2(n[2], n[3]))))
    }

    fn units(&self) -> Result<Units, String> {
        match self.get("units") {
            None | Some("imperial") | Some("feet") => Ok(Units::Imperial),
            Some("metric") => Ok(Units::Metric),
            Some(u) => Err(format!("--units {u}: use imperial or metric")),
        }
    }
}

pub fn main(args: Vec<String>) -> i32 {
    match run(args) {
        Ok(out) => {
            if !out.is_null() {
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            }
            0
        }
        Err(e) => {
            eprintln!("greenline-cli: {e}");
            1
        }
    }
}

/// Runs one command; its JSON result, or `Null` when it printed text itself.
pub fn run(args: Vec<String>) -> anyhow::Result<Value> {
    let a = Args::parse(args).map_err(anyhow::Error::msg)?;
    let Some(cmd) = a.pos.first().cloned() else {
        println!("{USAGE}");
        return Ok(Value::Null);
    };
    if a.has("help") || cmd == "help" {
        println!("{USAGE}");
        return Ok(Value::Null);
    }
    match cmd.as_str() {
        "guide" => {
            println!("{GUIDE}");
            Ok(Value::Null)
        }
        "ops" => {
            println!("{}", ops::REFERENCE);
            Ok(Value::Null)
        }
        "info" | "text" | "paths" | "snap" | "render" | "list" | "measurements" | "apply" | "proposals" | "withdraw" | "export" => {
            let file = a.pos.get(1).ok_or_else(|| anyhow::anyhow!("{cmd} needs a PDF path"))?;
            let doc = Doc::open(Path::new(file))?;
            command(&cmd, &a, &doc, pdf::pdfium()?)
        }
        other => anyhow::bail!("unknown command \"{other}\" (try `greenline-cli help`)"),
    }
}

/// A PDF and what Greenline keeps for it.
pub struct Doc {
    pub path: PathBuf,
    pub annots: Vec<Annot>,
    pub scales: Vec<Scale>,
}

impl Doc {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        // Canonical, as the app opens it, so both find the same markups file.
        let path = dunce::canonicalize(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let (annots, scales) = match sidecar::load(&path)? {
            Some(s) => (s.annots, s.scales),
            None => (Vec::new(), Vec::new()),
        };
        Ok(Self { path, annots, scales })
    }
}

/// Refuses an output path that is the source PDF (by its real path, so
/// `sheet.pdf` for `Sheet.pdf` on Windows or a symlinked folder still counts).
fn not_source(out: &Path, src: &Path) -> anyhow::Result<()> {
    let real_parent = out.parent().filter(|p| !p.as_os_str().is_empty()).and_then(|p| dunce::canonicalize(p).ok()).or_else(|| std::env::current_dir().ok());
    let real_out = real_parent.zip(out.file_name()).map(|(p, f)| p.join(f));
    if real_out.as_deref() == Some(src) || dunce::canonicalize(out).is_ok_and(|o| o == src) {
        anyhow::bail!("refusing to overwrite the original PDF");
    }
    Ok(())
}

/// Writes `out` through a temporary file renamed into place, so a hard link
/// to another file (the source PDF, say) is replaced, never written through.
fn write_new(out: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = out.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| anyhow::anyhow!("{}: {e}", out.display()))?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("{}: {e}", out.display())
    })
}

fn page_arg(a: &Args, pages: usize) -> anyhow::Result<usize> {
    let n: usize = a.get("page").ok_or_else(|| anyhow::anyhow!("needs --page N"))?.parse().map_err(|_| anyhow::anyhow!("--page must be a number"))?;
    if n == 0 || n > pages {
        anyhow::bail!("page {n} out of range: the document has {pages} pages");
    }
    Ok(n - 1)
}

fn page_sizes(d: &PdfDocument) -> Vec<Vec2> {
    d.pages().iter().map(|p| Vec2::new(p.width().value, p.height().value)).collect()
}

fn round2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn color_name(c: [u8; 3]) -> String {
    COLORS.iter().find(|(k, _)| *k == c).map(|(_, n)| n.to_lowercase()).unwrap_or_else(|| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
}

/// Accepted markups and scales, plus every proposal's when `with_proposals`.
/// A markup and, if it's proposed, its batch id.
type Tagged = (Annot, Option<String>);

fn markups(doc: &Doc, with_proposals: bool, units: Units) -> anyhow::Result<(Vec<Tagged>, Vec<Scale>)> {
    let mut all: Vec<Tagged> = doc.annots.iter().cloned().map(|a| (a, None)).collect();
    let mut scales = doc.scales.clone();
    if with_proposals {
        for b in proposals::load(&doc.path)?.batches {
            scales = ops::merged_scales(&scales, &b.scales);
            all.extend(b.annots.into_iter().map(|a| (a, Some(b.id.clone()))));
        }
    }
    let mut annots: Vec<Annot> = all.iter().map(|(a, _)| a.clone()).collect();
    measure::relabel(&mut annots, &scales, units);
    Ok((annots.into_iter().zip(all.into_iter().map(|(_, p)| p)).collect(), scales))
}

fn measured_json(a: &Annot, scales: &[Scale], units: Units) -> Value {
    match measure::measured(a, scales) {
        None => json!(null),
        Some(m) => json!({
            "length": measure::fmt_len(m.length, units),
            "length_in": m.length,
            "area": m.area.map(|v| measure::fmt_area(v, units)),
            "area_sq_in": m.area,
            "width": measure::fmt_len(m.width, units),
            "width_in": m.width,
            "height": measure::fmt_len(m.height, units),
            "height_in": m.height,
            "scale": m.scale,
        }),
    }
}

fn annot_json(a: &Annot, proposal: Option<&str>, scales: &[Scale], units: Units) -> Value {
    let mut v = json!({
        "id": a.id,
        "page": a.page + 1,
        "type": a.kind_name().to_lowercase(),
        "color": color_name(a.color),
        "label": a.label(),
        "bounds": pdf::rect4(a.bounds()),
        "geometry": serde_json::to_value(&a.kind).unwrap_or_default(),
    });
    if let Some(g) = a.group {
        v["group"] = json!(g);
    }
    if let Some(p) = proposal {
        v["proposal"] = json!(p);
    }
    if matches!(a.kind, Kind::Measure { .. }) {
        v["measured"] = measured_json(a, scales, units);
    }
    v
}

fn scale_json(s: &Scale) -> Value {
    json!({ "page": s.page + 1, "label": s.label, "ratio": s.ratio(), "region": s.region.map(pdf::rect4) })
}

fn read_ops(src: &str) -> anyhow::Result<Value> {
    let text = if src == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        s
    } else {
        std::fs::read_to_string(src).map_err(|e| anyhow::anyhow!("{src}: {e}"))?
    };
    serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{src}: not JSON: {e}"))
}

/// The markups a render draws: accepted, proposed, an ops file and a grid, as asked.
fn overlay(a: &Args, doc: &Doc, pages: &[Vec2], page: usize, units: Units) -> anyhow::Result<Vec<Annot>> {
    let mut out = Vec::new();
    let mut scales = doc.scales.clone();
    if a.has("markups") || a.has("proposals") {
        let (all, s) = markups(doc, a.has("proposals"), units)?;
        scales = s;
        out.extend(all.into_iter().filter(|(x, p)| x.page == page && (a.has("markups") || p.is_some())).map(|(x, _)| x));
    }
    if let Some(f) = a.get("ops") {
        let (id, group) = (1_000_000, 1_000_000);
        let b = ops::parse(&read_ops(f)?, pages, id, group).map_err(anyhow::Error::msg)?;
        scales = ops::merged_scales(&scales, &b.scales);
        out.extend(b.annots.into_iter().filter(|x| x.page == page));
    }
    // Everything drawn reads from the scales as they'd be, draft scales included.
    measure::relabel(&mut out, &scales, units);
    if let Some(step) = a.num("grid").map_err(anyhow::Error::msg)? {
        if step < 10.0 {
            anyhow::bail!("--grid step must be at least 10 points");
        }
        out.extend(ops::grid(page, pages[page], step));
    }
    Ok(out)
}

fn command(cmd: &str, a: &Args, doc: &Doc, pdfium: &Pdfium) -> anyhow::Result<Value> {
    let units = a.units().map_err(anyhow::Error::msg)?;
    let d = pdfium.load_pdf_from_file(&doc.path, None).map_err(|e| anyhow::anyhow!("{}: {e:?}", doc.path.display()))?;
    let pages = page_sizes(&d);
    match cmd {
        "info" => {
            let props = proposals::load(&doc.path)?;
            let mut list = Vec::new();
            for (i, s) in pages.iter().enumerate() {
                let found = d.pages().get(i as PdfPageIndex).ok().and_then(|p| p.text().ok().map(|t| t.all())).map(|t| measure::find_scales(&t)).unwrap_or_default();
                let scale = doc.scales.iter().find(|x| x.page == i && x.region.is_none());
                list.push(json!({
                    "page": i + 1,
                    "width": round2(s.x), "height": round2(s.y),
                    "inches": format!("{} x {}", round2(s.x / 72.0), round2(s.y / 72.0)),
                    "scale": scale.map(|x| x.label.clone()),
                    "scale_regions": doc.scales.iter().filter(|x| x.page == i && x.region.is_some()).count(),
                    "scales_written": found.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
                    "markups": doc.annots.iter().filter(|x| x.page == i).count(),
                    "proposed": props.batches.iter().flat_map(|b| &b.annots).filter(|x| x.page == i).count(),
                }));
            }
            Ok(json!({
                "pdf": doc.path,
                "pages": list,
                "markups_file": sidecar::file_for(&doc.path),
                "proposals_file": proposals::file_for(&doc.path),
                "proposal_batches": props.batches.len(),
                "note": "Sheets under 11 x 17 in are often reduced prints: a scale written on them is then wrong; calibrate on a dimension instead.",
            }))
        }
        "text" => {
            let page = page_arg(a, pages.len())?;
            let region = a.rect("region").map_err(anyhow::Error::msg)?;
            let needle = a.get("contains").map(str::to_lowercase);
            let runs: Vec<pdf::TextRun> = pdf::text_runs(&d.pages().get(page as PdfPageIndex)?)?
                .into_iter()
                .filter(|r| region.is_none_or(|g| g.intersects(Rect::from_min_max(pos2(r.rect[0] as f32, r.rect[1] as f32), pos2(r.rect[2] as f32, r.rect[3] as f32)))))
                .filter(|r| needle.as_ref().is_none_or(|n| r.text.to_lowercase().contains(n)))
                .collect();
            Ok(json!({ "page": page + 1, "count": runs.len(), "runs": runs }))
        }
        "paths" => {
            let page = page_arg(a, pages.len())?;
            let region = a.rect("region").map_err(anyhow::Error::msg)?;
            let min = a.num("min").map_err(anyhow::Error::msg)?.unwrap_or(2.0);
            let limit = a.num("limit").map_err(anyhow::Error::msg)?.unwrap_or(3000.0) as usize;
            let segs = pdf::segments(&d.pages().get(page as PdfPageIndex)?)?;
            let kept: Vec<_> = segs.iter().filter(|s| (s[1] - s[0]).length() >= min && region.is_none_or(|r| pdf::crosses(s, r))).collect();
            let lines: Vec<[f64; 4]> = kept.iter().take(limit).map(|s| [round2(s[0].x), round2(s[0].y), round2(s[1].x), round2(s[1].y)]).collect();
            Ok(json!({
                "page": page + 1,
                "count": kept.len(),
                "truncated": kept.len() > limit,
                "lines": lines,
                "note": "Each line is [x0, y0, x1, y1]. Narrow with --region; `snap` fits a rectangle to these.",
            }))
        }
        "snap" => {
            let page = page_arg(a, pages.len())?;
            let rough = a.rect("rect").map_err(anyhow::Error::msg)?.ok_or_else(|| anyhow::anyhow!("needs --rect x0,y0,x1,y1"))?;
            let tol = a.num("tol").map_err(anyhow::Error::msg)?.unwrap_or(6.0);
            let segs = pdf::segments(&d.pages().get(page as PdfPageIndex)?)?;
            let near: Vec<_> = segs.into_iter().filter(|s| pdf::crosses(s, rough.expand(tol))).collect();
            let (r, hit) = pdf::snap(rough, &near, tol);
            let mut v = json!({
                "page": page + 1,
                "rect": pdf::rect4(r),
                "snapped": { "left": hit[0], "top": hit[1], "right": hit[2], "bottom": hit[3] },
            });
            let scales = &doc.scales;
            if let Some(s) = measure::scale_at(scales, page, r.center()) {
                v["width"] = json!(measure::fmt_len(r.width() as f64 * s.per_pt, units));
                v["height"] = json!(measure::fmt_len(r.height() as f64 * s.per_pt, units));
                v["scale"] = json!(s.label);
            }
            Ok(v)
        }
        "render" => {
            let page = page_arg(a, pages.len())?;
            let dpi = a.num("dpi").map_err(anyhow::Error::msg)?.unwrap_or(100.0);
            if !(10.0..=1200.0).contains(&dpi) {
                anyhow::bail!("--dpi must be 10 to 1200");
            }
            let full = Rect::from_min_size(pos2(0.0, 0.0), pages[page]);
            let region = a.rect("region").map_err(anyhow::Error::msg)?.map(|r| r.intersect(full)).unwrap_or(full);
            if region.width() < 1.0 || region.height() < 1.0 {
                anyhow::bail!("--region is off the page");
            }
            let k = dpi / 72.0;
            let (w, h) = ((region.width() * k).round() as usize, (region.height() * k).round() as usize);
            if w.max(h) > 12000 {
                anyhow::bail!("{w} x {h} px is too big: lower --dpi or narrow --region");
            }
            if let Some(o) = a.get("out") {
                not_source(Path::new(o), &doc.path)?;
            }
            let extra = overlay(a, doc, &pages, page, units)?;
            let stem = doc.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "page".into());
            let out = match a.get("out") {
                Some(o) => PathBuf::from(o),
                None => {
                    let dir = std::env::temp_dir().join("greenline");
                    std::fs::create_dir_all(&dir)?;
                    dir.join(format!("{stem}-p{}.png", page + 1))
                }
            };
            let img = if extra.is_empty() {
                render::render_region(&d, page, k, [(region.min.x * k) as i32, (region.min.y * k) as i32], [w, h])?
            } else {
                // Markups go through the flattened export, so the picture is what an export gives.
                let tmp = std::env::temp_dir().join(format!("greenline-render-{}.pdf", std::process::id()));
                render::flatten(pdfium, &doc.path, &tmp, &extra)?;
                let r = pdfium.load_pdf_from_file(&tmp, None).map_err(|e| anyhow::anyhow!("{e:?}")).and_then(|t| render::render_region(&t, page, k, [(region.min.x * k) as i32, (region.min.y * k) as i32], [w, h]));
                let _ = std::fs::remove_file(&tmp);
                r?
            };
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
            let mut png = Vec::new();
            image::RgbaImage::from_raw(w as u32, h as u32, rgba)
                .ok_or_else(|| anyhow::anyhow!("render size mismatch"))?
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
            write_new(&out, &png)?;
            Ok(json!({
                "file": out,
                "page": page + 1,
                "dpi": dpi,
                "px_per_pt": k,
                "region": pdf::rect4(region),
                "width_px": w, "height_px": h,
                "drawn": extra.len(),
                "to_page": "x = region[0] + px / px_per_pt, y = region[1] + py / px_per_pt",
            }))
        }
        "list" => {
            let page = a.get("page").map(|_| page_arg(a, pages.len())).transpose()?;
            let (all, scales) = markups(doc, a.has("proposals"), units)?;
            let items: Vec<Value> = all.iter().filter(|(x, _)| page.is_none_or(|p| x.page == p)).map(|(x, p)| annot_json(x, p.as_deref(), &scales, units)).collect();
            Ok(json!({ "count": items.len(), "markups": items, "scales": scales.iter().map(scale_json).collect::<Vec<_>>() }))
        }
        "measurements" => {
            let (all, scales) = markups(doc, a.has("proposals"), units)?;
            let mut ms: Vec<&Annot> = all.iter().map(|(x, _)| x).filter(|x| matches!(x.kind, Kind::Measure { .. })).collect();
            ms.sort_by_key(|x| (x.page, x.bounds().min.y as i64, x.bounds().min.x as i64));
            let mut v = json!({
                "count": ms.len(),
                "measurements": ms.iter().map(|x| {
                    let name = if let Kind::Measure { name, .. } = &x.kind { name.clone() } else { String::new() };
                    json!({ "id": x.id, "page": x.page + 1, "name": name, "type": x.kind_name().to_lowercase(), "label": x.label(), "measured": measured_json(x, &scales, units) })
                }).collect::<Vec<_>>(),
            });
            if let Some(f) = a.get("csv") {
                not_source(Path::new(f), &doc.path)?;
                write_new(Path::new(f), measure::csv(&ms, &scales, units).as_bytes())?;
                v["csv"] = json!(f);
            }
            Ok(v)
        }
        "apply" => {
            let src = a.pos.get(2).ok_or_else(|| anyhow::anyhow!("apply needs an ops file (or - for standard input)"))?;
            let ops_json = read_ops(src)?;
            let agent = a.get("agent").map(str::to_string).or_else(|| std::env::var("GREENLINE_AGENT").ok()).unwrap_or_else(|| "agent".into());
            let summary = a.get("summary").unwrap_or("").to_string();
            let dry = a.has("dry-run");
            let build = |p: &proposals::Proposals| -> anyhow::Result<(ops::Batch, Vec<Scale>)> {
                let (id, group) = p.next_ids();
                let mut b = ops::parse(&ops_json, &pages, id, group).map_err(anyhow::Error::msg)?;
                let mut scales = doc.scales.clone();
                for x in &p.batches {
                    scales = ops::merged_scales(&scales, &x.scales);
                }
                ops::relabel(&mut b, &scales, units);
                let all = ops::merged_scales(&scales, &b.scales);
                Ok((b, all))
            };
            let report = |b: &ops::Batch, scales: &[Scale], id: Option<&str>| {
                json!({
                    "proposal": id,
                    "dry_run": id.is_none(),
                    "markups": b.annots.len(),
                    "scales": b.scales.iter().map(scale_json).collect::<Vec<_>>(),
                    "measurements": b.annots.iter().filter(|x| matches!(x.kind, Kind::Measure { .. })).map(|x| json!({ "page": x.page + 1, "label": x.label(), "measured": measured_json(x, scales, units) })).collect::<Vec<_>>(),
                    "note": if id.is_some() { "Proposed for review: the reviewer accepts or rejects it in Greenline (it shows up in the open window, or the next time this PDF is opened)." } else { "Checked only: nothing was proposed." },
                })
            };
            if dry {
                let (b, scales) = build(&proposals::load(&doc.path)?)?;
                return Ok(report(&b, &scales, None));
            }
            proposals::update(&doc.path, |p| {
                let (b, scales) = build(p)?;
                let id = p.take_id();
                let created = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
                let out = report(&b, &scales, Some(&id));
                p.batches.push(proposals::Proposal { id, agent: agent.clone(), summary: summary.clone(), created, annots: b.annots, scales: b.scales, accepted_groups: Default::default() });
                Ok(out)
            })
        }
        "proposals" => {
            let p = proposals::load(&doc.path)?;
            let batches: Vec<Value> = p
                .batches
                .iter()
                .map(|b| {
                    let mut pages: Vec<usize> = b.annots.iter().map(|x| x.page + 1).chain(b.scales.iter().map(|s| s.page + 1)).collect();
                    pages.sort();
                    pages.dedup();
                    json!({
                        "id": b.id, "agent": b.agent, "summary": b.summary, "created": b.created,
                        "markups": b.annots.len(), "scales": b.scales.len(), "pages": pages,
                    })
                })
                .collect();
            Ok(json!({ "count": batches.len(), "batches": batches }))
        }
        "withdraw" => {
            let which = a.pos.get(2).ok_or_else(|| anyhow::anyhow!("withdraw needs a proposal id (or all)"))?.clone();
            let n = proposals::update(&doc.path, |p| {
                let before = p.batches.len();
                p.batches.retain(|b| which != "all" && b.id != which);
                Ok(before - p.batches.len())
            })?;
            if n == 0 {
                anyhow::bail!("no proposal \"{which}\"");
            }
            Ok(json!({ "withdrawn": n }))
        }
        "export" => {
            let mode = if a.has("editable") { ExportMode::Editable } else { ExportMode::Flatten };
            let (all, _) = markups(doc, a.has("proposals"), units)?;
            let annots: Vec<Annot> = all.into_iter().map(|(x, _)| x).filter(|x| x.text_size().is_none() || !x.text().trim().is_empty()).collect();
            let stem = doc.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
            let suffix = if mode == ExportMode::Editable { "_markup_editable" } else { "_markup" };
            let out = a.get("out").map(PathBuf::from).unwrap_or_else(|| doc.path.with_file_name(format!("{stem}{suffix}.pdf")));
            not_source(&out, &doc.path)?;
            let author = a.get("author").map(str::to_string).unwrap_or_else(|| "Greenline".into());
            let images = render::load_images(&annots)?;
            let missing = render::export(pdfium, &doc.path, &out, &annots, &images, mode, &author)?;
            Ok(json!({ "file": out, "markups": annots.len(), "characters_without_font": missing }))
        }
        _ => unreachable!(),
    }
}
