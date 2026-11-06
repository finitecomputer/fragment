---
name: slides-finite
description: Make, edit and present slide decks as fragments with open-slide (React pages on a 1920×1080 canvas). Use when a human wants a presentation, a talk, a pitch or slides, wants to change a deck, or wants a deck's notes applied.
metadata:
  hermes:
    tags: [Slides, Presentations, Decks]
    related_skills: [apps-finite, design-finite]
---

# Slides

A deck is a fragment made from the `slides` template: one deck, written
with [open-slide](https://github.com/open-slide/open-slide). Each page is
a React component on a 1920 × 1080 canvas. Anyone with the link sees it.
An editor presses **Present live** and everyone watching follows. Anyone
signed in leaves notes on a page, and you apply them.

`apps-finite` covers fragments in general (links, sharing, members). This
skill covers what is particular to a deck.

## Make one

```sh
mkdir -p ~/apps && cd ~/apps
fragment init q3-review --template slides --title "Q3 review"   # scaffold, create, first deploy
cd q3-review
node scripts/build.mjs                                           # the first build installs open-slide
fragment deploy q3-review --dir .
```

The template holds no build: until the first build and deploy, the deck's
link shows a page saying to build it. Then write the deck the human asked
for:

1. Read open-slide's authoring guide before the first edit. It comes with
   the first build: read
   `node_modules/@open-slide/core/skills/slide-authoring/SKILL.md` (and its
   `references/`). Its rules hold: one `index.tsx`, only `react` and
   `@open-slide/core`, inline styles, every page fits 1920 × 1080.
2. Edit `slides/deck/index.tsx`: the pages (`export default`, in order),
   `meta.title`, `design`, and speaker `notes` (index-aligned with the
   pages). Keep the folder `slides/deck`: one deck per fragment.
3. Build and deploy:

   ```sh
   node scripts/build.mjs
   fragment deploy q3-review --dir .
   ```

4. Look at it before you share it: open the deck's link in your browser
   and check each page fits (nothing cut off at the bottom).

## The build

`node scripts/build.mjs` builds the deck into `site/`, and the fragment
serves `site/`. Never edit `site/` by hand: the next build replaces it.

- The first build on a computer installs open-slide (about 250 MB, about
  15 s) from the deck's lockfile into `/tmp/open-slide/`, outside your
  saved disk, and links it as `node_modules`. After a restart the first
  build installs it again. Later builds take about a second.
- Never `npm install` in the deck's folder: a `node_modules` there is
  saved with your disk.
- Keep `package.json` and `package-lock.json` as they are. Your computer
  installs only npm releases at least 14 days old, so a newer open-slide
  may not install yet.
- `node scripts/build.mjs --check` builds again aside and says whether
  `site/` is that build, changing nothing.
- A failed build says why. A page that throws shows an error in the
  browser, not at build time: open the deck and look.

## Notes

People leave notes on a page. Each note's `anchor` is its page number:
`p1` is the first page of `export default`.

```sh
fragment call q3-review notes      # every note: {id, anchor, text, author, reply_to, resolved}
```

Apply each open note: change the page, build, deploy. Then reply in its
thread with what you changed and resolve it:

```sh
fragment call q3-review annotate --input '{"reply_to": 4, "text": "Shortened the title."}'
fragment call q3-review resolve --input '{"id": 4, "resolved": true}'
```

Ask the human about a note you can't apply; don't guess. Notes stay on
their page number, so after you add, remove or reorder pages, tell the
human which notes now sit on a different page.

## Presenting

Presenting is the human's: **Present live** on the deck's page (editors
and the owner). Everyone who has the deck open follows. open-slide's
present mode (`P`, or `F` for fullscreen) and its presenter view with
notes and a timer work as usual. Exports (PDF, PPTX, HTML) are in the
deck's menu and run in the browser.
