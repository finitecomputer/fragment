# Game Visual Design

Art direction, UI and feel for browser games. Engine, physics and testing
mechanics belong to the build workflow; this file covers how the game looks,
reads and sounds.

## Art direction

Set it before any code, from the subject, genre and tone. Palette, lighting,
asset style, UI treatment and loading screens all flow from it.

- Read the concept: horror wants dark palettes, fog and desaturated textures; a
  kids' puzzle game bright primaries and rounded shapes; a sci-fi shooter neon
  accents, metallic materials and volumetric light. Genre licenses looks a
  product UI would avoid.
- Commit to one style: low-poly stylized, realistic PBR, pixel-art-inspired,
  cel-shaded, voxel, synthwave, hand-painted.
- Palette of 3-5 core colors: one dominant, one accent, neutrals. Apply it to
  environment, UI and particles alike.
- Light for mood: warm directional for adventure, cold blue ambient for horror,
  hard rim light for action.
- Post-processing and tone mapping are art direction: bloom, color grading and
  vignette set the mood; ACES filmic reads cinematic, Reinhard softer.
- Pixel art stays crisp: `imageSmoothingEnabled = false` on the canvas and
  `image-rendering: pixelated` on scaled images.
- The UI belongs to the game world: menus, HUD, loading and game-over share its
  palette and type.

## Art assets

Generate or source real art rather than shipping placeholder rectangles.

Always:

- a title or splash image that sets the visual identity
- a loading-screen background
- game-over and victory art for the emotional payoff

When useful: environment or skybox concept art as a reference for the scene,
character and enemy concepts, UI textures and patterns.

Prompts name the style, mood and lighting and repeat the established direction
every time ("low-poly isometric forest at warm sunset, stylized"; "dark
cyberpunk alley, neon reflections on wet pavement, moody"). Landscape for
backgrounds and loading screens, square for icons. Sprites, tiles and UI pieces
need clean cutouts so they composite; `imagery.md` covers how to get them.

Model and texture sources with permissive licenses: Poly Pizza, Kenney,
Quaternius, OpenGameArt, itch.io; ambientCG and Poly Haven for PBR textures and
HDRIs. Prefer stylized low-poly to high-poly realism for browser performance.

## UI typography

All of `typography.md` applies.

- Two fonts at most: a display face for titles and game-over, a legible sans
  for HUD and menus.
- 12px tiny labels, 14px buttons and interactive UI, 16px body and dialog;
  display faces at 24px and up.
- HUD numbers use `tabular-nums lining-nums` at 14-16px so digits do not jitter.

| Genre | Display | HUD and body |
| --- | --- | --- |
| Fantasy, RPG | Cormorant, Playfair, Erode | Satoshi, General Sans |
| Sci-fi, cyber | Cabinet Grotesk, JetBrains Mono | Inter, Geist |
| Horror | Boska, Instrument Serif | Switzer, Inter |
| Casual, puzzle | Plus Jakarta Sans, Chillax | Same family, lighter |
| Retro, pixel | Azeret Mono, Fira Code | Same family |

On canvas, load fonts through CSS and wait for `document.fonts.ready` before
drawing text.

## Game design system

Every screen (title, HUD, menus, dialogs, settings, loading, game-over) shares
one token set adapted from `tokens.md`. Derive the palette from the game's art
direction; the values below are one sci-fi example:

```css
:root {
  --font-display: 'Cabinet Grotesk', sans-serif;
  --font-body: 'Satoshi', sans-serif;
  --color-bg: #0a0a0f;
  --color-surface: rgba(255, 255, 255, 0.05);
  --color-border: rgba(255, 255, 255, 0.12);
  --color-text: #e8e8ec;
  --color-text-muted: #8888a0;
  --color-primary: #4af0c0;   /* health, XP, CTAs */
  --color-danger: #ff4466;
  --color-warning: #ffaa22;
  --panel-blur: 12px;
  --panel-radius: 8px;
  --transition-ui: 180ms cubic-bezier(0.16, 1, 0.3, 1);
}
.game-panel {
  background: var(--color-surface);
  backdrop-filter: blur(var(--panel-blur)); /* the scene shows through */
  border: 1px solid var(--color-border);
  border-radius: var(--panel-radius);
  padding: var(--space-4);
}
```

| Screen | Font | Size | Surface |
| --- | --- | --- | --- |
| HUD (score, health, timer) | body | `--text-sm` | Transparent or subtle, tabular numbers |
| Title | display | `--text-2xl` | Background art |
| Menus, pause | body | `--text-sm` | Panel |
| Dialog, tutorial | body | `--text-base` | Panel |
| Settings | body | `--text-sm`, `--text-xs` | Panel |
| Game over, victory | display | `--text-xl` to `--text-2xl` | Art plus overlay |
| Loading | body | `--text-sm` | Background art |
| Tooltip | body | `--text-xs` | Small panel |

Render the HUD, a menu, buttons and every type size over a scene screenshot and
check contrast and cohesion before building the screens.

## Contrast over a moving scene

Text over a dynamic scene always gets a treatment: a translucent panel, a
vignette, or at minimum
`text-shadow: 0 1px 3px rgba(0,0,0,0.7), 0 0 8px rgba(0,0,0,0.3);`. Screenshot
the HUD over both the brightest and the darkest scenes.

## Readability

- Keep on-screen text minimal; show controls on the start or menu screen rather
  than over play.
- Make key elements easy to see; keep scenes out of murky darkness unless the
  design calls for it.
- Draw the background on the canvas, not in CSS behind it.
- One canvas, centered and filling the viewport.

## Sound

A silent game feels unfinished. Ship music and effects by default.

- Music matched to the genre: ambient for exploration, uptempo for action,
  minimal for puzzles. Loops that loop cleanly, about 128kbps and 30-90 seconds.
- Effects for every interaction: jumps, hits, pickups, UI clicks, explosions,
  ambience. Short procedural tones from Web Audio work for UI feedback.
- Sources: Pixabay Music, Freesound, Incompetech, OpenGameArt, Kenney audio.
  Respect each license's attribution terms.
- Volume and mute controls in settings. Audio starts on the first user gesture,
  so a "click to play" screen doubles as the audio unlock.

## Visual review

| Moment | Capture | Check |
| --- | --- | --- |
| First load | Title, menu | Fonts loaded, layout right |
| Play | Active scene | Entities render, HUD readable, no z-fighting or pop-in |
| Interaction | After an action | State change visible, particles, score updated |
| Edge | Pause, game over | Design system consistent, tokens applied |
| Sizes | 1280px and 375px | Layout holds, touch targets adequate |
| Feel | Any busy scene | Holds about 60fps (55+ average, 30+ on the worst 1% of frames); under 30 fails |

Compare frames at 0, 500 and 1000ms to judge an animation. Go past the start
screen: the screenshots are the source of truth, and anything missing from them
is missing from the game.
