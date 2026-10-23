# Art Direction by Surface

Most subjects signal their direction: a photography portfolio is moody and
image-forward, a bakery warm and inviting, a law firm sober and minimal. Read the
signal before reaching for any default.

## Informational sites

Portfolios, editorial and blogs, small business pages, landing pages.

| Type | Direction comes from | Starting points |
| --- | --- | --- |
| Portfolio | The person's work: a fashion photographer is moody and image-forward, an architect precise and grid-structured, a musician rhythmic and warm | Display font matching the creator; accent drawn from the work; neutral surfaces that let the work lead |
| Editorial, blog | The editorial voice: a literary magazine is typographic and spacious, a tech blog clean and dense, a food blog warm and sensory | Serif and sans pairing; body at `--text-base`, line height 1.6-1.7 |
| Small business | The business's character: a consultancy authoritative and minimal, a yoga studio calm and organic | Palette and type from its personality; strong CTA contrast |
| Landing page | The product: a developer tool technical and precise, a wellness product soft and aspirational, an agency bold | One hero moment at `--text-2xl` or above; everything else compact |

### What makes them work

- **Content hierarchy is the design.** Primary navigation of 5-7 items at most.
  Essential content first, depth on demand. Every page answers: where am I,
  what is here, where can I go next?
- **Typography carries the voice.** Editorial: serif headings over a sans body.
  Portfolios: a refined display face at hero scale and an invisible body face.
  Small business: a clean sans for professionalism. Blogs: Source Serif 4 or
  Lora at 16-18px, 65ch measure. UI text (nav, buttons) at `--text-sm`, metadata at `--text-xs`.
- **Whitespace is confidence.** Generous section padding,
  `clamp(var(--space-12), 8vw, var(--space-32))`. Let the hero breathe: 60-80vh
  is a good starting height. Leave
  gaps unfilled rather than adding another CTA or ornament.
- **Rich visual content.** Pages are rarely text-only. Use real or art-directed
  imagery (see `imagery.md`) and one or two full-bleed moments per page.
- **Editorial grids.** Multi-column compositions rather than one stacked column
  (see `layout.md`).
- **One memorable choice.** An elegant typeface, a restrained accent, an
  asymmetric moment. Personality comes from type and spacing more than from
  color or effects.
- **Performance is respect.** LCP under 1.5s, under 800KB initial weight (see
  `review.md`).

### Practices by type

Portfolios:

- Lead with the work: the best 3-5 pieces above the fold.
- One case study with context, process and outcome beats twenty thumbnails.
- Minimal navigation: work, about, contact, maybe writing.
- Contact is an email link, not a twelve-field form.
- The site itself is a portfolio piece.

Editorial and blogs:

- Reading comes first: `--text-base`, line height 1.6-1.7, 65ch.
- Date, author, read time and category are visible but quiet.
- A visual moment every two or three scroll heights: pull quote, full-bleed
  image, embedded media, code block.
- Design the reading journey: wide hero, prose with a sidebar, a full-bleed
  image, a two-column comparison. References: NYT features, Stripe Press, It's
  Nice That.
- Index pages are magazine spreads (one large feature, smaller items,
  image-forward), not a flat list.
- End each article with two or three related pieces.

Small business:

- Answer "what does this business do, and why should I care?" in the first
  viewport.
- Social proof (testimonials, client logos, ratings) within two scrolls.
- One primary CTA per page ("Book a call", "Get a quote"), unmissable.
- Local businesses: hours, location, phone and directions instantly findable.
- Most of the audience is on a phone; design mobile first.

### Navigation

A sticky header that hides on scroll down and returns on scroll up keeps
content in view:

```css
.header {
  position: sticky; top: 0; z-index: 50;
  background: oklch(from var(--color-bg) l c h / 0.9);
  backdrop-filter: blur(12px);
  transition: transform 0.3s var(--ease-out), box-shadow 0.3s var(--ease-out);
}
.header--hidden { transform: translateY(-100%); }
.header--scrolled { box-shadow: var(--shadow-sm); }
```

### Accessibility specifics

- Reading order matches visual order; screen readers follow the DOM, not the
  grid.
- Set `<html lang>`.
- Link text describes the destination.
- Video has captions; audio has a transcript.

## Product UI and dashboards

Product UI is functional, not theatrical. Dashboards should feel like products,
not admin templates.

- Type tops out at `--text-xl`. If a page title feels small, add weight or
  space.
- Full width minus navigation; one scroll region.
- Hierarchy first: the one number or view the operator checks daily dominates;
  supporting views recede.
- Use a component kit for behavior (dialogs, menus, command palettes, data
  tables, forms), then restyle it to the direction card.
- Interactive states use the 180ms curve (see `motion.md`).
- Design every state (see `states.md`); dashboards are mostly loading, empty
  and partial-data states on day one.
- Charts follow `dataviz.md`.

### KPI tiles

- Value at `--text-xl` (product UI), `tabular-nums`, the strongest element in
  the tile.
- Label at `--text-xs` or `--text-sm` in `--color-text-muted`.
- Delta in `--color-success` for up, `--color-error` for down,
  `--color-text-muted` for flat. Check whether up is actually good for this
  metric.
- A sparkline beside the value answers "compared to what?" in a word's width.
- Animate value changes with a count or digit flip.
- A row of identical tiles is a grid of cards; vary emphasis so the primary
  metric leads.
