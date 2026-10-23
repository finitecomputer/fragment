# Typography

Font choice is a design decision, never a default. A jazz festival and a SaaS
dashboard do not share a typeface. Size tokens live in `tokens.md`.

## Choosing fonts

Infer the direction from the subject, tone and audience before picking anything.
A legal firm needs authoritative serifs, a children's learning app a friendly
rounded sans, a fashion brand high-contrast editorial type.

Starting points, to be varied per project. Two unrelated projects (two bakeries,
even) should not share a pairing.

| Concept | Display | Body |
| --- | --- | --- |
| Warm, editorial, literary | Boska, Zodiak, Erode | Source Serif 4, Lora |
| Clean, modern, tech | Cabinet Grotesk, General Sans | Satoshi, Inter |
| Luxury, fashion, refined | Instrument Serif, Cormorant | Switzer, Work Sans |
| Friendly, playful, education | Chillax, Plus Jakarta Sans | Nunito, General Sans |
| Bold, creative, agency | Clash Display, Clash Grotesk | Satoshi, General Sans |
| Authoritative, corporate, legal | Playfair Display, DM Serif Display | DM Sans, Source Sans 3 |
| Technical, developer, data | JetBrains Mono, Geist Mono | Geist, Inter |
| Organic, wellness, nature | Zodiak, Erode | Work Sans, Nunito |
| Retro, vintage, nostalgic | Boska, Gambetta | Switzer, Archivo |
| Minimal, Swiss, precise | Switzer, Satoshi | Satoshi, General Sans |

- Check Fontshare first. Its curated catalog is less overexposed than Google
  Fonts, where Roboto, Open Sans, Lato, Montserrat and Poppins have become
  invisible. Use Google Fonts when Fontshare lacks what you need; mixing the two
  is fine.
- Pick for fit, not fashion. A typeface chosen because it is currently popular
  reads as a trend, not a decision.
- System fonts (Arial, Helvetica, Georgia, Times New Roman, Verdana, Tahoma,
  Trebuchet MS, Calibri) are fallbacks only:
  `font-family: 'Chosen Font', 'System Fallback', sans-serif;`
- Too niche or polarizing for general work: Syne, Savate, Special Gothic
  Expanded One, Anybody, Climate Crisis, Unbounded, Red Rose, Space Grotesk,
  Pally, Comico.
- Never, in any context including games: Papyrus, Comic Sans, Impact, Lobster.
- For a genuinely generic brief, Satoshi or General Sans is the fallback
  pairing base.

## How many families

One family with enough range (weights, widths, optical sizes) can carry the
whole hierarchy; add a second only when it brings character the first cannot.
Two is typical, three is the ceiling. Load only the weights you use.

## Hierarchy

Build meaningful contrast between display, body, label and data text through
family, scale, weight, width or spacing. Everything medium-sized and
medium-weight is the failure.

Self-check: count the distinct type styles on the page (unique combinations of
family, size, weight and color). More than 4-5 means audit ruthlessly.

## Display and body territory

| Font role | Minimum size | Tokens |
| --- | --- | --- |
| Display font | 24px | `--text-xl` and above |
| Body font | 12px | `--text-xs` to `--text-base` |
| Body font, bold, as heading | 18px | `--text-lg` to `--text-xl` |

- Name them in CSS: `--font-display` and `--font-body`.
- Condensed, compressed, extended, high-contrast and decorative faces stay at
  display sizes (24px and up).
- Body text (16-18px) wants even proportions and a generous x-height: Satoshi,
  General Sans, Inter, DM Sans, Work Sans, Source Serif 4.
- Small text (12-14px) wants a large x-height and open counters; prefer sans.
  12px is the floor.
- Display-scale type (`--text-2xl` and up) belongs to informational and landing
  pages. Product UI tops out at `--text-xl`.

## Details

- Line height: 1.6-1.7 for body, about 1.15 for headings.
- Measure: 65-75ch for prose. Headings can run wider.
- `text-wrap: balance` on headings, `text-wrap: pretty` on paragraphs.
- Numbers in tables, KPIs, HUDs and charts:
  `font-variant-numeric: tabular-nums lining-nums` so digits do not shift.
- Label text sits close to what it labels; keep text horizontal where possible.

## Loading

Fontshare:

```html
<link href="https://api.fontshare.com/v2/css?f[]=satoshi@400,500,700&f[]=boska@400,700&display=swap" rel="stylesheet" />
```

Names are lowercase and hyphenated (`general-sans`, `cabinet-grotesk`). Always
add `display=swap`.

Google Fonts:

```html
<link rel="preconnect" href="https://fonts.googleapis.com" />
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
<link href="https://fonts.googleapis.com/css2?family=Instrument+Serif:ital@0;1&family=Work+Sans:wght@300..700&display=swap" rel="stylesheet" />
```

Preconnect to both domains, use variable axis ranges (`wght@300..700`) instead
of separate weights, and always `display=swap`.

## Variable fonts

Variable axes give precise hierarchy without extra families:

```css
h1 { font-weight: 750; }
h2 { font-variation-settings: 'wght' 620, 'wdth' 90; }
```
