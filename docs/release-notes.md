Greenline 1.3.0 — fast PDF markup for drawing, spec and deck review.

**Install (Windows 10/11, 64-bit):** download `Greenline-1.3.0-setup.exe` below and run it. No admin rights needed; it installs for you only. Installing over Redline or an earlier Greenline keeps your markups.
Because the installer isn't code-signed yet, Windows may show "Windows protected your PC": click **More info → Run anyway**.

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
