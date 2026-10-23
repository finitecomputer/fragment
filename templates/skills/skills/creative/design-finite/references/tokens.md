# Design Tokens

The token set every screen draws from. Hardcoded values are how a design
drifts.

## Type scale

A fluid scale with `clamp()`. Font choice and pairing live in `typography.md`.

```css
:root {
  --text-xs: clamp(0.75rem, 0.7rem + 0.25vw, 0.875rem);   /* 12 -> 14px */
  --text-sm: clamp(0.875rem, 0.8rem + 0.35vw, 1rem);      /* 14 -> 16px */
  --text-base: clamp(1rem, 0.95rem + 0.25vw, 1.125rem);   /* 16 -> 18px */
  --text-lg: clamp(1.125rem, 1rem + 0.75vw, 1.5rem);      /* 18 -> 24px */
  --text-xl: clamp(1.5rem, 1.2rem + 1.25vw, 2.25rem);     /* 24 -> 36px */
  --text-2xl: clamp(2rem, 1.2rem + 2.5vw, 3.5rem);        /* 32 -> 56px */
  --text-3xl: clamp(2.5rem, 1rem + 4vw, 5rem);            /* 40 -> 80px */
  --text-hero: clamp(3rem, 0.5rem + 7vw, 8rem);           /* 48 -> 128px */
}
```

| Element | Token | Notes |
| --- | --- | --- |
| Tiny labels, badges, metadata | `--text-xs` | 12px is the absolute floor for any text |
| Buttons, nav links, UI chrome | `--text-sm` | UI text recedes below body |
| Body copy, every context | `--text-base` | 16px baseline; long-form may go to 18px |
| Section headings | `--text-lg` | One step above body |
| Page title | `--text-xl` | One per page; display fonts start here |
| Hero heading | `--text-2xl` | Informational and landing pages only, one per page |
| Display | `--text-3xl`, `--text-hero` | Editorial heroes and splash only |

Maximum display size by surface:

| Surface | Max token | Notes |
| --- | --- | --- |
| Informational (portfolio, editorial, landing) | `--text-hero` | Hero only; interior pages cap at `--text-2xl` |
| Brand experience | `--text-2xl` | One hero moment; the rest at `--text-xl` or below |
| Product UI (SaaS, dashboard, admin, commerce) | `--text-xl` | Functional, not theatrical. If a title feels small, add weight or space, not size |

Rules:

- Use 3-4 sizes per page. A typical page uses xs, sm, base and lg; landing pages
  add one hero size.
- Body copy is `--text-base`. Using `--text-lg` for body is the most common
  sizing mistake and makes everything feel bloated.
- Mix `rem` with `vw` in the preferred value (`1rem + 2vw`) so zoom still
  works. Check at 200% zoom.

## Spacing

All spacing derives from a 4px unit.

```css
:root {
  --space-1: 0.25rem;  --space-2: 0.5rem;  --space-3: 0.75rem; --space-4: 1rem;
  --space-5: 1.25rem;  --space-6: 1.5rem;  --space-8: 2rem;    --space-10: 2.5rem;
  --space-12: 3rem;    --space-16: 4rem;   --space-20: 5rem;   --space-24: 6rem;
  --space-32: 8rem;
}
```

- `--space-1` to `--space-3`: tight (icon gaps, input and badge padding).
- `--space-4` to `--space-8`: components (card padding, form gaps).
- `--space-10` to `--space-32`: layout (section padding, gutters).
- Fluid sections: `padding-block: clamp(var(--space-8), 6vw, var(--space-24));`
  and vary it between sections so the rhythm is not uniform.

## Color

### Art direction first

Infer the palette from the subject before reaching for a default. A jazz
festival feels warm and expressive, a law firm sober and restrained, a toy store
bright and playful.

1. The human or brand supplies colors: use them in the role structure below.
2. No colors, but the subject is clear: derive the palette from the subject's
   domain, mood and audience.
3. The subject is ambiguous and the human gave no direction: use the fallback
   palette below.

### Restraint

One accent plus neutrals carries most pages. Extra hues (warning, notification,
the categorical set) are reserved for status and data visualization, where color
encodes something. Use the accent sparingly enough that it still means "act
here".

### Roles

- Three text levels: `--color-text`, `--color-text-muted`, `--color-text-faint`,
  plus `--color-text-inverse`.
- Surface layers: bg, surface, surface-2, surface-offset, surface-dynamic, then
  divider and border.
- Accent and status roles each get base, hover, active and highlight.
- Contrast: WCAG AA, 4.5:1 for body text and 3:1 for large text and chart
  marks, checked on every surface the text sits on. In OKLCH, keep the text and
  background `L` at least 0.40 apart.
- Muted text passes AA and carries secondary copy, labels and captions. Faint
  text is below AA: use it only for placeholders, disabled controls and
  decorative glyphs, never for data or anything that must be read.

### Implementation

- Author in OKLCH. Use `color-mix(in oklab, ...)` for opacity and tints.
- Gradients interpolate better in oklab:
  `linear-gradient(in oklab, var(--color-primary), var(--color-blue))`.
- Wide gamut: give each mode its own richer accent, after the palette blocks
  so it wins:

  ```css
  @media (color-gamut: p3) {
    :root:not([data-theme='dark']) { --color-primary: oklch(0.48 0.14 192); }
    [data-theme='dark'] { --color-primary: oklch(0.66 0.11 205); }
  }
  ```
- Selection color: `::selection { background: oklch(from var(--color-primary) l c h / 0.25); }`.

### Light and dark

Ship both modes by default. Follow `prefers-color-scheme`, add a manual toggle
(sun and moon icon) in the header, and remember the choice where storage is
available. A single mode is right when the art direction depends on it (a game,
an immersive piece, a darkroom portfolio); say so in the direction card.

- Light: `:root, [data-theme='light'] { ... }`. Dark: `[data-theme='dark'] { ... }`.
- Duplicate the dark values inside
  `@media (prefers-color-scheme: dark) { :root:not([data-theme]) { ... } }` for
  visitors who never toggle.
- Dark mode needs its own shadows (lower visibility; see `layout.md`) and its
  own accent lightness, not an inverted light palette.

```js
(function () {
  const t = document.querySelector('[data-theme-toggle]');
  const r = document.documentElement;
  const read = () => { try { return localStorage.getItem('theme'); } catch { return null; } };
  const save = (v) => { try { localStorage.setItem('theme', v); } catch {} };
  let d = read() || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  r.setAttribute('data-theme', d);
  t && t.addEventListener('click', () => {
    d = d === 'dark' ? 'light' : 'dark';
    r.setAttribute('data-theme', d);
    save(d);
    t.setAttribute('aria-label', 'Switch to ' + (d === 'dark' ? 'light' : 'dark') + ' mode');
    // swap the sun/moon icon here
  });
})();
```

Storage can be blocked (sandboxed frames, strict privacy modes); the guards fall
back to the system preference.

### Fallback palette: Nexus

Warm beige surfaces, teal accent, with Satoshi or General Sans as the fallback
type. A safe fallback for genuinely generic briefs, not a default for every
project. Custom palettes keep these variable names and both modes. Muted text
and light-mode gold are one step darker than the original Nexus values so they
clear AA and the 3:1 mark contrast.

```css
:root, [data-theme='light'] {
  --color-bg: #f7f6f2;              --color-surface: #f9f8f5;
  --color-surface-2: #fbfbf9;       --color-surface-offset: #f3f0ec;
  --color-surface-offset-2: #edeae5; --color-surface-dynamic: #e6e4df;
  --color-divider: #dcd9d5;         --color-border: #d4d1ca;

  --color-text: #28251d;            --color-text-muted: #6e6d68;
  --color-text-faint: #bab9b4;      --color-text-inverse: #f9f8f4;

  --color-primary: #01696f;         --color-primary-hover: #0c4e54;
  --color-primary-active: #0f3638;  --color-primary-highlight: #cedcd8;

  --color-warning: #964219;         --color-warning-hover: #713417;
  --color-warning-active: #4b2614;  --color-warning-highlight: #ddcfc6;
  --color-error: #a12c7b;           --color-error-hover: #7d1e5e;
  --color-error-active: #561740;    --color-error-highlight: #e0ced7;
  --color-success: #437a22;         --color-success-hover: #2e5c10;
  --color-success-active: #1e3f0a;  --color-success-highlight: #d4dfcc;
  --color-notification: #a13544;    --color-notification-hover: #782b33;
  --color-notification-active: #521f24; --color-notification-highlight: #dececb;

  /* Data visualization hues: each clears 3:1 on the background for chart marks */
  --color-orange: #da7101;  --color-gold: #b07a00;
  --color-blue: #006494;    --color-purple: #7a39bb;
}

[data-theme='dark'] {
  --color-bg: #171614;              --color-surface: #1c1b19;
  --color-surface-2: #201f1d;       --color-surface-offset: #1d1c1a;
  --color-surface-offset-2: #22211f; --color-surface-dynamic: #2d2c2a;
  --color-divider: #262523;         --color-border: #393836;

  --color-text: #cdccca;            --color-text-muted: #868583;
  --color-text-faint: #5a5957;      --color-text-inverse: #2b2a28;

  --color-primary: #4f98a3;         --color-primary-hover: #227f8b;
  --color-primary-active: #1a626b;  --color-primary-highlight: #313b3b;

  --color-warning: #bb653b;         --color-warning-hover: #b95525;
  --color-warning-active: #993d10;  --color-warning-highlight: #564942;
  --color-error: #d163a7;           --color-error-hover: #b9478f;
  --color-error-active: #9b2f76;    --color-error-highlight: #4c3d46;
  --color-success: #6daa45;         --color-success-hover: #4d8f25;
  --color-success-active: #387015;  --color-success-highlight: #3a4435;
  --color-notification: #dd6974;    --color-notification-hover: #c24a59;
  --color-notification-active: #a53142; --color-notification-highlight: #574848;

  --color-orange: #fdab43;  --color-gold: #e8af34;
  --color-blue: #5591c7;    --color-purple: #a86fdf;
}
```

## Radius, shadow, motion, width

```css
:root {
  --radius-sm: 0.375rem; --radius-md: 0.5rem; --radius-lg: 0.75rem;
  --radius-xl: 1rem;     --radius-full: 9999px;

  /* Layered contact + diffuse shadows, tone-matched to warm surfaces */
  --shadow-sm: 0 1px 2px oklch(0.2 0.01 80 / 0.06), 0 4px 16px oklch(0.2 0.01 80 / 0.04);
  --shadow-md: 0 2px 4px oklch(0.2 0.01 80 / 0.08), 0 12px 32px oklch(0.2 0.01 80 / 0.06);
  --shadow-lg: 0 4px 8px oklch(0.2 0.01 80 / 0.08), 0 24px 48px oklch(0.2 0.01 80 / 0.12);

  --transition-interactive: 180ms cubic-bezier(0.16, 1, 0.3, 1);
  --ease-out: cubic-bezier(0.16, 1, 0.3, 1);
  --ease-in: cubic-bezier(0.4, 0, 1, 1);
  --ease-in-out: cubic-bezier(0.4, 0, 0.2, 1);

  --content-narrow: 640px;   /* prose, forms */
  --content-default: 960px;  /* most content, card grids */
  --content-wide: 1200px;    /* dashboards, multi-column */
  --content-full: 100%;      /* full-bleed moments */

  --font-display: 'Chosen Display', Georgia, serif;   /* see typography.md */
  --font-body: 'Chosen Body', 'Helvetica Neue', sans-serif;
}
[data-theme='dark'] {
  --shadow-sm: 0 1px 2px oklch(0 0 0 / 0.2);
  --shadow-md: 0 4px 12px oklch(0 0 0 / 0.3);
  --shadow-lg: 0 12px 32px oklch(0 0 0 / 0.4);
}
```

How to use radius and shadow lives in `layout.md`; easing and durations in
`motion.md`.

## Base stylesheet

Load before any component styles. It sets the quality floor: box model, text
rendering, balanced headings, readable measure, focus rings and reduced motion.

```css
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

html {
  -webkit-text-size-adjust: none; text-size-adjust: none;
  -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;
  scroll-behavior: smooth;
  hanging-punctuation: first last;
  scroll-padding-top: var(--space-16);
}

body {
  min-height: 100dvh;
  line-height: 1.6;
  font-family: var(--font-body, sans-serif);
  font-size: var(--text-base);
  color: var(--color-text);
  background-color: var(--color-bg);
}

img, picture, video, canvas, svg { display: block; max-width: 100%; height: auto; }
img { background: var(--color-surface-offset); }
ul[role='list'], ol[role='list'] { list-style: none; }
input, button, textarea, select { font: inherit; color: inherit; }
button { cursor: pointer; background: none; border: none; }
table { border-collapse: collapse; width: 100%; }

h1, h2, h3, h4, h5, h6 { text-wrap: balance; line-height: 1.15; }
p, li, figcaption { text-wrap: pretty; max-width: 72ch; }

:focus-visible {
  outline: 2px solid var(--color-primary);
  outline-offset: 3px;
  border-radius: var(--radius-sm);
}

/* Only interactive elements transition; see motion.md */
a, button, [role='button'], [role='link'], input, textarea, select {
  transition: color var(--transition-interactive),
    background var(--transition-interactive),
    border-color var(--transition-interactive),
    box-shadow var(--transition-interactive);
}

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    scroll-behavior: auto !important;
  }
}

.sr-only {
  position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px;
  overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border-width: 0;
}
```

## Framework mapping

The token names are the contract; frameworks only rename them.

- Tailwind: map `--text-*`, `--space-*`, `--radius-*` and colors into the theme
  so utilities resolve to tokens. Use `motion-safe:` and `motion-reduce:` for
  reduced motion.
- shadcn/ui and similar kits: bridge the palette into the kit's variables
  (`--background`, `--foreground`, `--card`, `--primary`, `--muted-foreground`,
  `--destructive`, `--border`, `--ring`) in whatever color format the kit
  expects, for both modes, so every component inherits the palette. Override kit
  transitions with `var(--transition-interactive)`.
- A component kit supplies accessible behavior, not a design. Restyle it to the
  direction card; an unmodified kit is a template look.
