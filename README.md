# Greenline

Fast, native PDF markup for senior review of drawing sets, specs and decks. Built by [Copeland Building Envelope Consulting](https://copelandbec.com) for its own reviews, in CopelandBEC colours. Rust + egui + PDFium, for Windows and Linux. (Called Redline before 1.3; installing over it keeps your markups.)

## Install

**Windows:** run `Greenline-<version>-setup.exe` from the [latest release](../../releases/latest). Per-user install, no admin rights. The installer is unsigned for now, so SmartScreen may warn: **More info → Run anyway**. It adds Start-menu (optional desktop) shortcuts and "Open with → Greenline" for PDFs without changing your default PDF app.

**Linux:** `scripts/fetch-pdfium.sh && cargo build --release`, then run `target/release/greenline file.pdf`.

## Use

| Key | Action |
|---|---|
| `C` | Callout: press on the target, drag to where the note goes (click = quick note) |
| `Ctrl+click` with a callout selected | Add another leader: arrowhead at the click, tail at the first leader's bend; drag a tip to adjust |
| `T` `K` `R` `H` | Text box, revision cloud, rectangle, highlight |
| `A` `L` `P` | Arrow, line, pen |
| `D` `E` | Measure a length or an area: click the points, double-click or `Enter` to finish (drag for one segment or a rectangle); `Shift` squares a point to the last |
| `U` | Scale for measuring: pick the drawing scale, use one written on the sheet, calibrate on a known dimension, or add a region for a detail at its own scale. A reduced or scanned set needs calibrating |
| `V` / `Esc` | Select: move, resize, double-click or `Enter` to edit text |
| `1`–`6`, `[` `]`, `{` `}` | Colour (CopelandBEC medium green by default, then red, blue, black, orange, yellow), line width, text size |
| `S` `O` `M` `G` | Thumbnails, overview, markup list, go to page |
| `I` | Stamps: seal and signature images (PNG/JPEG) and the date; place them, then copy the set to the same spot on other sheets |
| `Ctrl+G` / `Ctrl+Shift+G` | Group / ungroup the selection (Alt+click picks one piece of a group) |
| Arrows | With a selection: nudge it (`Shift` for bigger steps); otherwise previous/next page and scroll |
| `Ctrl+E` / `Ctrl+Alt+E` | Export flattened / editable PDF next to the original; **Export → Measurements (CSV)** lists every measurement's length, area, width and height for a takeoff |
| `F1` | All shortcuts, your name for exported comments, and light/dark appearance |

Markups autosave (`%APPDATA%\Greenline` on Windows, `~/.local/share/greenline` on Linux; an old Redline folder is moved there on first launch); the source PDF is never modified. Stamp images are copied into `images/` there, so placed stamps keep working if the original file moves.

## Build and release

CI (`.github/workflows/build.yml`) tests on Linux and Windows, builds `greenline.exe`, and packages the Inno Setup installer (`installer/greenline.iss`) as a workflow artifact. Pushing a `v*` tag also publishes a GitHub release with the installer, using `docs/release-notes.md`. Bump `version` in `Cargo.toml`, `greenline.rc` and the installer default together.

After changing dependencies, run `scripts/third-party-licenses.sh` (needs `cargo install --locked --features cli cargo-about`) to regenerate `THIRD-PARTY-LICENSES.html`, which the installer ships alongside our licences, PDFium's and Montserrat's.

## Contributing

Greenline is provided as-is. Issues are turned off, and pull requests are not being accepted: the source is published so you can read it, build it and modify it for your own use, under the licence below.

## No warranty

Except as required by applicable law or agreed to in writing, Greenline is free software provided **"as is", without warranty of any kind**, express or implied, including the warranties of merchantability, fitness for a particular purpose and non-infringement. You use it at your own risk. In no event will Copeland Building Envelope Consulting, Inc. or the contributors be liable for any claim, damages or other liability, whether direct, indirect, incidental, special or consequential, arising from the software or its use, including lost or corrupted files or markups, missed or incorrect markups, and decisions made from its output. The full terms are in the [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) licences, which govern.

Greenline is a markup tool, not a professional service. Using it creates no consultant, engineering or other professional relationship with CopelandBEC, and nothing it shows or exports is professional advice. Measurements and quantities depend on the drawing's own scale and accuracy and on how the user calibrated and placed them. Check them against the dimensions on the drawings before relying on them. Keep your own copies of anything that matters.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Copyright (c) 2026 Copeland Building Envelope Consulting, Inc.

The licence covers the code. Apart from the reasonable attribution Apache-2.0 allows, it grants no rights to the Copeland, CopelandBEC or Copeland Building Envelope Consulting names, logos or branding. If you distribute a modified version, don't present it as an official CopelandBEC product.
