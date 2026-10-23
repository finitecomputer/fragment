# Data Visualization

Start with the comparison the reader needs to make. The chart should reveal
the data's shape and support a closer reading. These principles draw on
Edward Tufte's work on graphical integrity and analytical design.

## Choose the form

- Time series: a line makes trends and changes visible.
- Categories: aligned bars or dots make magnitudes easy to compare.
- Several related series: small multiples with shared scales expose differences.
- Precise values: a table may be clearer than a chart. Add sparklines when the
  trend matters alongside the current value.
- Several variables: use position, color or small multiples only for variables
  relevant to the question. Keep the encoding consistent across views.

Use the project's chart library and restyle its output to fit the page's type
and color roles. A new dependency should solve a need the current tools cannot.

## Preserve integrity

- Give quantities their units, dates and source. Make missing data and
  uncertainty visible.
- Bar lengths need a zero baseline. For line charts, choose a scale that makes
  the relevant variation legible and label it clearly.
- Keep scales consistent across panels intended for comparison. Explain a
  changed scale or aggregation instead of hiding it in styling.
- Map values proportionally to the visual property being read: length, area
  or color. Decorative perspective can distort those comparisons.
- Distinguish an observed relationship from a causal explanation. When the
  mechanism matters, show or explain it alongside the data.

## Reduce noise

Let data lead, labels support and grids recede. Put labels near their series
when that is clearer than a separate legend. Use color to encode a category or
emphasize a comparison, with labels or shapes carrying the distinction too.

Run two checks on the rendered chart:

- **Eraser test:** could a border, tick, legend or annotation go without losing
  information? Remove redundant explanations and repeated scale notes.
- **Collision test:** do labels cross each other, data marks or reference lines?
  Move prose into a caption, place labels in the margin, or use a leader line.

A restrained chart can be expressive through composition and emphasis. Effects
earn their place when they clarify a real pattern and preserve its magnitude.

## Check the reading experience

Check both the overall pattern and individual values at the actual display
size. Keep labels readable when the chart narrows; simplify the presentation
or provide a clear scroll affordance rather than shrinking everything.

Essential values must be available without hovering. Give interactive charts
keyboard and touch access, and provide a useful summary or table when the
graphic alone cannot convey the information accessibly.
