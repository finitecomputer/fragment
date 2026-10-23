# Credits

- Platform page styles, fonts and landscapes: supplied onboarding HTML reference
  (2026-10-22), including its pointer-following card rim.

- `finite-logo.svg`: Finite's logo supplied by Skyler (2026-10-18),
  copied unchanged above the splash and welcome cards. Its paths are also
  `icon.svg` (full-bleed: black, or white in a dark color scheme), `app-icon.svg`
  (white, on a #181818 square) and `apple-touch-icon.png` (that, rendered at
  180 px with `rsvg-convert`).
- `wallpaper.jpg` and `wallpaper-dark.jpg`: light and dark landscapes supplied
  by Skyler (2026-10-13), copied unchanged from `landscapebg-lightmode.jpeg`
  and `landscapebg-darkmode.jpeg` at their original 2616×1632 resolution.
  Both the desktop and launcher use the same viewport-fixed crop.
- `agent.png`: every agent's image, a CRT monitor tinted to the agent's
  colour (shell.css), from Paul (2026-10-03), trimmed to its
  edges and resized to 240 wide. It is the agent template's
  (`templates/agent/site/agent.png`); this one and the chat template's are
  symlinks to it, and crates/templates embeds the two templates' once.
- `vendor/split-grid.js` (`vendor/split-grid.LICENSE.txt`) and the
  Lucide icons in `icons.js` (`vendor/lucide-LICENSE`): their own licenses.
