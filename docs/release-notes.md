Greenline 2.0.0-rc.1 — fast PDF markup for drawing, spec and deck review. **Release candidate:** for trying out before 2.0; report anything odd to the team.

**Install (Windows 10/11, 64-bit):** download `Greenline-2.0.0-rc.1-setup.exe` below and run it. No admin rights needed; it installs for you only. Installing over Redline or an earlier Greenline keeps your markups.
Because the installer isn't code-signed yet, Windows may show "Windows protected your PC": click **More info → Run anyway**.

**New in 2.0**
- **Polylines and polygons.** `N` for a polyline, `Shift+N` for a polygon: click the corners, double-click or Enter to finish (click the first point again to close it as a polygon). Drag a corner to reshape.
- **Fill.** The swatch beside the colours sets a fill — any markup colour or white — and its opacity, for clouds, rectangles, polygons, areas, callouts and text boxes. A filled shape selects with a click inside.
- **Line types.** The sample beside the line width: dashed, dotted, dash-dot, or one from your pattern library.
- **Hatches.** In the fill picker, with a scale: diagonal, crosshatch, horizontal, or from the library.
- **Bluebeam patterns.** Import Bluebeam line type (`.blx`) and hatch (`.bhx`) files from either list, or point **F1 ▸ Pattern library folder** at a shared folder — your firm's standards. Markups keep their pattern, so they look the same on anyone's machine.
- **Find text (`Ctrl+F`).** Searches the whole document; matches are highlighted on the sheets. `Enter`/`F3` next, `Shift` for previous, `Esc` closes.
- **Zoom to selection (`Z`).** Also in the zoom menu.
- **Bold, italic, underline.** The **B I U** buttons or `Ctrl+B/I/U` for the selected notes.
- **New blank document (`Ctrl+N`).** Letter to ARCH E or A4 to A1, portrait or landscape, up to 200 pages.
- **Images as markups.** Drop a PNG or JPEG on a sheet, or tool menu ▸ **Insert image…**: full colour, sized to fit, resizable by its corners.
- **Tabs.** Opening another PDF opens it in a tab. `Ctrl+Tab`/`Ctrl+Shift+Tab` switch, `Ctrl+W` or middle-click closes, drag a tab to move it; each keeps its own markups, view and undo.
- **Split screen (`\`).** Two places side by side, each with its own zoom: compare a plan with its details, or two documents. Click a pane (or scroll in it) to work there; pick a tab while a pane is active to show another document in it. The other document's tab is marked ⬌.
- **Per-word formatting.** While typing a note, `Ctrl+B` / `I` / `U` (or **B I U**) format the words selected, or what you type next. Outside the note they still format the whole of the selected notes.
- **Smaller files.** **Export → Smaller copy** writes `<name>_smaller.pdf` with big scans and photos brought down to 200 dpi and recompressed, and names any page worth a second look; the original is never changed. Placed photos now go into exports as compact JPEGs.
- **Faster all round.**
  - Big sets open at once: page sizes are read without loading every page.
  - Page renders are lighter and arrive sooner.
  - Zoomed in, the view is covered by sharp tiles that are kept as you pan, instead of re-rendering the whole view after every move.
  - Switching back to a tab shows its pages at once.
  - Thumbnails come from the page renders.
  - Busy sheets paint several times faster.
  - Autosave, image decoding and the pattern library load off the main thread.
  - Exports and smaller copies run in a process of their own, so you can keep reviewing meanwhile.
- For agents: `greenline-cli` ops gain `polyline`, `polygon`, `fill`, `line`, `hatch`, `bold`/`italic`/`underline` (for the whole note or a list of its words), `greenline-cli patterns` lists the library, and `greenline-cli shrink` makes a smaller copy.

Press `F1` in the app for all shortcuts, your name on exported comments, appearance, and the pattern library folder.
