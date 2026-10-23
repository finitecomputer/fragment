# Imagery, Icons and Visual Assets

Use credible imagery or leave the space clean. Intentional photography,
illustration, generated artwork, diagrams and product imagery all work; if the
available asset weakens the design, omit it.

## Choosing imagery

- Real photos and video come from real sources. Use URLs from actual search
  results, never invented ones.
- When real imagery is missing or generic, generate art directed to the
  direction card rather than leaving a gap or using stock filler.
- Product imagery (real screenshots, the actual object) beats illustration of
  the product.
- Diagrams beat decorative illustration whenever there is a mechanism to
  explain.
- One or two full-bleed image moments per page create rhythm; more becomes
  wallpaper.

## Art-directing generated images

Generate through the platform's image step, following `image-generation-finite`
for its contract and limits. It produces JPEGs from text; edits, reference
images, model choice and aspect-ratio controls are not available there. For an
existing image, follow that skill's alternatives: describe a new result or make
a crop, resize, composite or text overlay with code. The design side:

- Carry the direction card into every prompt: subject, palette character,
  lighting, materials, mood and finish. Repeat the shared style phrase across a
  set so the images belong together.
- Describe the look you need in the prompt: expressive illustration, studio
  photorealism, raw film-like photography, or brand and production polish.
- Plan the crop for the slot: landscape for heroes and backgrounds, portrait
  for tall editorial slots, square for tiles and avatars. Keep the subject
  inside that crop and resize the result with code.
- Sprites and UI pieces: ask for an isolated subject on a flat, plain
  background, then cut it out and save as PNG for alpha. The generated JPEG
  has no transparency.
- Images with text: image models still misspell small text. Verify it, or set
  the text in HTML over the image.

When an available tool supports editing an existing image:

1. Inspect the source; name exactly what changes and what must stay.
2. Write the change and the invariants together, naming lighting, perspective
   and the desired finish: "Replace the red mug with the reference bottle; keep
   the hand, table, framing, shadows and background."
3. Add reference images for product identity, character consistency, labels or
   style matching.
4. For nearby or overlapping objects, describe the target's location and the
   neighbors that stay untouched.
5. Prefer one precise edit over a broad restyle when fidelity matters, and make
   the smallest useful follow-up. Repeated full-image passes degrade hands, fine
   detail and text.

## Icons

- Use Lucide (default, 24px grid, 2px stroke) or Phosphor (six weights). Drawing
  or generating icons yields inconsistent strokes.
- Add icons only where they aid comprehension.
- All or nothing in a navigation set: every item has one or none does.
- 16px inline, 20px in buttons and nav, 24px standalone.
- `currentColor`, so icons follow text color in both modes.
- Icon-only buttons need an `aria-label` and a tooltip.
- Icons sit at natural size, not inside tinted circles or rounded squares.

## Logos

A simple custom SVG wordmark or mark is often right for a new project: set in
the display face, tuned spacing, one color that works in both modes. Keep it to
lettering and simple geometry; illustrative logos need a real illustrator or a
generated, art-directed asset.

## Patterns, texture and filters

For focal areas only (see backgrounds and depth in `layout.md`). These use
`currentColor`, so they adapt to both modes:

```html
<svg width="0" height="0" aria-hidden="true">
  <defs>
    <pattern id="dots" width="20" height="20" patternUnits="userSpaceOnUse">
      <circle cx="10" cy="10" r="1.5" fill="currentColor" opacity="0.15" />
    </pattern>
    <pattern id="grid" width="40" height="40" patternUnits="userSpaceOnUse">
      <path d="M 40 0 L 0 0 0 40" fill="none" stroke="currentColor" stroke-width="0.5" opacity="0.1" />
    </pattern>
    <filter id="grain">
      <feTurbulence type="fractalNoise" baseFrequency="0.65" numOctaves="3" stitchTiles="stitch" />
      <feColorMatrix type="saturate" values="0" />
      <feBlend in="SourceGraphic" mode="multiply" />
    </filter>
    <filter id="soft-shadow">
      <feDropShadow dx="0" dy="4" stdDeviation="6" flood-color="oklch(0.2 0.01 80)" flood-opacity="0.12" />
    </filter>
  </defs>
</svg>
```

A pattern that carries the subject (a map grid for a logistics product, ruled
paper for a notebook app) earns its place; a generic dot field rarely does.

## 3D

Use 3D for a hero moment, a product showcase or an immersive piece, not as
ambient decoration. Keep it light: cap the pixel ratio at 2, stay under about
100 draw calls, instance repeated objects, compress geometry and textures, and
lazy-load anything below the fold. Three.js is the default library.

## Maps

- MapLibre GL with OpenFreeMap styles (`liberty`, `bright`, `positron`) for free
  vector maps; pick or tune the style to the palette so the map belongs to the
  page.
- Mapbox GL when terrain or a custom Studio style justifies a key.
- D3 for choropleths and custom projections.
- Restyle markers and popups to the system; default pins look like a template.
