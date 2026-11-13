# Greenline

Fast, native PDF markup for senior review of drawing sets, specs and decks. Built by [Copeland Building Envelope Consulting](https://copelandbec.com) for its own reviews, in Copeland colours. Rust + egui + PDFium, for Windows and Linux. (Called Redline before 1.3; installing over it keeps your markups.)

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
| `V` / `Esc` | Select: move, resize, double-click or `Enter` to edit text |
| `1`–`6`, `[` `]`, `{` `}` | Colour (Copeland medium green by default, then red, blue, black, orange, yellow), line width, text size |
| `S` `O` `M` `G` | Thumbnails, overview, markup list, go to page |
| `I` | Stamps: seal and signature images (PNG/JPEG) and the date; place them, then copy the set to the same spot on other sheets |
| `Ctrl+G` / `Ctrl+Shift+G` | Group / ungroup the selection (Alt+click picks one piece of a group) |
| Arrows | With a selection: nudge it (`Shift` for bigger steps); otherwise previous/next page and scroll |
| `Ctrl+E` / `Ctrl+Alt+E` | Export flattened / editable PDF next to the original |
| `F1` | All shortcuts, your name for exported comments, and light/dark appearance |

Markups autosave (`%APPDATA%\Greenline` on Windows, `~/.local/share/greenline` on Linux; an old Redline folder is moved there on first launch); the source PDF is never modified. Stamp images are copied into `images/` there, so placed stamps keep working if the original file moves.

## Build and release

CI (`.github/workflows/build.yml`) tests on Linux and Windows, builds `greenline.exe`, and packages the Inno Setup installer (`installer/greenline.iss`) as a workflow artifact. Pushing a `v*` tag also publishes a GitHub release with the installer, using `docs/release-notes.md`. Bump `version` in `Cargo.toml`, `greenline.rc` and the installer default together.

After changing dependencies, run `scripts/third-party-licenses.sh` (needs `cargo install --locked --features cli cargo-about`) to regenerate `THIRD-PARTY-LICENSES.html`, which the installer ships alongside our licences, PDFium's and Montserrat's.

## Contributing

Greenline is provided as-is. Issues are turned off, and pull requests are not being accepted: the source is published so you can read it, build it and modify it for your own use, under the licence below.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Copyright (c) 2026 Copeland Building Envelope Consulting, Inc.

The licence covers the code. It doesn't grant rights to the Copeland or CopelandBEC names, logos or branding. If you distribute a modified version, don't present it as an official Copeland product.
