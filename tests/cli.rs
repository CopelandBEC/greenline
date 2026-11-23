//! `greenline-cli` end to end, as an agent drives it: run as its own process
//! with a throwaway data folder, on a sheet made here with a window drawn on it.

use pdfium_render::prelude::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    dir: PathBuf,
    pdf: PathBuf,
}

impl Env {
    fn new(name: &str, with_sheet: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("greenline-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let pdf = dir.join("sheet.pdf");
        if with_sheet {
            make_sheet(&pdf);
        }
        Self { dir, pdf }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_greenline-cli"));
        c.args(args).env("XDG_DATA_HOME", self.dir.join("data")).env("APPDATA", self.dir.join("data")).current_dir(&self.dir);
        c
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = self.cmd(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)))
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = self.cmd(args).output().unwrap();
        assert!(!out.status.success(), "{args:?} should fail");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn pdf(&self) -> &str {
        self.pdf.to_str().unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An 11 x 8.5 in sheet: a 54 x 90 pt window outline at (100, 100) in view
/// points, its tag, a scale note and a dimension line 144 pt long.
fn make_sheet(file: &Path) {
    let pdfium = greenline::pdf::pdfium().unwrap();
    let mut doc = pdfium.create_new_pdf().unwrap();
    let h = 612.0;
    let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::new_custom(PdfPoints::new(792.0), PdfPoints::new(h))).unwrap();
    let black = PdfColor::new(0, 0, 0, 255);
    fn path<'a>(doc: &PdfDocument<'a>, h: f32, black: PdfColor, pts: &[(f32, f32)], close: bool) -> PdfPagePathObject<'a> {
        let mut o = PdfPagePathObject::new(doc, PdfPoints::new(pts[0].0), PdfPoints::new(h - pts[0].1), Some(black), Some(PdfPoints::new(1.0)), None).unwrap();
        for p in &pts[1..] {
            o.line_to(PdfPoints::new(p.0), PdfPoints::new(h - p.1)).unwrap();
        }
        if close {
            o.close_path().unwrap();
        }
        o
    }
        let window = path(&doc, h, black, &[(100.0, 100.0), (154.0, 100.0), (154.0, 190.0), (100.0, 190.0)], true);
    page.objects_mut().add_path_object(window).unwrap();
    let dim = path(&doc, h, black, &[(300.0, 400.0), (444.0, 400.0)], false);
    page.objects_mut().add_path_object(dim).unwrap();
    let font = doc.fonts_mut().helvetica();
    for (text, x, y) in [("W1", 115.0, 210.0), ("SCALE: 1/4\" = 1'-0\"", 300.0, 500.0), ("8'-0\"", 360.0, 395.0)] {
        let mut t = PdfPageTextObject::new(&doc, text, font, PdfPoints::new(10.0)).unwrap();
        t.translate(PdfPoints::new(x), PdfPoints::new(h - y)).unwrap();
        page.objects_mut().add_text_object(t).unwrap();
    }
    page.regenerate_content().unwrap();
    drop(page);
    doc.save_to_file(file).unwrap();
}

#[test]
fn an_agent_takes_off_a_window_for_review() {
    let env = Env::new("takeoff", true);
    let pdf = env.pdf();

    let info = env.run(&["info", pdf]);
    assert_eq!(info["pages"][0]["inches"], "11 x 8.5");
    assert_eq!(info["pages"][0]["scales_written"][0], "1/4\" = 1'-0\"");
    assert_eq!(info["pages"][0]["scale"], Value::Null);

    // The tag, then the outline near it, then the outline fitted from a rough box.
    let tag = &env.run(&["text", pdf, "--page", "1", "--contains", "w1"])["runs"][0];
    assert_eq!(tag["text"], "W1");
    let lines = env.run(&["paths", pdf, "--page", "1", "--region", "90,90,170,200"]);
    assert_eq!(lines["count"], 4, "{lines}");
    let snap = env.run(&["snap", pdf, "--page", "1", "--rect", "103,96,151,194"]);
    assert_eq!(snap["rect"], serde_json::json!([100.0, 100.0, 154.0, 190.0]));

    // Calibrate on the 8'-0" dimension (144 pt: 1/4" = 1'-0"), take off the window.
    let ops = env.dir.join("ops.json");
    std::fs::write(&ops, r#"[
        {"op": "scale", "page": 1, "calibrate": {"from": [300, 400], "to": [444, 400], "length": "8'-0\""}},
        {"op": "area", "page": 1, "rect": [100, 100, 154, 190], "name": "W1", "group": "w1"},
        {"op": "callout", "page": 1, "target": [154, 120], "at": [200, 60], "text": "W1: confirm sill pan", "group": "w1"}
    ]"#).unwrap();
    let dry = env.run(&["apply", pdf, ops.to_str().unwrap(), "--dry-run"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["measurements"][0]["label"], "W1: 15.00 sf", "3'-0\" x 5'-0\": {dry}");
    assert_eq!(env.run(&["proposals", pdf])["count"], 0, "a dry run proposes nothing");

    let applied = env.run(&["apply", pdf, ops.to_str().unwrap(), "--agent", "test", "--summary", "Window W1"]);
    assert_eq!(applied["proposal"], "1");
    let props = env.run(&["proposals", pdf]);
    assert_eq!(props["batches"][0]["agent"], "test");
    assert_eq!(props["batches"][0]["markups"], 2);
    assert_eq!(env.run(&["list", pdf])["count"], 0, "proposals aren't markups until accepted");
    let listed = env.run(&["list", pdf, "--proposals"]);
    assert_eq!(listed["count"], 2);
    assert_eq!(listed["markups"][0]["proposal"], "1");
    assert_eq!(listed["markups"][0]["measured"]["width"], "3'-0\"");

    let csv = env.dir.join("takeoff.csv");
    let m = env.run(&["measurements", pdf, "--proposals", "--csv", csv.to_str().unwrap()]);
    assert_eq!(m["count"], 1);
    let csv = std::fs::read_to_string(&csv).unwrap();
    assert!(csv.lines().nth(1).unwrap().contains("15.0000"), "{csv}");

    // The picture shows the proposal (and a grid), the same as an export would.
    let png = env.dir.join("p1.png");
    let r = env.run(&["render", pdf, "--page", "1", "--dpi", "72", "--proposals", "--grid", "72", "--out", png.to_str().unwrap()]);
    assert_eq!((r["width_px"].as_u64(), r["height_px"].as_u64()), (Some(792), Some(612)));
    let img = image::open(&png).unwrap().to_rgba8();
    let inside = img.get_pixel(127, 160);
    assert!(inside[2] < 245, "the area's tint inside the window: {inside:?}");

    // Bad ops are refused whole, naming the op.
    std::fs::write(&ops, r#"[{"op": "cloud", "page": 1, "rect": [0, 0, 50, 50]}, {"op": "cloud", "page": 2, "rect": [0, 0, 50, 50]}]"#).unwrap();
    assert!(env.fails(&["apply", pdf, ops.to_str().unwrap()]).contains("op 2 (cloud): page 2 out of range"));
    assert_eq!(env.run(&["proposals", pdf])["count"], 1);

    let out = env.dir.join("out.pdf");
    let e = env.run(&["export", pdf, "--proposals", "--out", out.to_str().unwrap()]);
    assert_eq!(e["markups"], 2);
    assert!(out.exists());
    assert!(env.fails(&["export", pdf, "--out", pdf]).contains("refusing to overwrite"));
    assert!(env.fails(&["measurements", pdf, "--csv", pdf]).contains("refusing to overwrite"));
    assert!(env.fails(&["render", pdf, "--page", "1", "--out", pdf]).contains("refusing to overwrite"));
    assert!(env.fails(&["export", pdf, "--out", "sheet.pdf"]).contains("refusing to overwrite"), "relative to the working folder");
    // A smaller copy of a vector sheet: nothing to recompress, but still a copy, beside it.
    let s = env.run(&["shrink", pdf]);
    assert_eq!(s["images_recompressed"], 0);
    assert!(env.dir.join("sheet_smaller.pdf").exists() && s["check_pages"].as_array().unwrap().is_empty());
    assert!(env.fails(&["shrink", pdf, "--out", pdf]).contains("refusing to overwrite"));
    assert!(env.fails(&["shrink", pdf, "--dpi", "5"]).contains("50 to 1200"));
    // A hard link to the source is replaced, not written through.
    let before = std::fs::read(&env.pdf).unwrap();
    let link = env.dir.join("linked.csv");
    if std::fs::hard_link(&env.pdf, &link).is_ok() {
        env.run(&["measurements", pdf, "--csv", link.to_str().unwrap()]);
        assert_eq!(std::fs::read(&env.pdf).unwrap(), before, "the source PDF is untouched");
    }
    // A draft scale relabels what's already there in a preview.
    std::fs::write(&ops, r#"[{"op": "scale", "page": 1, "ratio": 96}]"#).unwrap();
    env.run(&["render", pdf, "--page", "1", "--dpi", "30", "--proposals", "--ops", ops.to_str().unwrap(), "--out", png.to_str().unwrap()]);

    assert_eq!(env.run(&["withdraw", pdf, "1"])["withdrawn"], 1);
    assert_eq!(env.run(&["proposals", pdf])["count"], 0);
    assert!(env.fails(&["withdraw", pdf, "1"]).contains("no proposal"));
}

#[test]
fn pages_count_in_the_order_the_reviewer_arranged() {
    use greenline::pages::{insert_pages, Layout, Src};
    let env = Env::new("arranged", true);
    let pdf = env.pdf();
    // As the app saves it: a blank half-letter page put before the sheet.
    let real = dunce::canonicalize(&env.pdf).unwrap();
    let stem = format!("sheet-{:016x}.json", greenline::sidecar::fnv(real.as_os_str().as_encoded_bytes()));
    let layout = insert_pages(&Layout::identity(1), 0, vec![Src::Blank { width: 396.0, height: 612.0 }]);
    let sidecar = greenline::sidecar::Sidecar { pdf: real, annots: Vec::new(), scales: Vec::new(), accepted: Vec::new(), pages: Some(layout) };
    let data = env.dir.join("data/greenline");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join(stem), serde_json::to_vec(&sidecar).unwrap()).unwrap();

    let info = env.run(&["info", pdf]);
    assert_eq!(info["pages_arranged"], true);
    assert_eq!(info["pages"][0]["inches"], "5.5 x 8.5");
    assert_eq!(info["pages"][0]["from"], "blank page");
    assert_eq!(info["pages"][1]["from"], "page 1 of the PDF");
    assert_eq!(env.run(&["text", pdf, "--page", "2", "--contains", "w1"])["count"], 1, "the sheet is page 2 now");

    let ops = env.dir.join("ops.json");
    std::fs::write(&ops, r#"[{"op": "cloud", "page": 2, "rect": [90, 90, 164, 200]}]"#).unwrap();
    env.run(&["apply", pdf, ops.to_str().unwrap()]);
    let props: Value = serde_json::from_slice(&std::fs::read(env.dir.join("data/greenline").read_dir().unwrap().flatten().map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(".proposals.json")).unwrap()).unwrap()).unwrap();
    assert_ne!(props["batches"][0]["layout"], Value::Null, "marked with the order its pages count in");
    assert_eq!(env.run(&["list", pdf, "--proposals"])["count"], 1);
    // One made for another order (the reviewer moved pages meanwhile) is left out, and listed as stale.
    let pfile = env.dir.join("data/greenline").read_dir().unwrap().flatten().map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(".proposals.json")).unwrap();
    let mut stale = props.clone();
    stale["batches"][0]["layout"] = serde_json::json!(12345);
    std::fs::write(&pfile, serde_json::to_vec(&stale).unwrap()).unwrap();
    assert_eq!(env.run(&["list", pdf, "--proposals"])["count"], 0);
    assert_eq!(env.run(&["proposals", pdf])["batches"][0]["stale"], true);
    std::fs::write(&pfile, serde_json::to_vec(&props).unwrap()).unwrap();

    let out = env.dir.join("out.pdf");
    env.run(&["export", pdf, "--proposals", "--out", out.to_str().unwrap()]);
    let pdfium = greenline::pdf::pdfium().unwrap();
    let doc = pdfium.load_pdf_from_file(&out, None).unwrap();
    let sizes: Vec<f32> = doc.pages().iter().map(|p| p.width().value).collect();
    assert_eq!(sizes, [396.0, 792.0], "exported in the arranged order");
    drop(doc);
    // A smaller copy too (nothing to recompress here: the arranged pages as they are).
    let small = env.dir.join("small.pdf");
    env.run(&["shrink", pdf, "--out", small.to_str().unwrap()]);
    let doc = pdfium.load_pdf_from_file(&small, None).unwrap();
    let sizes: Vec<f32> = doc.pages().iter().map(|p| p.width().value).collect();
    assert_eq!(sizes, [396.0, 792.0], "a smaller copy keeps the arranged order");
}

#[test]
fn guide_and_help_print_text() {
    let env = Env::new("help", false);
    for args in [&["guide"][..], &["help"], &["ops"], &[]] {
        let out = env.cmd(args).output().unwrap();
        assert!(out.status.success());
        assert!(!out.stdout.is_empty());
    }
    assert!(env.fails(&["frobnicate"]).contains("unknown command"));
    assert!(env.fails(&["info", "/no/such.pdf"]).contains("no/such.pdf"));
}
