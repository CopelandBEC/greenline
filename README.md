# Redline

Fast, native PDF markup for senior review of drawing sets, specs and decks. Rust + egui + PDFium.

## Install

**Windows:** run `Redline-<version>-setup.exe` from the [latest release](../../releases/latest). Per-user install, no admin rights. The installer is unsigned for now, so SmartScreen may warn: **More info → Run anyway**. It adds Start-menu (optional desktop) shortcuts and "Open with → Redline" for PDFs without changing your default PDF app.

**Linux:** `scripts/fetch-pdfium.sh && cargo build --release`, then run `target/release/redline file.pdf`.

## Use

| Key | Action |
|---|---|
| `C` | Callout: press on the target, drag to where the note goes (click = quick note) |
| `T` `K` `R` `H` | Text box, revision cloud, rectangle, highlight |
| `A` `L` `P` | Arrow, line, pen |
| `V` / `Esc` | Select: move, resize, double-click or `Enter` to edit text |
| `1`–`6`, `[` `]`, `{` `}` | Colour, line width, text size |
| `S` `O` `M` `G` | Thumbnails, overview, markup list, go to page |
| `Ctrl+E` / `Ctrl+Alt+E` | Export flattened / editable PDF next to the original |
| `F1` | All shortcuts, and your name for exported comments |

Markups autosave (`%APPDATA%\Redline` on Windows, `~/.local/share/redline` on Linux); the source PDF is never modified.

## Build and release

CI (`.github/workflows/build.yml`) tests on Linux and Windows, builds `redline.exe`, and packages the Inno Setup installer (`installer/redline.iss`) as a workflow artifact. Pushing a `v*` tag also publishes a GitHub release with the installer, using `docs/release-notes.md`. Bump `version` in `Cargo.toml` and `redline.rc` together.
