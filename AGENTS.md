# Redline

Lean native PDF markup app for senior review of drawings/specs/decks (Rust, egui + PDFium).

- `scripts/fetch-pdfium.sh` downloads `libpdfium.so` into `vendor/` (gitignored); the binary finds it next to itself, in `vendor/`, `~/.local/lib`, or `/usr/lib`.
- Annotations are stored in page-local *view points* (displayed page, y down). `annot::Annot::prims()` is the single geometry source for both the screen painter (`app.rs`) and the flattened export (`render.rs::export`), so keep them in sync through prims, not separate drawing code.
- Export maps view→PDF user space with PDFium's own device↔page transform (`ViewToUser`), so /Rotate and CropBox are handled; covered by `export_lands_in_view_coordinates`.
- PDFium is single-threaded here: everything touching it runs on the worker thread in `render.rs`.
- Markups autosave to `~/.local/share/redline/<stem>-<fnv hash of path>.json`; the source PDF is never modified.
- Tests: `cargo test`. Real-file export check: `REDLINE_SAMPLE=/path.pdf cargo test --release -- --ignored --nocapture`.
