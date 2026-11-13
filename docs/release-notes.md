Greenline 1.5.0 — fast PDF markup for drawing, spec and deck review.

**Install (Windows 10/11, 64-bit):** download `Greenline-1.5.0-setup.exe` below and run it. No admin rights needed; it installs for you only. Installing over Redline or an earlier Greenline keeps your markups.
Because the installer isn't code-signed yet, Windows may show "Windows protected your PC": click **More info → Run anyway**.

**New in 1.5: measuring**
- **Length `D`** and **Area `E`**. Click the points, then double-click or press `Enter` to finish; an area also closes when you click its first point again. Dragging gives one segment or a rectangle. `Shift` squares a point to the last one, `Backspace` takes one back, and `Esc` drops the measurement.
- **Scale `U`** (or click the scale in the status bar). Pick the drawing scale, click a scale Greenline found written on the sheet, or **calibrate**: drag along a dimension you know and type its length (`12'-6"`, `150"`, `3.2 m`). Details drawn at their own scale get a **region** with its own scale.
- **Check reduced prints.** A drawing scale is only right if the PDF is at its printed size. Greenline warns when a sheet is smaller than a drawing sheet; calibrate those.
- Feet-inches to 1/8" and square feet, or metric. Measurements export with your markups.
- **Export → Measurements (CSV)** lists each one's length or perimeter, area, width and height, as text and as numbers, ready for a takeoff spreadsheet.

**New in 1.5: agents can work with your drawings, and you review what they propose**
- `greenline-cli`, installed next to the app (the installer can add it to your PATH), lets an AI agent or a script read a drawing set. It gets the text and lines with their positions, renders with a coordinate grid, and fits outlines to the drawn lines. It can then set scales and propose markups and measurements in batches. Ask an agent to "cloud any air or water barrier discontinuities on A3.0–A5.1", or to "outline and measure every window on the elevations and give me a takeoff spreadsheet". Have it run `greenline-cli guide` first.
- **Nothing an agent does goes straight into your markups.** Its proposals appear on the sheets with an orange dashed outline and in the **Review** panel (`Y`). Accept or reject a whole batch or each markup. Accepted markups are yours like any other, and `Ctrl+Z` takes them back out.
- Chat apps without a shell can use the same commands as MCP tools: `greenline-cli mcp`.

**Also new**
- The README has a plain no-warranty section. Greenline is provided as is, under the MIT and Apache-2.0 licences. Check measurements against the drawings' dimensions before relying on them.

**New in 1.4: a callout can point at several places**
- With a callout selected, `Ctrl`+click on the drawing adds another leader: the arrowhead lands where you click, and its tail meets the first leader's bend. `Ctrl`+click again for a third, and so on.
- It works straight after placing a callout with `C` — the `Ctrl`+click that ends your typing adds the first extra leader in the same gesture. Drag any arrowhead to fine-tune it.
- Extra leaders are part of the same markup: select, move, recolour, undo, group and export all treat them as one, in flattened and editable PDFs alike.

**New in 1.3.2: Greenline is open source**
- The source is now public at github.com/CopelandBEC/greenline, licensed under MIT or Apache-2.0 (your choice).
- The installer now includes Greenline's licences and the licences of the open-source components it's built with, in the `licenses` folder next to the program.

**New in 1.3.1**
- The markup list and stamp panel now sit on the same soft shaded background as the page thumbnails in light mode, so all the side panels match.

**New in 1.3: Redline is now Greenline**
- New name, new icon and Copeland branding throughout. Installing replaces Redline: its Start-menu entry, "Open with" entry and program folder are removed, and your markups, seals and signatures move to `%APPDATA%\Greenline` the first time Greenline starts.
- Light and dark appearance in the Copeland palette, following your Windows setting. Change it under **Appearance** in `F1`.
- New markups are Copeland green (the brand's medium green, which stands out on black linework). Red is still one keypress away (`2`); the colour keys are now green `1`, red `2`, blue `3`, black `4`, orange `5`, yellow `6`.

**Fixed in 1.2.1**
- Linux: the Open, Export as… and Add… (seal/signature) file dialogs no longer make the desktop report Greenline as "not responding" while they're open.

**New in 1.2: stamp and sign drawing sets**
- Press `I` (or **Stamps** in the toolbar) for the stamp panel, which shares the right side with the markup list.
- **Add…** your seal for each state and your signature (PNG or JPEG). They're kept in Greenline's own folder, so you add them once. A scan's white background is made transparent so title-block lines show through.
- Click a seal, signature or **Place date** in the panel, then click on the sheet (or drag to size it). Greenline remembers the size you last used; the date is today's, in the format you pick, and stays editable.
- Select the set and click **Copy to pages** (all other pages, or a range like `2-14, 16`). Each copy goes the same distance from the nearest page corner, so it stays in the title block even on sheets of a different size. Pages that already carry that seal are skipped, and `Ctrl+Z` undoes the whole copy.
- Each copied set is grouped on its sheet: drag any part to move the set, or `Alt`+drag to move one piece. Check every sheet at a glance in the overview `O`.

**Also new**
- Group and ungroup any markups with `Ctrl+G` / `Ctrl+Shift+G`; `Alt`+click picks one piece of a group.
- With markups selected, the arrow keys nudge them (`Shift` for bigger steps). With nothing selected they still change page and scroll.
- Seals and signatures export in flattened PDFs and as stamps in editable PDFs. They place an image; they are not a certificate-based digital signature.

**Also in Greenline**
- Single-key markup tools: Callout `C`, Text `T`, Cloud `K`, Rectangle `R`, Highlight `H`, Arrow `A`, Line `L`, Pen `P`, Select `V`.
- Select several markups with `Ctrl`/`Shift`+click or by dragging a box around them; move, recolour, resize or delete them together.
- Page thumbnails `S`, all-pages overview `O`, markup list `M`, go to page `G`.
- Export: flattened PDF (`Ctrl+E`), editable PDF where each markup is a movable comment for Bluebeam/Acrobat (`Ctrl+Alt+E`), and a CSV comment list.
- Markups autosave as you work (in `%APPDATA%\Greenline`); the original PDF is never changed.

Press `F1` in the app for all shortcuts and to set the name shown on your exported comments.
