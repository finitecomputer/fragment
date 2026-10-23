# Analytical Design

Extends `dataviz.md` with Tufte's *Envisioning Information* (1990), *Visual
Explanations* (1997) and *Beautiful Evidence* (2006). Load for dashboards, dense
displays, sparklines and explanatory graphics.

## The six principles of analytical design

From *Beautiful Evidence*; they apply to any analytical presentation, not only
charts.

1. **Show comparisons, contrasts, differences.** Every display answers
   "compared to what?"
2. **Show causality, mechanism, structure, explanation.** Go past description to
   the why behind the pattern.
3. **Show multivariate data.** Real problems have more than one or two
   variables; reducing to one hides the interactions.
4. **Integrate words, numbers, images and diagrams.** Labels beside the data,
   equations beside the curves they generate.
5. **Document the evidence.** Provenance, authorship, scales, sources and
   measurements make the display trustworthy.
6. **Content comes first.** Analytical presentations stand or fall on the
   quality, relevance and integrity of their content; design cannot rescue weak
   evidence.

In a critique, walk all six. The weakest principle is usually the biggest
improvement.

## Sparklines

Word-sized, data-intense graphics.

- Typographic resolution: sized like text, inline with prose or in table cells.
- No axes, labels or decoration.
- Mark endpoints (start and end, or min and max) where useful.
- Height about the x-height of surrounding text (14-20px); length about a word.
- One colored dot flags the key point (current value or anomaly).
- Pair with the latest value: `120 ▁▂▃▅▇▇▆▅ 132`.
- Stack them in tables so the eye scans vertically: name, sparkline, current,
  delta.

Use them for dashboards with many metrics, trends inside prose, or anywhere a
full chart would dominate. They show shape, not precise value, and they do not
suit categorical or part-to-whole data.

## Layering and separation

Distinct elements can share space when they are layered by value, weight, hue
or transparency instead of separated spatially.

- 1+1=3: two heavy lines side by side create a phantom third. Lighten one.
- Hierarchy by weight: primary data dark and saturated, secondary light gray,
  annotations lighter still.
- Color separates overlapping data; it does not decorate.
- Grids, axes and reference lines whisper: present, unobtrusive.

Squint at the graphic: the important data stay visible and the chartjunk
disappears first.

## Micro and macro

A micro/macro graphic tells different stories at different distances: the macro
view shows the overall pattern, the close read shows individual points, labels
and exceptions. The Vietnam Memorial is the canon: from afar a sweep of names,
up close one name. A table of sparklines works the same way. Show overview and
detail at once through layering instead of choosing between them.

## Escaping flatland

Add dimensions to a flat screen without 3D gimmicks: color (categorical or
sequential), size, shape, position, time (small multiples, animation, inline
series) and layering. 3D bars, pies with depth and distorting isometric
projections add visual dimension without information.

## Range frames and dot-dash plots

- Range frame: the axis line spans only the data's actual range, so the axis
  itself reports the extent.
- Dot-dash plot: a scatter whose axes are replaced by marginal rug plots, so
  each axis shows that variable's distribution.

Every standard element (axis, tick, gridline) can be redesigned to carry data.

## Confections, parallelism, narrative

From *Visual Explanations*.

- Confections assemble images, maps, text and diagrams into one explanatory
  composition: Minard's march on Moscow, Snow's cholera map, exploded technical
  drawings. They work when every element serves the argument.
- Parallelism repeats visual structure to enable comparison: small multiples,
  side-by-side maps, before and after, repeated annotation styles.
- Narrative graphics combine space and time in one frame. Minard encodes troop
  size, geography, direction, temperature and time at once.

## Cause and effect

Causality needs both the variables and the mechanism linking them.

- Show the intervention and the response in the same frame.
- Annotate the mechanism directly on the data.
- Use sequence (small multiples through time) to imply mechanism.
- Pair the data with a process diagram of the proposed cause.

The Challenger O-ring case: the data, plotted against temperature, showed the
risk, but the presentation hid the relationship. Tufte's redesign makes it
unavoidable.

## Extended test

After the seven-question Tufte test in `dataviz.md`:

8. Comparison: does it answer "compared to what?"
9. Causality: is the mechanism visible, not only the pattern?
10. Multivariate: are interactions shown, or is the problem over-reduced?
11. Integration: are words, numbers and images interleaved?
12. Documentation: could a stranger evaluate the evidence?
13. Layering: do important elements dominate and secondary ones recede?
14. Micro/macro: does it reward both a glance and a close read?
