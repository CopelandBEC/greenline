# Greenline for agents

`greenline-cli` lets you read a PDF drawing set, propose markups and measurements, and check your work by rendering it. A person reviews everything you propose in the Greenline app and accepts or rejects it. You never change their markups directly, and the PDF itself is never modified.

Every command prints JSON. Errors go to standard error with exit code 1. `greenline-cli help` lists the commands, and `greenline-cli ops` gives the op reference.

In a chat client without a shell, the same commands are MCP tools: add a server that runs `greenline-cli mcp`. Use a script when you can, though. A takeoff of many windows is much faster in one program than in tool calls.

## Coordinates

Everything uses **page points** on the page as displayed: the origin is at the top-left, y runs down, and there are 72 points per paper inch. Pages count from 1. `text`, `paths`, `snap`, `render` and `list` all report in these coordinates, and ops take them.

A render's pixels map back to page points as `x = region[0] + px / px_per_pt`, `y = region[1] + py / px_per_pt`. The render prints both values.

## Work in a script, not one call at a time

Write a short program, in Python or whatever you have, that calls `greenline-cli`, reads its JSON, does the arithmetic and writes one ops file. Then send the whole batch with a single `apply`. One script that finds every window on ten sheets beats hundreds of tool calls, and it can be rerun after a fix.

```python
import json, subprocess
def gl(*args):
    return json.loads(subprocess.run(["greenline-cli", *map(str, args)], check=True, capture_output=True, text=True).stdout)

info = gl("info", "set.pdf")
runs = gl("text", "set.pdf", "--page", 12, "--contains", "W")["runs"]
```

## Seeing the drawing

- `info <pdf>`: page sizes in points and inches, the scale set on each page, scales written on each sheet, and how many markups and proposals each page has.
- `render <pdf> --page N [--dpi 100] [--region x0,y0,x1,y1] [--grid 72]`: a PNG to look at. Zoom with `--region` and a higher `--dpi`, since 100 dpi of a whole sheet is too coarse to read details. `--grid 72` draws labelled lines every inch, so you can read positions off the picture. `--markups`, `--proposals` and `--ops file.json` draw existing markups, pending proposals or your draft ops, exactly as an export would. Look at your draft before you apply it.
- `text <pdf> --page N [--region R] [--contains S]`: the sheet's text runs, each with its box. Use this for tags (window marks, detail callouts, room names), notes and dimension strings. It's far more exact than reading text from an image.
- `paths <pdf> --page N --region R`: the vector lines drawn in that region, as `[x0, y0, x1, y1]`. Scanned sheets have none.
- `snap <pdf> --page N --rect R`: fits a rough rectangle to the drawn lines around it and reports which edges found a line. With a scale set, it also reports the width and height.

**Positions from an image are approximate.** For anything measured, find the object roughly (from a render or from text), then `snap` it or read `paths`, and use those coordinates.

## Scale

Measurements need a scale on their page. Without one they read "no scale".

- A drawing scale (`{"op": "scale", "page": 4, "preset": "1/4\" = 1'-0\""}`) is only right if the PDF is at its printed size. Compare `info`'s sheet size with the title block: **a set reduced to 11 x 17 or letter makes the written scale wrong.**
- Calibrating is safer. Find a dimension string with `text`, find its dimension line with `paths`, and send `{"op": "scale", "page": 4, "calibrate": {"from": [x, y], "to": [x, y], "length": "24'-0\""}}`.
- Details drawn at their own scale get a `"region"`.
- Check one known dimension with a `length` op and `apply --dry-run` before you measure everything.

## Proposing

`apply <pdf> ops.json --agent "<your name>" --summary "<one line>"` checks the whole batch and adds it for review. One bad op rejects the batch, and the error names the op. `--dry-run` checks the ops and reports the measurement values without proposing anything. `proposals` lists what's waiting, `withdraw <pdf> <id>` takes a batch back, and `list --proposals` / `measurements --proposals` include pending work.

Keep markups the way a senior reviewer would write them: a cloud around the problem, plus a callout that says what's wrong and what to do. Put the problem in the callout's first words. Use `"group"` to tie a cloud and its callout together. Red is for problems; the default green is for general notes and measurements.

## Takeoffs

For a window takeoff:

1. Find the window tags with `text`.
2. Find each window's outline near its tag with `paths` and `snap`.
3. Add an `area` op with `"rect"` and `"name"` set to the tag.
4. Run `measurements --proposals --csv takeoff.csv`. Each row has width, height, area and perimeter, as text and as numbers.

The person accepts the markups in Greenline, and the CSV is ready for a spreadsheet.

## Limits

- Check scales against a dimension.
- Say in your summary what you couldn't tell from the drawings.
- A missed discontinuity is worse than an extra question: when unsure, cloud it and ask in the callout.
