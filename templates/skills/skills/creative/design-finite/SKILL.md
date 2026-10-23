---
name: design-finite
description: Design direction, critique and polish for anything people look at, from sites, product UI and dashboards to charts, games and imagery. Use before building or restyling any of them, when a screen looks generic or AI-made, or when a chart needs designing or critique.
metadata:
  hermes:
    tags: [Design, Art Direction, UI, Typography, Color, Motion, Data Visualization, Polish]
    related_skills: [image-generation-finite]
---

# Finite Design

This skill owns design judgment: what the work should look and feel like, and
whether the result does. Building, testing and hosting stay with the project's
own workflow.

## Stance

- **Specific.** Derive the look from the subject's own world: its materials,
  artifacts, vocabulary, environment and audience. Match the emotional tone to
  that audience; a bank, a donor tool and a game should not feel alike. Use real
  content early, because generic copy produces generic composition. Tie every
  major choice (hero, type, layout, imagery, signature) to a fact from the
  brief; a choice with no reason beyond looking polished gets simplified or
  replaced. The test: could this design belong to any other product?
- **Hierarchy.** One focal point per view carries its thesis. Build contrast
  with scale and weight before adding decoration. Squint test: blur your eyes;
  the most important thing stays visible and the noise disappears first.
- **Signature.** Spend boldness on one memorable element rooted in the brief and
  keep the supporting system disciplined. Maximal work needs depth; minimal work
  needs unusually precise type, spacing and detail.
- **Earn.** Every element earns its ink. A container earns its boundary,
  ornament earns its place by carrying meaning (category, status, sequence),
  chart ink carries data. Numbering claims sequence, grouping claims similarity,
  a divider claims a boundary; use each only where the relationship is real.
- **Restraint.** Fewer visual ideas with more conviction. When a design is
  already loud, the move is usually quieter; when in doubt, go classic and
  clean.
- **Complete.** Every state a person can see is designed: hover, focus, active,
  disabled, loading, empty, error, success, first run. The settings page and the
  error screen get the hero's care.

## Workflow

### 1. Read the brief

Infer these from the prompt, the codebase and any existing brand:

1. Product type: marketing, editorial, product UI, dashboard, onboarding,
   internal tool, game.
2. Audience: consumer, executive, analyst, creative, operator, student, donor.
3. Personality: restrained, warm, technical, editorial, playful, premium,
   institutional.
4. Risk budget: conservative refresh, noticeable redesign, high-conviction new
   direction.
5. Constraints: accessibility, brand colors, performance, device mix, dense
   data, approval process.
6. Current weakness (existing work): bland, cluttered, timid, inconsistent,
   loud, weak hierarchy, dead empty states.

Ask a focused question first only when the answer changes the product contract
or makes a first draft unsafe or wasteful. The first reveal answers aesthetic
uncertainty.

Done when you can state the subject, the audience and the view's single job in
one sentence each.

### 2. Choose the mode

- **Creation**: the direction is unsettled. Run 3a, 4, 5, 6.
- **Refinement**: the work exists and needs a sharper pass. Run 3b, the chosen
  passes, 4 only where tokens are missing, then 6.
- **Critique only**: run 3b's audit (or the chart critique in
  [references/dataviz.md](references/dataviz.md)) and report the findings, a
  named direction with 3-5 concrete rules when the work needs one, and a
  prioritized plan.

Creation and final polish happen in separate passes.

### 3a. Creation: write a direction card

Study real references from the subject's world (its print, packaging, signage,
tools, the competitors worth respecting) until the first direction has stopped
being generic. Then write the card:

1. Subject, audience, and the view's single job.
2. Visual source: the materials, artifacts, language, environment or
   conventions that belong to this subject.
3. Type, palette and mode character, described by intent: light and dark, or
   the one mode the direction depends on.
4. Layout principle: how the information moves, groups or unfolds.
5. Signature: one memorable element rooted in the brief, and one aesthetic risk
   worth taking.

The card is a proposal until the human reacts to the reveal; form it in one
quick pass.

Done when all five lines are filled and each traces to a fact from the brief or
a reference.

### 3b. Refinement: audit, then pick passes

Name what is wrong before changing anything:

- hierarchy: what is the focal point, and is it strong enough?
- typography: are size, weight and spacing doing real work?
- composition: is the rhythm intentional, or stacked blocks?
- color: is there a clear palette with contrast and discipline?
- interaction: does motion support the task?
- states: are empty, loading, success and error designed?
- copy: is each idea said once, in the user's vocabulary?

Then pick one or two passes from [references/passes.md](references/passes.md)
(distill, layout, bolder, quieter, polish, delight, onboard, extract) and
sequence the rest by its rules.

Done when every audit line has a named finding or an explicit "fine", and the
chosen passes target the worst findings.

### 4. Define the system before the screens

Set tokens for type, spacing, color roles in light and dark, radius, shadow and
motion ([references/tokens.md](references/tokens.md)). Reuse the chosen scale,
palette roles, spacing rhythm, radius hierarchy and surface treatment
everywhere; an exception must strengthen the signature or mark a real product
distinction.

Done when every size, gap, color, radius and shadow on screen resolves to a
token, and motion uses the easing tokens with the durations in
[references/motion.md](references/motion.md).

### 5. Reveal early

The feeling of rapid creation is part of the product. Early human taste is
worth more than repeated agent guesses, and revision costs more as the work
nears completion.

1. Make the smallest coherent draft that makes the direction tangible: a strong
   first viewport plus one representative section or state.
2. Check only that it renders without fatal errors; make at most one automatic
   correction pass.
3. Show a screenshot. Label what is provisional and what is untested.
4. Offer 2-3 meaningfully different directions in concrete terms (subject cues,
   signature, type, density, color, imagery, motion); recommend one and say why.
5. Make the cost of the next step legible: a quick visual revision versus
   completing the build and the full review.

With precise art direction from the human, reveal early with that one
direction. Small, well-specified edits skip this step.

Done when the human has chosen a direction or redirected in their own words.

### 6. Finish and review

Complete the work with real content and assets, design every state, then
review it as a product with the screenshot protocol in
[references/review.md](references/review.md).

After two failed attempts at the same issue, report it with the available
choices. A newly discovered aesthetic or product decision goes back to the
human; a clear defect gets fixed.

Done when the slop check and every review check pass at wide and narrow widths
in each mode the card chose, and a screenshot backs each quality claim you
make.

## Slop check

Run it before every reveal and every review. Each line gives the target, then
the tell that marks its absence.

- Palette from the subject: one accent plus neutrals, solid fills, critical
  numbers in plain high-contrast type, atmosphere kept to one or two focal
  areas. Tell: purple or indigo gradients, gradient text or buttons, glowing
  orbs, gradient-mesh backdrops, the centered hero on a gradient, neon on dark,
  glass everywhere.
- Composition that follows content weight: a full-width callout, 2+1 or
  staggered sets, section heights and grids that change. Tell: the
  three-column icon, title and blurb grid; hero, features, testimonials,
  pricing and CTA at equal padding.
- Boundaries from spacing, type, alignment and dividers, with a card only where
  the boundary means something. Tell: cards as the default unit, cards inside
  cards.
- Left-aligned body text, descriptions and card content; centered only for
  short hero lines. Tell: everything centered.
- A radius hierarchy matched to element size; status as a dot, label or badge.
  Tell: one bubbly radius everywhere, thick colored left borders on cards.
- Ornament that carries meaning, icons at natural size where they aid
  comprehension, and empty space filled with better content or left to breathe.
  Tell: decorative eyebrows, pills, numbered labels and badges; icons in tinted
  circles; emoji as design; floating blobs; wavy dividers.
- Credible imagery (photography, illustration, generated art, diagrams, product
  shots) or a clean space. Tell: geometric collages, improvised SVG scenes,
  hand-coded mascots.
- Clear contrast between display, body, label and data text. Tell: everything
  medium-sized, medium-weight and equally emphasized.
- Copy that follows *Copy is design* below. Tell: "Welcome to", "Unlock the
  power of", "Your all-in-one solution", helper text repeating its label.
- Effects that explain or elevate: motion explains a change, shadow means
  elevation. Tell: decorative motion that slows the task, shadows and effects
  patching a weak layout.
- Designed states and data: empty states with a message, an action and a
  visual; important outcomes shown inline; charts with data dominant, a
  whispering grid and direct labels. Tell: "No items", toasts for outcomes that
  matter, default chart styling.

Last question: would a designer at a respected studio ship this, and could it
belong only to this product?

## Copy is design

Use the user's vocabulary, concrete nouns from the domain, active control
labels and one consistent name per action. Say each idea once. Errors say what
happened and how to recover; empty states point to the next action; link text
names its destination ("Read the case study"). The best hero copy could only
belong to this one product.

## References

Load what the surface needs.

| Surface | Read |
| --- | --- |
| Website, landing, editorial, portfolio | [tokens](references/tokens.md), [typography](references/typography.md), [layout](references/layout.md), [motion](references/motion.md), [site-types](references/site-types.md), [imagery](references/imagery.md), [review](references/review.md) |
| Product UI, dashboard, internal tool | [tokens](references/tokens.md), [typography](references/typography.md), [layout](references/layout.md), [states](references/states.md), [site-types](references/site-types.md), [dataviz](references/dataviz.md), [review](references/review.md) |
| Chart or data display | [dataviz](references/dataviz.md); add [dataviz-analytical](references/dataviz-analytical.md) for dashboards, dense or explanatory displays |
| Browser game | [games](references/games.md), [tokens](references/tokens.md), [typography](references/typography.md), [motion](references/motion.md) |
| Redesign or polish pass | [passes](references/passes.md), [review](references/review.md), plus the surface's own files |
| Imagery, icons, logos, patterns, 3D, maps | [imagery](references/imagery.md) |

[examples/](examples/) holds before-and-after chart demos: the same data in a
default and a designed treatment.
