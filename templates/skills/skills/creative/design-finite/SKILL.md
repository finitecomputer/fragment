---
name: design-finite
description: Design direction and polish for sites, apps, charts and games. Use when creating a visual interface, critiquing a design, or improving its hierarchy, layout, typography or imagery.
metadata:
  hermes:
    tags: [Design, UI, Typography, Data Visualization]
    related_skills: [image-generation-finite]
---

# Finite Design

Make the work feel specific to its subject and useful to its audience. Scale
the effort to the request. For Fragment apps, `apps-finite` owns the build,
publish and first-reveal workflow; this skill supplies visual judgment.

## Choose a direction

Start with the audience, the view's main job and the existing brand. Preserve
what already works unless the human wants a new direction.

For new work, choose a visual idea from the subject's world: its materials,
artifacts, setting or language. A literary archive, a garden planner and a
trading screen should have different reasons for their type, color and density.
Use real content early; it should shape the layout. Consult references when
they resolve an uncertain choice, then make the direction tangible in a small
draft. A minor edit needs no new design process.

## Design with intent

- **Specificity.** Give the work a memorable choice rooted in the brief: type,
  composition, imagery or an interaction. Keep the supporting choices coherent.
  If swapping the name and accent color makes it fit any product, revisit the
  content and composition.
- **Hierarchy.** Make the primary task or idea easy to find. Use scale, weight
  and spacing to distinguish it from supporting detail. Content weight should
  determine section size and rhythm.
- **Structure.** Group related content through proximity and alignment. Use
  cards, borders and labels where they express a real boundary or relationship.
  Let prose breathe and keep dense information easy to compare.
- **Consistency.** Reuse the project's components and tokens. Where none exist,
  establish a small set of type, spacing and color roles as the design develops.
  Choose fonts and colors for the subject; carry that choice across states.
- **Restraint.** Spend detail where it helps the work. Motion can explain a
  change, a shadow can signal elevation, and ornament can carry character.
  Simplify effects that compete with the content or slow the task.
- **Copy.** Use the person's vocabulary and concrete content. Say each idea
  once. An action names what it does; an error explains how to recover.
- **Imagery.** Use relevant photographs, illustrations, diagrams or product
  screenshots. Keep a space clean when an asset adds nothing. For generated
  imagery, follow `image-generation-finite` and its supported capabilities.

## Refine existing work

Name the biggest visible weakness before editing: unclear hierarchy, cramped
spacing, weak type contrast, distracting decoration, or an unfinished state.
Make a focused pass on that weakness and inspect the result before widening
the change. Preserve the existing visual language unless it causes the problem.

For critique only, report prioritized findings with the affected element,
the problem it creates for the user and a concrete improvement.

## Review the result

Inspect the rendered work at its actual size, at wide and narrow widths, in
the modes it ships. Exercise the states affected by the change. Look for:

- A clear focal point, readable type and grouping that matches the content.
- Clipping, overlapping labels, crowded controls or a layout that loses its
  hierarchy on a phone.
- Keyboard access, visible focus, meaningful labels and usable touch targets.
- Loading, empty and error states that explain what happens next, where relevant.
- Motion that respects reduced-motion preferences and leaves the task responsive.

Measure text contrast against every background the text role can appear on,
including interactive states and each shipped color mode. Normal text needs
at least 4.5:1, including muted labels, captions and placeholders. Use 3:1 only
for text meeting WCAG's large-text definition. A muted role still has to be
readable; reserve lower-contrast treatment for genuinely disabled controls or
pure decoration. Verify the actual foreground/background pairs, accounting for
opacity, rather than assuming a token name or lightness difference guarantees
contrast. See [WCAG text contrast](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html).

Use screenshots to judge composition and show meaningful changes. State what
you checked and what remains untested.

## When needed

- For charts and dashboards, read [Data visualization](references/dataviz.md).
- For browser-game art direction and readability, read [Games](references/games.md).
