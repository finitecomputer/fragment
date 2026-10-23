# Layout and Composition

Spatial composition, widths, borders, radius, shadows, depth and mobile.

## Composition

Build on clean, grid-aligned order, then add one or two moments of surprise: an
overlap, an asymmetric hero, a grid-breaking accent. Surprise works only against
a baseline of order; if every section breaks the grid, none of them surprise.

- Most sections: clean grid, generous whitespace, the 4px spacing system.
- One or two moments per page: asymmetry, overlap, diagonal flow, a full-bleed
  break.
- Vary section heights, container widths and grid structure from section to
  section. Uniform padding everywhere makes a monotonous rhythm.
- Bento grids suit dashboards and portfolios when blocks genuinely differ in
  weight; size each block by its content.
- Let structure express real relationships. Group what belongs together, keep
  tight spacing inside groups and generous spacing between them.
- Make the hero express the page's central claim or experience. A large
  headline, three metrics, feature cards and an accent effect is a template, not
  a hero.

Tools: CSS Grid and subgrid for 2D layouts, container queries for components
that adapt to their slot, `clamp()` for fluid spacing.

## Widths

- Prose: `max-width: 65ch`, centered with `margin-inline: auto`.
- Headings and hero text may run the full content width.
- Data-dense views (dashboards, tables, admin): full width minus navigation.
- Card grids: let content density choose the minimum,
  `grid-template-columns: repeat(auto-fill, minmax(min(300px, 100%), 1fr));`
- Width ladder: `--content-narrow` (prose, forms), `--content-default` (most
  content), `--content-wide` (dashboards, multi-column), `--content-full`
  (full-bleed hero and media).
- Full-bleed is a moment, not the default: one or two per page.
- Masonry only for genuinely variable heights:
  `columns: 3 300px; column-gap: var(--space-4);` with
  `break-inside: avoid` on children.

## Editorial grids

Informational pages should read like a well-designed magazine, not one long
column. Vary the column count section to section: single-column prose, a
two-column feature, a full-bleed image, a three-column set.

- Asymmetric two-column: text beside an image or pull quote; alternate sides
  between sections.
- Feature grid: one large item spanning two columns with two or three smaller
  ones below.
- Full-bleed and inset: full-width image, narrow text, full-width image. The
  width changes create rhythm.
- Sidebar and main: a sticky table of contents or metadata rail beside long-form
  text.
- Pull quotes and callouts that run wider than the body text or break into the
  margin.
- Photo grids with varied spans: one 2x2 image beside two or three smaller ones.

On mobile these collapse to one column; keep full-bleed images and pull quotes as
section dividers so the rhythm survives.

## Borders

- Alpha-blended, not solid gray:
  `border: 1px solid oklch(from var(--color-text) l c h / 0.12);` adapts to both
  modes.
- 1px almost always. 2px is for focus rings, active states and deliberate
  emphasis.
- Borders are structure: quieter than the content they hold.
- Often a surface shift (surface to surface-2) or a shadow separates more
  cleanly than a border.
- Status on a card is a small dot, a text label or a badge; a thick colored edge
  reads as a template.

## Radius

- Use the tokens; build a hierarchy by element size. Chips and badges can be
  pills; inputs and cards `--radius-md` to `--radius-lg`; large containers
  `--radius-lg`, `--radius-xl` or square.
- Dense UI gets smaller radii than hero cards.
- Nested radius accounts for the gap: `inner = outer - gap`. If the gap exceeds
  the outer radius, the inner element is square.

```css
.card { border-radius: var(--radius-xl); padding: var(--space-3); }
.card-inner { border-radius: calc(var(--radius-xl) - var(--space-3)); }
```

- Consistent across the page: if cards use `--radius-lg`, every card does,
  unless a difference means something.

## Shadows

- Shadow means elevation. A shadowed element should feel lifted off the
  surface; flat elements stay flat.
- Tone-match the shadow color to the surface hue; pure black on warm surfaces
  looks dirty.
- Layer a tight contact shadow with a wide diffuse one; the `--shadow-*` tokens
  in `tokens.md` are built this way. Retint them when the palette changes.

```css
.card { box-shadow: var(--shadow-sm); }
.card[href]:hover { box-shadow: var(--shadow-md); }
.floating-panel { box-shadow: var(--shadow-lg); }
```

- Dark mode: shadows barely read. Increase spread, or use a faint light edge
  (`oklch(1 0 0 / 0.03)`) or a lighter surface for elevation.

## Backgrounds and depth

Most surfaces are clean neutrals from the surface tokens. Focal areas (a hero, a
key feature, a primary CTA band) can carry atmosphere, one or two per page and
never as the whole page's backdrop:
stacked radial or conic gradients, a grain overlay (`feTurbulence` at about
`opacity: 0.04`), geometric patterns, layered shadows, `mix-blend-mode`, a
gradient animated through `@property`. Snippets live in `imagery.md`.

`backdrop-filter: blur()` is functional when content scrolls or moves beneath
(sticky headers, panels over a game scene); used as a default card surface it is
the generic glass look.

If a section feels empty, it needs better content or layout, not decoration.

## Mobile first

- Design at 375px first, then expand. A complete mobile experience makes the
  desktop better.
- Touch targets at least 44x44px, padding included.
- One column with generous spacing beats a cramped two-column layout.
- Keep primary actions in thumb reach; consider a sticky bottom bar for the one
  critical action.
- Collapse, don't shrink: bento becomes stacked blocks, side-by-side
  comparisons become tabs, tables become card lists or scroll with a visible
  hint.
- Navigation: five or fewer items become a bottom tab bar; more become a menu
  with grouped sections.
- Check that hero type does not swallow the viewport and that body text and
  inputs stay at 16px or more (smaller inputs trigger iOS zoom).
- Every tappable element shows an `:active` state.
- Hover-only UI needs a tap or focus alternative under `@media (hover: none)`.
- Sticky header, sticky footer and a banner together can leave no content on a
  phone. Budget fixed chrome.
- Check at 375, 390, 768 and 1024px and up.

Dashboards on mobile: the sidebar becomes bottom tabs or a menu, KPIs stack,
charts go full width, tables become card lists or horizontal scroll, and there is
still one scroll region.
