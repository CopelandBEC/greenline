Greenline 2.0.0 — fast PDF markup for drawing, spec and deck review.

**Install (Windows 10/11, 64-bit):** download `Greenline-2.0.0-setup.exe` below and run it. No admin rights needed; it installs for you only. Installing over Redline or an earlier Greenline keeps your markups.
Because the installer isn't code-signed yet, Windows may show "Windows protected your PC": click **More info → Run anyway**.

**New in 2.0**
- **Pages: move, insert, delete.** Drag a thumbnail in the sidebar or the overview (`O`) to move the page. Right-click one to insert a blank page or pages from another PDF before or after it, or to delete it. In the overview, `Ctrl+click`/`Shift+click` (or `Ctrl+A`) picks several pages to drag or delete (`Del`) at once. Markups go with their pages, `Ctrl+Z` undoes it, and exports come out in the new order. The PDF itself is never changed.
- **Polylines and polygons.** `N` for a polyline, `Shift+N` for a polygon: click the corners, double-click the last one or press Enter to finish. The first point is marked and the line snaps to it: click it to close the shape as a polygon. Drag a corner to reshape.
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
- **Right-click a page** in the thumbnails or overview to open it in a **new tab** (another view of the same document, sharing its markups; tabs on one document show their page) or in **split view**.
- **Exports say they're done:** a notice in the corner names the file, with **Open** and **Show in folder** (PDF exports, smaller copies, comment and measurement lists). A failed export stays up until closed.
- **Copy and paste markups:** `Ctrl+C` / `Ctrl+X` / `Ctrl+V` paste where the pointer is, also into another tab. Text copied from another program pastes as a note.
- **Text sizes from 4 to 72 pt:** type or drag any size in the toolbar, with presets in its ⏷ menu.
- **Note boxes take a height:** drag a note's bottom edge or corner (or draw the box with `T`), and **Fit text to box** sizes the text to the biggest that fits it. Text **aligns** left, centre or right, and top, middle or bottom.
- **Arrange:** line selected markups up by their left, centre, right, top, middle or bottom (one on its own lines up with the page), or **centre them on the page**.
- **Shift keeps it straight:** lines, arrows, measurements and moves go across, up and down, or at 45°.
- Notes running off the edge of a page still show past it.
- **`Shift+I` inserts an image.**
- A note placed with a click (`T`) now starts its text at the click, not below it.
- **Smaller files.** **Export → Smaller copy** writes `<name>_smaller.pdf` with big scans and photos brought down to 200 dpi and recompressed, and names any page worth a second look; the original is never changed. Placed photos now go into exports as compact JPEGs.
- **A softer look.** Dark mode in soft, slightly cool grays with off-white text instead of near-black, and in both modes soft shadows under pages, thumbnails and dialogs give a little depth. Light, dark or follow the system in `F1`.
- **Faster all round.**
  - Big sets open at once: page sizes are read without loading every page.
  - Page renders are lighter and arrive sooner.
  - Zoomed in, the view is covered by sharp tiles that are kept as you pan, instead of re-rendering the whole view after every move.
  - Switching back to a tab shows its pages at once.
  - Thumbnails come from the page renders.
  - Busy sheets paint several times faster.
  - Autosave, image decoding and the pattern library load off the main thread.
  - Exports and smaller copies run in a process of their own, so you can keep reviewing meanwhile.
- For agents: page numbers follow the reviewer's arrangement (`info` says where each page came from), and proposals made for an earlier order can't be accepted into a new one. `greenline-cli` ops gain `polyline`, `polygon`, `fill`, `line`, `hatch`, `bold`/`italic`/`underline` (for the whole note or a list of its words), `greenline-cli patterns` lists the library, and `greenline-cli shrink` makes a smaller copy.

**Since 2.0.0-rc.1:** moving, inserting and deleting pages; dragging tabs to reorder them; split screen, per-word formatting and smaller copies (planned for 2.1, brought forward); the speed work; and the softer look. Fixes: clicking out corners quickly no longer ends a polyline, polygon or measurement early; sharp corners no longer spike past their point on screen, and a polygon just begun no longer flashes a line across the sheet; the find bar's close button, the unsaved-tab dot and other symbols show instead of empty boxes; and an underlined note being typed underlines as it reflows.

Press `F1` in the app for all shortcuts, your name on exported comments, appearance, and the pattern library folder.
