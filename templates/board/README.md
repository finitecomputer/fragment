# board

An agent's designs side by side on one canvas: web pages at desktop,
tablet and phone widths, and screenshots. Drag or scroll to move around,
pinch (or ctrl/⌘ and the wheel) to zoom, **Fit** to see everything. Click
into a page to use it, and press **Done** to come back to the board.
When the agent changes a frame, every open board redraws that frame in
place, and everyone keeps their own view.

```sh
fragment init my-designs --template board
```

## Files

- `site/` is the board: `index.html`, `board.js` (the canvas, and the
  frames drawn from the `frames` query), `board.css`.
- `app.mjs` holds the manifest, one row per frame, and says where each
  frame's file is at main (`/frames/<id>`).
- `fragment.json` declares the three operations, each described, so they
  show in `fragment status` and are tools of `fragment mcp`.
- `frames/<id>/` holds a frame's file. It lives at main and is never
  deployed: writing it is enough.

## Making frames (for agents)

**A frame is one file**: an HTML page with everything it needs inside it,
or a picture. The board shows that file and nothing beside it, on every
board, public or not: a page's `./style.css`, `./app.js` or `./logo.png`
never load. Write the file to `frames/<id>/`, then call `put_frame`; open
boards redraw that frame.

```sh
# a desktop page
fragment write <board> frames/home/index.html --from home.html
fragment call <board> put_frame --input '{"id": "home", "kind": "html", "label": "Home", "size": "desktop"}'

# the same page at a tablet's and a phone's width: each id its own folder
fragment write <board> frames/home-tablet/index.html --from home.html
fragment call <board> put_frame --input '{"id": "home-tablet", "kind": "html", "label": "Home", "size": "tablet"}'
fragment write <board> frames/home-phone/index.html --from home.html
fragment call <board> put_frame --input '{"id": "home-phone", "kind": "html", "label": "Home", "size": "mobile"}'

# a custom width, and a page taller than the window
fragment call <board> put_frame --input '{"id": "wide", "kind": "html", "label": "Wide", "size": "custom", "width": 1920, "height": 3000}'
```

`fragment write` sends text of up to 1 MiB. A larger page (pictures
inline make one quickly) and every picture go through a folder synced to
the board, which moves main and deploys nothing:

```sh
fragment sync <board> --dir <folder>          # <folder>/frames/shot/shot.png
fragment call <board> put_frame --input '{"id": "shot", "kind": "image", "label": "Home today", "file": "shot.png", "width": 1440}'
```

**One file from a build.** Bundle it into one HTML file, CSS, scripts and
pictures inside:

- **Vite:** add [`vite-plugin-singlefile`](https://github.com/richardtallent/vite-plugin-singlefile)
  to your own project (`npm i -D vite-plugin-singlefile`, then
  `plugins: [viteSingleFile()]` in `vite.config.js`), build, and copy
  `dist/index.html` to `frames/<id>/index.html`.
- **By hand:** put CSS in `<style>`, scripts in `<script type="module">`
  (no `import` of local files), and pictures as `data:` URLs
  (`data:image/png;base64,` then `base64 -w0 logo.png`), or link them
  from public sites.

**A screenshot or a picture.** A .png, .jpg, .webp, .gif, .avif or .svg.
`width` scales it (a 2× screenshot of a 1440 page: `1440`); without it,
it shows at its own size.

**Changing a frame.** Write its file again and call `put_frame` again
(the same input will do). That one frame redraws; nothing else moves. If
its file can't be read (not written yet, or over 16 MiB), the board says
so on the frame, keeps showing the last version, and tries again.

**Placing frames.** `"x"` and `"y"` (both, in board pixels: a desktop
frame is 1440 wide) put a frame there. Frames with no place are laid out
in rows, in the order they were first put, below the placed ones.

**Taking one off.** `fragment call <board> remove_frame --input '{"id": "shot"}'`.
Its file stays at main; remove it from the synced folder and sync.

**Reading the board.** `fragment call <board> frames` answers the
manifest: each frame's `kind`, `label`, `file`, `size`, `width`,
`height`, `x`, `y` and `rev`.

As tools: `claude mcp add <board> -- fragment mcp <board> --write`.

| Operation | Kind | Who | Answers |
|---|---|---|---|
| `frames` | query | whoever may open the board | `{frames: [...], max, file_max_bytes}`, in the order first put |
| `put_frame` | mutation | editors and up | `{id, rev, created}`; sets the frame whole and redraws it |
| `remove_frame` | mutation | editors and up | `{removed}`: 1, or 0 when it was gone already |

Each put takes the board's next `rev`, so a frame removed and put again
never has a rev it had before. A retry with the same call id is a replay
(the platform's ledger): it answers the first result and redraws
nothing.

## A frame's page

The board reads the page's bytes and shows them in an iframe sandboxed
without `allow-same-origin`: an opaque origin, so its scripts can't reach
the board, the viewer's session, or this fragment's cookies and storage
(`localStorage` throws). It may run scripts (`allow-scripts`: prototypes
move) and send its own forms (`allow-forms`). Nothing else: no popups,
`alert`s, downloads or navigating the board. Nothing serves a frame as a
page of this fragment: the platform serves the file itself inert.

A frame's keys stay inside it, so **Done** (top right, while you use a
page) is the way back; Escape works once the board has the keys again.

## Limits

| | | Why |
|---|---|---|
| Frames | 48 | each page is live in every viewer's browser |
| A frame's file | 16 MiB | every viewer downloads it whole each time it changes |
| `id` | 1 to 48 of `a-z 0-9 -` | it names the folder and is part of a URL |
| `label` | 1 to 80 characters | it is one line above the frame |
| `file` | 200 characters, names of `A-Z a-z 0-9 . _ -`, none starting with a dot | a path in the frame's folder |
| `width` | 240 to 3840 | a small phone to a 4K screen |
| `height` | 240 to 16000 | a long page, whole |
| `x`, `y` | ±100000 | the board's extent; Fit shows it all |
| One frame | 2 KiB as `frames` lists it | so the manifest (at most 96 KiB) is always one answer |

Each is refused with a reason: by the operation's input schema, by the
app for what a schema can't say, or, for a file's size, on the frame
itself.
