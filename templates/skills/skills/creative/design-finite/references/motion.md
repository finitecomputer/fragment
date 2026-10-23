# Motion and Interaction

Treat the interface as a physical space. Every element moves from somewhere to
somewhere; state changes transition rather than teleport. Motion explains a
change, keeps people oriented, or marks a moment; motion that only decorates
slows the task.

Priority: simplicity, then fluidity, then delight. Animation cannot rescue a bad
layout. How much delight each moment earns is the delight-impact curve in
`passes.md`.

## Easing

Easing is the biggest lever on how motion feels. The right curve makes the same
duration feel responsive; the wrong one makes everything sluggish.

| Easing | Curve | Use for |
| --- | --- | --- |
| ease-out (default) | `cubic-bezier(0.16, 1, 0.3, 1)` | Entries, reveals, user-initiated actions: open a modal, show a menu, press feedback. Peak velocity at the start feels instant |
| ease-in-out | `cubic-bezier(0.4, 0, 0.2, 1)` | On-screen elements moving or morphing: resize, reorder, carousel slide |
| ease-in | `cubic-bezier(0.4, 0, 1, 1)` | Exits only; the element accelerates away |
| linear | `linear` | Scroll-linked timelines, continuous rotation, progress, marquees, hold-to-confirm timers |
| ease | `ease` | Subtle color or opacity hover tints |
| spring | `damping: 30, stiffness: 200` | Layout transitions, shared-element morphs, drag and release |

## Durations

| Context | Duration | Easing |
| --- | --- | --- |
| Hover, focus, active | 180ms (`--transition-interactive`) | ease-out |
| Tooltip, popover | 150-200ms | ease-out |
| Button press | 100-150ms, `scale(0.97)` or `translateY(1px)` | ease-out |
| Modal enter | 300ms, opacity plus `scale(0.95)` to 1 | ease-out |
| Modal exit | 200ms | ease-in |
| Sheet or dropdown enter | 250-300ms, slide from its edge plus fade | ease-out |
| Sheet or dropdown exit | 200ms, reverse direction | ease-in |
| Route change | old view 200ms out, new view 300ms in | ease-in, ease-out |
| Layout morph | 350-500ms | ease-in-out |
| Number count-up | 400-800ms, longer for bigger deltas | ease-out |
| Chart entry | 600-800ms | ease-out |
| Skeleton shimmer | 1.5s loop | ease-in-out |
| Stagger | 40-60ms per item, under 400ms total | |

Exits are faster than entries.

Spring presets: smooth `damping 30, stiffness 200` (default), gentle `20, 120`
(modals, cards), snappy `25, 300` (toggles, buttons), bouncy `12, 200`
(celebrations only). A duration-based spring (`duration: 0.4, bounce: 0.2`)
coordinates sequences more easily.

## Hover, active and cursor

Hover states are only for things that do something when clicked: buttons, links,
clickable cards, nav items, toggles, icon buttons. Headings, paragraphs, static
cards and badges get none.

- Pair hover with `:active`: hover lifts, active presses down. `:focus-visible`
  matches hover plus the focus ring. On `@media (hover: none)` use `:active`
  only.
- List transitioned properties individually; `transition: all` animates layout
  by accident.

```css
.btn {
  transition: background var(--transition-interactive), color var(--transition-interactive),
    box-shadow var(--transition-interactive), transform var(--transition-interactive);
}
.btn:hover { transform: translateY(-1px); box-shadow: var(--shadow-md); }
.btn:active { transform: translateY(0); box-shadow: var(--shadow-sm); }

a.card {
  transition: box-shadow var(--transition-interactive), transform var(--transition-interactive),
    border-color var(--transition-interactive);
}
a.card:hover { transform: translateY(-2px); box-shadow: var(--shadow-md); }
a.card:active { transform: translateY(0); }
```

Cursors: `pointer` for clickable, `default` for static, `grab` and `grabbing` for
draggable, `not-allowed` for disabled (with `opacity: 0.5`), `zoom-in` for
expandable images, `col-resize` and `row-resize` for resizers.

## Reveals without layout shift

Scroll reveals must not shift layout. The element holds its final space from the
start; animate only `opacity`, `clip-path` or `filter`, never `translateY`, on
scroll-triggered elements.

```css
.reveal { opacity: 1; } /* visible when unsupported */
@supports (animation-timeline: view()) {
  .reveal {
    clip-path: inset(100% 0 0 0);
    animation: reveal-clip linear both;
    animation-timeline: view();
    animation-range: entry 0% entry 100%;
  }
}
@keyframes reveal-clip { to { clip-path: inset(0 0 0 0); } }
```

Mount animations (a list rendering, a view entering) may use transform, because
they do not push existing content.

## Native CSS patterns

Enter animations without JavaScript:

```css
dialog[open] {
  opacity: 1; transform: scale(1);
  transition: opacity 0.3s var(--ease-out), transform 0.3s var(--ease-out);
  @starting-style { opacity: 0; transform: scale(0.95); }
}
```

Route changes:

```css
@view-transition { navigation: auto; }
::view-transition-old(root) { animation: fade-out 0.2s var(--ease-in); }
::view-transition-new(root) { animation: fade-in 0.3s var(--ease-out); }
@keyframes fade-out { to { opacity: 0; } }
@keyframes fade-in { from { opacity: 0; } }
```

Animatable custom properties (`@property`) let gradients, angles and counters
animate smoothly. Use `@supports` fallbacks for scroll timelines, view
transitions and `@starting-style`.

## Libraries

- Motion (React, also vanilla): springs, `AnimatePresence` for exits, `layoutId`
  for shared-element morphs, variants for staggers.
- GSAP SVG plugins (DrawSVG, MorphSVG, MotionPath): drawing paths, morphing
  shapes, motion along curves.
- NumberFlow: digits that spin individually for prices, stats and KPIs.
- torph: text morphing for changing labels and status text.
- Splitting.js: per-character or per-word spans for a hero text reveal.
- Rough Notation: hand-drawn underlines, circles and highlights for emphasis.
- Magic UI, Motion Primitives: copy-and-adapt inspiration; restyle to the
  direction.

## Rules

- Shared elements morph between states.
- Direction stays consistent: forward enters from the right, back from the
  left.
- Persistent elements stay put across transitions.
- Text changes morph or crossfade.
- Animate only what changed.
- A loading state travels to where the result will appear.
- Respect `prefers-reduced-motion` with a static fallback.
