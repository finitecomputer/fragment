# Design Review

Judge real UI from screenshots or a live browser. Screenshots are product
review and the way the human sees the work early, as well as evidence at the
end.

## Screenshot protocol

Use a real browser (Playwright or whichever browser tool the environment
provides) against a running preview.

### Before the reveal

1. The page renders the intended content with no fatal console or page errors.
2. One representative desktop screenshot.
3. At most one automatic correction pass.
4. State what remains untested.

Skip exhaustive flows, breakpoint tuning and repeated screenshot-fix cycles on a
direction the human has not seen.

### After the direction is approved

1. Write a short inventory: key flows, important states, and the quality claims
   you expect to make.
2. Desktop first, at 1280px or wider. Exercise the main flow with real clicks
   and keyboard input.
3. Mobile at about 390px. Spot-check 768px when the layout changes there.
4. Light and dark modes, unless the direction card chose one.
5. One error or off-happy-path state, plus empty and loading states.
6. One pass looking only for clipping, overflow, weak contrast, awkward spacing
   and broken hierarchy.
7. Fix, reload, recapture. Keep the screenshots that support each claim.

Look at every screenshot you take. Anything missing from it is missing from the
product.

### Each screenshot

- Specific: the design could not belong to another product, and the signature
  is present.
- Focal point clear in under a second; the squint test holds.
- Type: hierarchy reads, no more than 4-5 distinct styles, no text under 12px,
  display faces only at display sizes.
- Spacing: on the scale, tight within groups, generous between, varied rhythm.
- Color: one accent doing one job, contrast passing, both modes deliberate.
- Alignment optically right; radius, borders and shadows on their hierarchy.
- No clipping, overflow, orphaned words in headings or collisions in charts.
- Every value on screen comes from a token; exceptions serve the signature.
- Copy concrete and said once; no placeholder text.
- Imagery credible, or the space clean.
- Charts pass the eraser, collision and Tufte tests in `dataviz.md`.
- Slop check from the skill passes.
- Mobile: touch targets, thumb reach, nothing hover-only, fixed chrome leaves
  room for content.

## Accessibility

- Semantic landmarks: `header`, `nav`, `main`, `section`, `article`, `aside`,
  `footer`.
- One `h1`, then headings in order without skipped levels.
- Contrast WCAG AA: 4.5:1 for body, 3:1 for large text.
- Keyboard: everything reachable with Tab, Enter, Space and Escape; visible
  focus rings.
- Every `img` has `alt` (empty for decorative images).
- Touch targets at least 44x44px.
- `prefers-reduced-motion` respected.
- Every input has a label; groups use `fieldset` and `legend`.
- A skip-to-content link is the first focusable element.
- Native semantics first; ARIA only where they fall short.
- Works at 200% zoom.

## Performance budgets

Speed is part of the feel.

- Core Web Vitals: LCP under 2.5s (product UI under 2.0s, informational under
  1.5s), INP under 200ms, CLS under 0.1.
- Initial weight under 1.5MB; informational pages under 800KB.
- Images: modern formats, `srcset`, explicit `width` and `height`,
  `loading="lazy"` and `decoding="async"` below the fold, preload the hero.
- Fonts: preconnect, `display=swap`, variable fonts, only the weights used,
  preload the critical face.
- Defer non-critical scripts; lazy-load heavy media and 3D.
- `content-visibility: auto` on long off-screen lists.
