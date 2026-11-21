# Greenline

Fast, native PDF markup for senior review of drawing sets, specs and decks. Built by [Copeland Building Envelope Consulting](https://copelandbec.com) for its own reviews, in CopelandBEC colours. Rust + egui + PDFium, for Windows and Linux. (Called Redline before 1.3; installing over it keeps your markups.)

## Install

**Windows:** run `Greenline-<version>-setup.exe` from the [latest release](../../releases/latest). Per-user install, no admin rights. The installer is unsigned for now, so SmartScreen may warn: **More info → Run anyway**. It adds Start-menu (optional desktop) shortcuts and "Open with → Greenline" for PDFs without changing your default PDF app.

**Linux:** `scripts/fetch-pdfium.sh && cargo build --release`, then run `target/release/greenline file.pdf` (and `target/release/greenline-cli` for the command line).

## Use

| Key | Action |
|---|---|
| `C` | Callout: press on the target, drag to where the note goes (click = quick note) |
| `Ctrl+click` with a callout selected | Add another leader: arrowhead at the click, tail at the first leader's bend; drag a tip to adjust |
| `T` `K` `R` `H` | Text box, revision cloud, rectangle, highlight |
| `A` `L` `P` | Arrow, line, pen |
| `N` / `Shift+N` | Polyline / polygon: click the corners, double-click or `Enter` to finish (click the first point again to close it as a polygon) |
| `D` `E` | Measure a length or an area: click the points, double-click or `Enter` to finish (drag for one segment or a rectangle); `Shift` squares a point to the last |
| `U` | Scale for measuring: pick the drawing scale, use one written on the sheet, calibrate on a known dimension, or add a region for a detail at its own scale. A reduced or scanned set needs calibrating |
| `V` / `Esc` | Select: move, resize, double-click or `Enter` to edit text |
| `Ctrl+B` / `Ctrl+I` / `Ctrl+U` | Bold / italic / underline: while typing, the words selected (or what you type next); otherwise the whole of the selected notes (also the **B I U** buttons) |
| `1`–`6`, `[` `]`, `{` `}` | Colour (CopelandBEC medium green by default, then red, blue, black, orange, yellow), line width, text size; the swatch beside the colours sets a fill, its opacity and a hatch; the sample beside the width picks a line type (dashed, dotted, or from the pattern library) |
| Pattern library | Bluebeam line type (`.blx`) and hatch (`.bhx`) files: import your own from the line type and hatch lists, or point `F1` at a shared folder (your firm's standards); markups keep their pattern, so they draw the same elsewhere |
| `S` `O` `M` `G` | Thumbnails, overview, markup list, go to page |
| `W` / `F` / `Z` | Fit width / fit page / zoom to the selected markups — or click the zoom % in the status bar for fit, actual size and presets |
| `Ctrl+F` | Find text in the document: matches are highlighted on the sheets; `Enter` / `F3` next, `Shift` for previous, `Esc` closes |
| Tabs | Opening another PDF opens it in a new tab (the strip appears once two are open); `Ctrl+Tab` / `Ctrl+Shift+Tab` switch, `Ctrl+W` or middle-click closes, drag a tab to reorder; each keeps its own markups, view and undo |
| `\` | Split screen: two places side by side, each with its own zoom. Click a pane (or scroll in it) to work there; pick a tab while a pane is active to show another document in it, and click the other document's tab (marked ⬌) or pane to switch back |
| `Ctrl+N` | New blank document: Letter to ARCH E or A4–A1, portrait or landscape, up to 200 pages, saved where you choose |
| Drop an image | A PNG or JPEG dropped on a sheet is placed there as a markup, full colour (tool menu ▸ **Insert image…** too); resize it by its corners |
| `I` | Stamps: seal and signature images (PNG/JPEG) and the date; place them, then copy the set to the same spot on other sheets |
| `Ctrl+G` / `Ctrl+Shift+G` | Group / ungroup the selection (Alt+click picks one piece of a group) |
| Arrows | With a selection: nudge it (`Shift` for bigger steps); otherwise previous/next page and scroll |
| `Ctrl+E` / `Ctrl+Alt+E` | Export flattened / editable PDF next to the original (placed photos go in as compact JPEGs); **Export → Measurements (CSV)** lists every measurement's length, area, width and height for a takeoff; **Export → Smaller copy** writes `<name>_smaller.pdf` with big scans and photos brought down to 200 dpi and recompressed, and names any page worth a second look |
| `F1` | All shortcuts, your name for exported comments, and light/dark appearance |

Markups autosave (`%APPDATA%\Greenline` on Windows, `~/.local/share/greenline` on Linux; an old Redline folder is moved there on first launch); the source PDF is never modified. Stamp images are copied into `images/` there, so placed stamps keep working if the original file moves.

## Agents and scripts

`greenline-cli` (installed next to the app; the installer can put it on your PATH) lets an AI agent or a script work on a drawing set. It can:

- read sheets: text with positions, vector lines, and renders with a coordinate grid;
- fit outlines to the drawn lines;
- set scales, and measure and mark up in batches.

Everything an agent adds is a **proposal**. It appears in Greenline for you to accept or reject, and never goes straight into your markups. Have the agent run `greenline-cli guide` first; [`docs/agents.md`](docs/agents.md) is the same guide.

Example jobs: "review sheets A3.0–A5.1 and cloud any air or water barrier discontinuities", or "outline every window on the elevations, measure them and give me a takeoff spreadsheet".

An agent with a shell (Claude Code, Codex) uses the command line directly and does best when it writes a script. A chat client without a shell can use the same commands as MCP tools: add a server whose command is `greenline-cli mcp`, for example `claude mcp add greenline -- greenline-cli mcp`.

## Build and release

CI (`.github/workflows/build.yml`) tests on Linux and Windows, builds `greenline.exe`, and packages the Inno Setup installer (`installer/greenline.iss`) as a workflow artifact. Pushing a `v*` tag also publishes a GitHub release with the installer, using `docs/release-notes.md`. Bump `version` in `Cargo.toml`, `greenline.rc` and the installer default together. A tag with a pre-release part (`v2.0.0-rc.1`) publishes as a GitHub pre-release; Windows file versions drop that part (`2.0.0`).

After changing dependencies, run `scripts/third-party-licenses.sh` (needs `cargo install --locked --features cli cargo-about`) to regenerate `THIRD-PARTY-LICENSES.html`, which the installer ships alongside our licences, PDFium's and Montserrat's.

## Roadmap

**2.0** is in release candidates: polylines and polygons, fills, line types and hatches (Bluebeam `.blx`/`.bhx`), text search, bold/italic/underline (per word, too), blank documents, images as markups, tabs, split screen, smaller copies of scanned sets, and a round of speed work.

## Contributing

Greenline is provided as-is. Issues are turned off, and pull requests are not being accepted: the source is published so you can read it, build it and modify it for your own use, under the licence below.

## No warranty

Except as required by applicable law or agreed to in writing, Greenline is free software provided **"as is", without warranty of any kind**, express or implied, including the warranties of merchantability, fitness for a particular purpose and non-infringement. You use it at your own risk. In no event will Copeland Building Envelope Consulting, Inc. or the contributors be liable for any claim, damages or other liability, whether direct, indirect, incidental, special or consequential, arising from the software or its use, including lost or corrupted files or markups, missed or incorrect markups, and decisions made from its output. The full terms are in the [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) licences, which govern.

Greenline is a markup tool, not a professional service. Using it creates no consultant, engineering or other professional relationship with CopelandBEC, and nothing it shows or exports is professional advice. Measurements and quantities depend on the drawing's own scale and accuracy and on how the user calibrated and placed them. Check them against the dimensions on the drawings before relying on them. Keep your own copies of anything that matters.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Copyright (c) 2026 Copeland Building Envelope Consulting, Inc.

The licence covers the code. Apart from the reasonable attribution Apache-2.0 allows, it grants no rights to the Copeland, CopelandBEC or Copeland Building Envelope Consulting names, logos or branding. If you distribute a modified version, don't present it as an official CopelandBEC product.
