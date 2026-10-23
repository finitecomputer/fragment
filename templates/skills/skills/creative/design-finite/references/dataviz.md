# Data Visualization

Clear, honest, high-density displays, after Edward Tufte's *The Visual Display
of Quantitative Information*, plus the web rules that put charts inside the
design system. For dashboards, dense displays, sparklines or explanatory
graphics, also load `dataviz-analytical.md`.

## Designing a new chart

1. **Clarify the story.** Which comparisons matter? What is the one insight?
   Who reads it, and how closely?
2. **Choose the form.**
   - Many comparisons: small multiples.
   - Dense data: a table, or sparklines inside one.
   - Time series: a line with a minimal grid.
   - Part-to-whole: bars or a table rather than a pie.
3. **Start minimal.** Add only what is necessary; every element earns its ink.
   Default to gray and use color to encode or emphasize.
4. **Eraser test.** For every label, tick, gridline, border and annotation: can
   it go without losing information not already shown elsewhere? Watch for
   duplicate encodings: a numeric label beside a value a tick already marks, a
   legend duplicating direct labels, per-panel scale notes duplicating a shared
   caption. When two labels or marks do the same job, keep one. The test
   targets non-data ink and duplicated labels; a second encoding of a value can
   stay when it reads at a different viewing distance (see aesthetics below).
5. **Collision test.** Draw each text element's bounding box in your head. Does
   another label, a data line or a marker cloud cross it? Fixes: move
   explanatory prose into the caption; put band or epoch labels in a strip above
   the plot; push baseline and reference labels to the outer margin; give
   in-plot annotations a leader line. Watch inverted axes (extremes and
   annotations both crowd the top), shared-scale small multiples (labels stack
   near zero in every panel) and dense scatter (text vanishes in the cloud).
   The eraser test catches redundant elements, the collision test crowded ones;
   both must pass.
6. **Tufte test** (below); for dashboards, dense or explanatory displays, add
   the extended test in `dataviz-analytical.md`. Then judge a screenshot at the
   real rendered size.

## Critiquing a chart

1. **Integrity.** Compute the lie factor where proportions look off. Check
   baselines and scales. Look for 3D distortion.
2. **Chartjunk.** Decoration, heavy grids, needless 3D, moiré patterns.
3. **Data-ink.** What can be erased? What is redundant?
4. **Recommend** specific before-and-after changes.

## Principles

### Graphical excellence

Complex ideas communicated with clarity, precision and efficiency. Excellent
graphics show the data; make the viewer think about substance rather than
method or design; avoid distorting what the data say; present many numbers in a
small space; make large datasets coherent; invite comparison; reveal several
levels of detail; serve a clear purpose; and sit close to their statistical and
verbal description.

### Graphical integrity

```
Lie factor = size of effect shown in the graphic / size of effect in the data
```

1.0 is truthful; beyond 0.95-1.05 is distortion.

1. Representation is directly proportional to the quantities represented.
2. Clear, detailed labeling defeats distortion.
3. Show data variation, not design variation.
4. In time series of money, deflate and keep consistent baselines.
5. The graphic has no more dimensions than the data.
6. Never quote data out of context.

Common violations: 3D effects on 2D data, truncated axes that exaggerate
change, inconsistent intervals, area or volume encoding linear data, missing
context or baselines.

### Data-ink ratio

```
Data-ink ratio = ink devoted to non-redundant data / total ink
```

Maximize it within reason: erase non-data ink (decoration, heavy grids, boxes),
erase redundant data ink (3D where 2D suffices), then revise and edit.

### Chartjunk

Interior decoration that conveys nothing:

- Unintentional optical art: busy patterns, cross-hatching that vibrates.
- The grid: heavy grids compete with the data; mute or remove them, or use light
  gray or dotted lines.
- The duck: graphics that draw attention to their own design.

Indicators: the viewer notices the design before the data; color or pattern
decorates instead of encoding; shadows, gradients and 3D without purpose; clip
art and icons that carry no data.

### Small multiples

The same design repeated, indexed by another variable. Identical scales across
panels, consistent encoding, minimal decoration between panels, clear labels for
what varies, tight spacing so the data dominate. Use them to compare across
categories, periods or conditions, or wherever animation would work but a static
view is needed.

### Data density

Numbers per unit of area is a quality signal. Graphics can usually shrink while
staying readable and gaining impact: sparklines, condensed series, small-multiple
arrays. Ask: what is the smallest readable size, and could this show more data
in the same space?

### Multifunctioning elements

One element can be data point, label, scale marker and grid reference at once:
labeled scatter points, an axis that is itself a data series, a range frame that
ends at the data's extent.

### Aesthetics

- Simple design, complex data. Complexity comes from the data, not decoration.
- Visual hierarchy: data, then labels, then annotations, then grids, then
  borders.
- Color sparingly and with purpose: gray by default, color for emphasis or
  encoding, colorblind-safe, no rainbow maps for sequential data.
- Readable type, a clear size hierarchy, horizontal text, labels close to their
  data.

The austere "Tufte look" is a safe default, not the only honest destination.
The principles demand integrity and density, not a particular aesthetic. The
line runs between ornament that carries information and ornament that distracts
from it: a gradient that brightens at peaks, or a glow on strong values,
amplifies a real encoding and can give a chart an emotional register the austere
version cannot reach. An effect that fabricates or hides signal is chartjunk.

## Charts in the design system

### Color

- Sequential data: shades of the accent.
- Categorical data: harmonious hues from the palette (the reserved data hues in
  `tokens.md`).
- Highlight one series in the accent and set the rest in gray when there is one
  story.
- Check both light and dark modes.

### Type

Body font throughout. Axis labels `--text-xs` or `--text-sm`, titles
`--text-base` or `--text-lg` bold, tooltips `--text-sm`, always
`font-variant-numeric: tabular-nums lining-nums`. Prefer direct labels on lines
and bars over a legend.

### Layout

- At least `--space-8` around a chart container.
- Charts fill their grid cell; no hardcoded dimensions.
- On mobile charts go full width; rotate labels or change the chart type if
  needed.

### Motion

Bars grow from the baseline, lines draw left to right, arcs sweep, all in
600-800ms on the ease-out curve. Value changes morph rather than rebuild.
Respect reduced motion.

### Interaction

- Tooltips snap to the nearest point; nobody should have to hunt for it.
- Click to drill into composite metrics.
- Hovering a category highlights it across adjacent charts.
- Touch: larger hit areas, swipe to pan a time series.

### Libraries

| Need | Use |
| --- | --- |
| Quick chart, vanilla HTML | Chart.js |
| Quick dashboard, React | Recharts |
| Novel or fully custom | D3 |
| Exploratory, concise API | Observable Plot |
| Rich animation, SSR or canvas | Nivo |
| Pixel-level React primitives | Visx |
| Maps | MapLibre GL or D3 (see `imagery.md`) |

A library's default styling is a starting point. Restyle grids, fonts, colors
and labels to the system every time.

### Sparklines

Word-sized trend lines with no axes or labels: about the x-height of the
surrounding text tall and a word long. Draw with SVG `<polyline>` or a chart library with every
axis hidden. Full rules in `dataviz-analytical.md`.

## The Tufte test

1. Data-ink: can any element be erased without losing data? Erase it.
2. Integrity: does the visual effect match the data effect?
3. Chartjunk: does anything exist only for decoration? Remove it.
4. Excellence: does it reveal the data at several levels?
5. Comparison: can the viewer compare elements easily?
6. Density: could it show more data in the same space?
7. Context: are labels, sources and scales all present?

## Examples

`../examples/` holds self-contained before-and-after demos: the same dataset in
a default treatment and a designed one. Open them in a browser to calibrate.
Take their composition and integrity, not their type sizes: their 9-11px labels
sit below the 12px floor.

- `giss-temperature.html`: NASA GISS temperature anomalies as small multiples
  with range frames.
- `sunspot-butterfly.html`: the sunspot butterfly diagram, where the default
  chart destroys the signature pattern.
- `kyoto-sakura.html`: Kyoto cherry-blossom dates, 812-2015.
