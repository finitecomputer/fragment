# board

An agent's designs side by side on one canvas: web pages at desktop,
tablet and phone widths, and screenshots. Drag or scroll to move around,
pinch (or ctrl/⌘ and the wheel) to zoom, **Fit** to see everything. Click
into a page to use it; click the board or press Escape to come back.
When the agent changes a frame, every open board redraws that frame in
place, and everyone keeps their own view.

```sh
fragment init my-designs --template board
```

## Files

- `site/` is the board: `index.html`, `board.js` (the canvas, and the
  frames drawn from the `frames` query), `board.css`.
- `app.mjs` holds the manifest, one row per frame, and serves each
  frame's files from main at `/frames/<id>/<file>`.
- `fragment.json` declares the three operations, each described, so they
  show in `fragment status` and are tools of `fragment mcp`.
- `frames/<id>/` holds a frame's files. They live at main and are never
  deployed: writing them is enough.

## Making frames (for agents)

A frame is a folder, `frames/<id>/`, and a row in the manifest. Write the
files, then call `put_frame`; open boards redraw that frame.

```sh
# a desktop page: one file
fragment write <board> frames/home/index.html --from home.html
fragment call <board> put_frame --input '{"id": "home", "kind": "html", "label": "Home", "size": "desktop"}'

# the same page at a tablet's and a phone's width
fragment call <board> put_frame --input '{"id": "home-tablet", "kind": "html", "label": "Home", "size": "tablet", "file": "index.html"}'
fragment call <board> put_frame --input '{"id": "home-phone", "kind": "html", "label": "Home", "size": "mobile"}'

# a custom width, and a page taller than the window
fragment call <board> put_frame --input '{"id": "wide", "kind": "html", "label": "Wide", "size": "custom", "width": 1920, "height": 3000}'
```

Each id is its own folder: write `frames/home-tablet/index.html` and
`frames/home-phone/index.html` too (the same page, if it is responsive).
Several files in one commit: `fragment write <board> frames/home/index.html
frames/home/app.js --dir out` (text files, at most 16).

**A screenshot or a picture.** `fragment write` sends text, so put
pictures in a folder synced to the board and sync it, which moves main
and deploys nothing:

```sh
fragment sync <board> --dir <folder>          # <folder>/frames/shot/shot.png
fragment call <board> put_frame --input '{"id": "shot", "kind": "image", "label": "Home today", "file": "shot.png", "width": 1440}'
```

`width` scales it (a 2× screenshot of a 1440 page: `1440`); without it, it
shows at its own size. A .png, .jpg, .webp, .gif, .avif or .svg; a file of
1 MiB or more is fine.

**Changing a frame.** Write its files again and call `put_frame` again
(the same input will do). That one frame redraws; nothing else moves.

**Placing frames.** `"x"` and `"y"` (both, in board pixels: a desktop
frame is 1440 wide) put a frame there. Frames with no place are laid out
in rows, in the order they were first put, below the placed ones.

**Taking one off.** `fragment call <board> remove_frame --input '{"id": "shot"}'`.
Its files stay at main; remove them from the synced folder and sync.

**Reading the board.** `fragment call <board> frames` answers the
manifest: each frame's `kind`, `label`, `file`, `size`, `width`,
`height`, `x`, `y` and `rev`.

As tools: `claude mcp add <board> -- fragment mcp <board> --write`.

| Operation | Kind | Who | Answers |
|---|---|---|---|
| `frames` | query | whoever may open the board | `{frames: [...], max}`, in the order first put |
| `put_frame` | mutation | editors and up | `{id, rev, created}`; sets the frame whole and redraws it |
| `remove_frame` | mutation | editors and up | `{removed}`: 1, or 0 when it was gone already |

A retry with the same call id is a replay (the platform's ledger): it
answers the first result and redraws nothing.

## A frame's page

It runs in an iframe sandboxed without `allow-same-origin`: an opaque
origin, so its scripts can't reach the board, the viewer's session, or
this fragment's cookies and storage (`localStorage` throws). It may run
scripts (`allow-scripts`: prototypes move) and send its own forms
(`allow-forms`). Nothing else: no popups, `alert`s, downloads or
navigating the board. Its files are served with the same sandbox, so a
frame opened on its own is no more than on the board.

The sandbox asks for a page's other files as a stranger would, so **on a
board that isn't public, keep a frame to one file**: CSS and scripts
inline, pictures as `data:` URLs or from public sites. On a public board
(`fragment visibility <board> public`) its own other files load too:
`./style.css`, `./app.js`, a built folder.

File names are letters, digits, `.`, `_` and `-`, none starting with a
dot.

## Limits

| | | Why |
|---|---|---|
| Frames | 48 | each page is live in every viewer's browser |
| `id` | 1 to 48 of `a-z 0-9 -` | it names the folder and is part of a URL |
| `label` | 1 to 80 characters | it is one line above the frame |
| `file` | 200 characters | a path in the frame's folder |
| `width` | 240 to 3840 | a small phone to a 4K screen |
| `height` | 240 to 16000 | a long page, whole |
| `x`, `y` | ±100000 | the board's extent |
| One frame | 2 KiB as `frames` lists it | so the manifest (at most 96 KiB) is always one answer |

Each is refused with a reason: by the operation's input schema, or by
the app for what a schema can't say.
