# States and Flow

A polished interface handles every state, not only the happy path. If someone can
see it, it must look intentional.

## Simplicity

- One primary action per view. Two equally weighted CTAs mean the view has not
  decided.
- Progressive disclosure over feature dumps: steps, expandable sections, layered
  sheets. Three steps of four fields beat one form of twelve.
- Context-preserving overlays (sheets, dialogs) over full-page navigation when
  the task is short.
- Stacked layers vary in height so depth is unmistakable.
- Every overlay has a title and a dismiss action.
- Long forms become steps with transitions, not a column of stacked inputs.

Self-check: can someone tell within one second what to do next?

## Skeleton loaders

Every data-loading view shows a skeleton that mirrors the real layout: avatar as
circle, title as bar, text as bars. Use surface tokens and a gentle shimmer, with
a static fallback under reduced motion.

```css
@keyframes shimmer { 0% { background-position: -200% 0; } 100% { background-position: 200% 0; } }
.skeleton {
  background: linear-gradient(90deg, var(--color-surface-offset) 25%,
    var(--color-surface-dynamic) 50%, var(--color-surface-offset) 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s ease-in-out infinite;
  border-radius: var(--radius-sm);
}
.skeleton-text { height: 1em; margin-bottom: var(--space-2); }
.skeleton-text:last-child { width: 60%; }
.skeleton-heading { height: 1.5em; width: 40%; margin-bottom: var(--space-4); }
```

## Empty states

Empty states are first impressions. Each one has three parts:

1. A warm message saying what belongs here.
2. The primary action that fills it.
3. A visual: an icon, an illustration or a gentle animation.

```html
<div class="empty-state">
  <div class="empty-state-icon"><i data-lucide="folder-open"></i></div>
  <h3>No projects yet</h3>
  <p>Create your first project to start tracking work.</p>
  <button class="btn btn-primary">Create project</button>
</div>
```

```css
.empty-state {
  display: flex; flex-direction: column; align-items: center; text-align: center;
  padding: var(--space-16) var(--space-8); color: var(--color-text-muted);
}
.empty-state-icon { width: 48px; height: 48px; margin-bottom: var(--space-4); color: var(--color-text-faint); }
.empty-state h3 { color: var(--color-text); margin-bottom: var(--space-2); }
.empty-state p { max-width: 36ch; margin-bottom: var(--space-6); }
```

## Errors

- Inline (validation): next to the field, in `--color-error`, specific ("Email
  is required"), saying how to recover.
- Page level (404, offline, failed load): designed like an empty state, with a
  message and an action. Raw error codes stay out of the UI.
- Use the error color sparingly so it keeps its meaning.

## Image fallbacks

Every image handles failure. The base stylesheet gives `img` a surface-offset
background; on `error`, replace the image with a fallback block (`role="img"`,
`aria-label` from the alt text, a placeholder glyph, the same background).

## Success and feedback

- Important outcomes get inline, contextual, animated feedback. Toasts are for
  background confirmations only.
- Significant completions earn a celebration; routine ones get a subtle
  acknowledgment (see the delight curve in `passes.md`).
- Changed values animate (count, flip or morph) so people notice them.

## First run

- Define the aha moment and shorten the path to it.
- The first screen orients and offers the next action; it is not a blank canvas
  or a mandatory tour.
- Teach at the point of use, in context.
