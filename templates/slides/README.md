# slides

A deck of slides, made with [open-slide](https://github.com/open-slide/open-slide)
(MIT): each page is a React component on a 1920 × 1080 canvas. Present it
live and everyone watching follows, page by page. Anyone signed in can
leave a note on a page.

```sh
fragment init my-talk --template slides   # scaffold, create, deploy
# or: fragment new my-talk --template slides, then fragment deploy <name> --dir my-talk
```

## Files

- `slides/deck/index.tsx` is the deck: its pages (`export default`, in
  order), `meta`, `design` and speaker `notes`. open-slide's
  `slide-authoring` skill is the reference for writing one
  (`node_modules/@open-slide/core/skills/` after a build). One deck per
  fragment.
- `package.json` pins open-slide and React to exact versions;
  `open-slide.config.ts` turns off open-slide's list of decks.
- `scripts/build.mjs` builds the deck into `site/` (below).
- `live/` is the live layer the build adds to the page:
  `slides-live.js` (the bar: who is here, Present live, following, notes),
  `slides-live.css`, and `slides-route.js` (`/` opens the deck).
- `app.mjs` and `applib/notes.mjs` are the notes: the doc template's
  mixin, copied whole. A note's `anchor` is its page: `p1` for the first.
- `site/` is the built deck the fragment serves. The template ships one,
  built from the starter deck, so a new deck deploys as it is.

## Building

```sh
npm run build        # or: node scripts/build.mjs
fragment deploy <name> --dir .
```

It needs Node 20.19 or later and npm. The first build installs open-slide
(about 250 MB) into a folder outside the deck, `$OPEN_SLIDE_HOME` or the
system's temporary folder, and links it as `node_modules`; later builds
take about a second. A `node_modules` of the deck's own (`npm install`)
is used instead. Edit `slides/`, build, deploy.

`open-slide dev` (`npx open-slide dev`, after a build) runs open-slide's
own editor on your machine; build and deploy when done.

## Presenting live

An editor or the owner presses **Present live**. Their page goes to the
`present` channel (`{page, live, from}`, which only editors and up may
post), and every open page follows it, in open-slide's present mode too.
A viewer can stop following and follow again. Presenting ends with
**Stop**, or when the presenter's page closes. The presenter's view
(`/s/deck/presenter`, with speaker notes and a timer) is open-slide's.

## Notes

The doc template's four operations (its README has the rules):

```sh
fragment call <deck> notes
fragment call <deck> annotate --input '{"anchor": "p2", "text": "Shorter title?"}'
fragment call <deck> resolve --input '{"id": 1, "resolved": true}'
```

A note stays on its page number: reordering pages leaves notes where
they were.

## Exports

open-slide's export menu makes PDF, PPTX and HTML in the viewer's own
browser; nothing runs on a server.

## Caching

`fragment.json`'s `immutable` keeps the build's hashed `assets/` for
good; pages and files of 1 MiB or more are always checked again.
