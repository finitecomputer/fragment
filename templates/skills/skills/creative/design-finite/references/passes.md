# Design Passes

Load this for a refinement round: the product exists and needs a sharper pass.
Pick one or two passes, run each to its checklist, then sequence the next.

## Distill

The interface is cluttered, noisy or trying to do too much.

- Remove competing actions and redundant information.
- Reduce visual variation: fewer sizes, weights, colors, surfaces.
- Keep the primary job obvious (see simplicity in `states.md`).

Distill quiets capabilities; it does not delete them. Move secondary features
behind disclosure rather than removing them blindly.

## Layout

The page works but feels spatially wrong. Layout problems cause most "this feels
off" reactions even when type and color are fine.

- Spacing follows one scale.
- Tight spacing inside groups, generous spacing between groups.
- Hierarchy survives the squint test.
- Fewer monotonous equal-card grids; vary size by content weight.
- Density matches the content type: prose breathes, data packs.
- Section rhythm varies: tighter for dense content, generous for hero moments.

## Bolder

The design is safe, timid or forgettable.

- One hero moment with stronger contrast in scale and weight.
- Clearer personality drawn from the subject.
- More decisive hierarchy.

Bold is not loud. Neon gradients, glass everywhere and dark-mode-with-glow are
the generic version of bold; reach for scale, type and a sharper signature
instead.

## Quieter

The UI over-signals or feels stressful.

- Fewer accents; one accent color doing one job.
- More whitespace.
- More disciplined color.
- Calmer type and interaction styling.

A stressful interface usually needs quieter before any polish.

## Polish

The last pass, after the features work.

- Alignment is optically correct, not only mathematically centered.
- Spacing follows the system everywhere.
- Every state exists: hover, focus, active, disabled, loading, empty, error,
  success.
- Copy voice is consistent; no placeholder strings or TODO energy.
- Motion is smooth and respects reduced motion.
- Radius, borders and shadows follow their hierarchy (see `layout.md`).
- Every surface gets the same care: settings, empty states and error screens
  included.

Polish refines; it does not redesign.

## Delight

Only after the baseline is stable. Delight amplifies usability; it never
competes with it.

The delight-impact curve: the less often a moment happens, the more delight it
can carry. Daily actions get efficiency with subtle touches; rare moments can be
theatrical.

| Moment | Frequency | Level | Pattern |
| --- | --- | --- | --- |
| Number input | Daily | Subtle | Separators shift as digits are typed |
| Tab navigation | Daily | Subtle | Indicator slides; arrow flips with direction |
| Empty state | First visit | Medium | Gentle animated pointer, illustration |
| Reorder | Occasional | Medium | Items make room, smooth drop |
| Delete | Occasional | Medium | Item leaves with satisfying motion |
| Critical completion | Once | Theatrical | Celebration animation, optional sound |
| Easter egg | Rare | Theatrical | Hidden gesture reveals a moment |

- Quick, non-blocking micro-interactions.
- Celebratory or empathetic moments in the right places.
- Animate numbers when values change (count, flip or morph).
- Aim for at least one moment that makes someone say "oh, that's nice", and no
  constant theatrics.

## Onboard

First-run, empty-state or activation work.

- Define the aha moment.
- Reduce time to first value.
- Replace dead empty states with orientation and a next action.
- Teach at the point of use rather than through ceremony. Most products need a
  better first screen, not a mandatory tour.

## Extract

The product has enough repeated UI to justify design-system cleanup.

- Repeated literal values become tokens.
- Repeated component patterns become primitives.
- Repeated layout patterns become composition rules.

Extract only patterns that are stable and repeated.

## Sequencing

Good: distill, layout, polish · bolder, polish · quieter, polish · onboard,
polish · layout, delight.

Bad: delight before hierarchy is fixed · polish before the feature works ·
extract before patterns settle · bolder and quieter in one pass.

## After each pass

Screenshot the result and re-run the skill's stance and slop check. Check for
rhythm, not only spacing. If the result still feels interchangeable with any
generated SaaS template, the pass was not strong enough.
