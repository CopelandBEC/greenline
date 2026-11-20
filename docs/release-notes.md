Greenline 2.1.0 — fast PDF markup for drawing, spec and deck review.

**Install (Windows 10/11, 64-bit):** download `Greenline-2.1.0-setup.exe` below and run it. No admin rights needed; it installs for you only. Installing over Redline or an earlier Greenline keeps your markups.
Because the installer isn't code-signed yet, Windows may show "Windows protected your PC": click **More info → Run anyway**.

**New in 2.1**
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
- For agents: `bold` / `italic` / `underline` ops take a list of words in the note, and `greenline-cli shrink` makes a smaller copy.

Press `F1` in the app for all shortcuts, your name on exported comments, appearance, and the pattern library folder.
