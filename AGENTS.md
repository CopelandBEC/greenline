# Redline

Lean native PDF markup app for senior review of drawings/specs/decks (Rust, egui + PDFium).

- Targets Linux and Windows (1.0+). `scripts/fetch-pdfium.sh` downloads `libpdfium.so` / `pdfium.dll` (Git Bash) into `vendor/` (gitignored); the binary finds it next to itself, in `vendor/`, `~/.local/lib`, or `/usr/lib`. Cross-check Windows code locally with `cargo check --target x86_64-pc-windows-gnu`.
- Platform specifics live behind `cfg(windows)`: font candidates (`fonts.rs`: Liberation/Noto on Linux, Arial/Segoe on Windows; the first must be Helvetica-metric), data dir (`sidecar::dir`), default author name. Paths are canonicalised with `dunce` (no `\\?\` prefixes).
- Release: CI (`.github/workflows/build.yml`) tests on Linux + Windows and builds the Inno Setup installer (`installer/redline.iss`); a `v*` tag publishes a GitHub release. Bump `Cargo.toml` and `redline.rc` versions together. Windows builds link the CRT statically (`.cargo/config.toml`).
- Annotations are stored in page-local *view points* (displayed page, y down). `annot::Annot::prims()` is the single geometry source for both the screen painter (`app.rs`) and the flattened export (`render.rs::export`), so keep them in sync through prims, not separate drawing code.
- Export maps view→PDF user space with PDFium's own device↔page transform (`ViewToUser`), so /Rotate and CropBox are handled; covered by `export_lands_in_view_coordinates`.
- Stamps (`stamps.rs`): imported PNG/JPEG seals and signatures are stored by content hash in `<data dir>/images/` (opaque scans get white knocked out, size capped) and listed in `stamps.json`; `Kind::Image` refers to the stored name, drawn via `Prim::Image` (textures on screen, PDFium image objects downsampled to 600 dpi on export). `Annot::group` ties markups on one page together; "Copy to pages" offsets from the nearest page corner.
- Export modes (`render::ExportMode`): Flatten burns prims into page content; Editable writes one Stamp annotation per markup whose appearance is the same prims (`emit_prims` targets either). Existing PDF annotations are never flattened (PDFium's flatten resized pages and dropped comments).
- Renders come in three layers (`render::Layer`): Base (whole page, power-of-two scale), Patch (sharp visible region after the view settles), Thumb (sidebar/overview). The worker drops queued renders for pages no longer in `Keep`.
- PDFium is single-threaded here: everything touching it runs on the worker thread in `render.rs`.
- Markups autosave to `<data dir>/<stem>-<fnv hash of path>.json` (`~/.local/share/redline` or `%APPDATA%\Redline`) plus `prefs.json`; the source PDF is never modified.
- Tests: `cargo test`. Real-file export check: `REDLINE_SAMPLE=/path.pdf cargo test --release -- --ignored --nocapture`.
