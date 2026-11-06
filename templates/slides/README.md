# slides

A deck of slides, made with [open-slide](https://github.com/open-slide/open-slide)
(MIT): each page is a React component on a 1920 × 1080 canvas. Present it
live and everyone watching follows, page by page. Anyone signed in can
leave a note on a page.

```sh
fragment init my-talk --template slides   # scaffold, create, first deploy (a page saying to build)
cd my-talk
node scripts/build.mjs                    # the first build installs open-slide (below)
fragment deploy my-talk --dir .
```

## Files

- `slides/deck/index.tsx` is the deck: its pages (`export default`, in
  order), `meta`, `design` and speaker `notes`. open-slide's
  `slide-authoring` skill is the reference for writing one
  (`node_modules/@open-slide/core/skills/` after a build). One deck per
  fragment.
- `package.json` pins open-slide and React to exact versions, and
  `package-lock.json` locks everything they need; `open-slide.config.ts`
  turns off open-slide's list of decks.
- `scripts/build.mjs` builds the deck into `site/` (below).
- `live/` is the live layer the build adds to the page:
  `slides-live.js` (the bar: who is here, Present live, following, notes),
  `slides-present.js` (who presents, and posts sent in order),
  `slides-live.css`, and `slides-route.js` (`/` opens the deck).
- `app.mjs` and `applib/notes.mjs` are the notes: the doc template's
  mixin, copied whole. A note's `anchor` is its page: `p1` for the first.
- `site/` is the built deck the fragment serves; the build writes it, and
  `site/build.json` names the hash of every input the build read and of
  every file it made. The template ships no build, only a page saying to
  make one: nothing of open-slide rides in the CLI or in any image.

## Building

```sh
npm run build        # or: node scripts/build.mjs
fragment deploy <name> --dir .
```

It needs Node 20.19 or later and npm. The first build installs open-slide
(about 250 MB, from the lockfile: `npm ci`) and keeps it, so later builds,
of any deck, take about a second with no network. It is kept in
`$OPEN_SLIDE_HOME`, else `$XDG_DATA_HOME/open-slide`, else
`~/.local/share/open-slide`, one folder per lockfile; a new lockfile's
install removes the old one. It is linked into the deck as
`node_modules`; a `node_modules` of the deck's own (`npm ci`) is used
instead. npm's download cache is removed after the install, and a failed
install leaves nothing. Builds in one home take turns, so none loses the
toolchain it is using to another's install. Edit `slides/`, build, deploy.

`node scripts/build.mjs --check` builds the deck again aside and says
whether `site/` is that build, byte for byte, without changing it.

`open-slide dev` (`npx open-slide dev`, after a build) runs open-slide's
own editor on your machine; build and deploy when done.

## Presenting live

An editor or the owner presses **Present live**. Their page goes to the
`present` channel (`{page, live, from}`, which only editors and up may
post), and every open page follows it, in open-slide's present mode too.
The latest Present wins: an earlier presenter stops when someone presses
it after them. A viewer can stop following and follow again.
Presenting ends with **Stop**, or when the presenter's page closes. The presenter's view
(`/s/deck/presenter`, with speaker notes and a timer) is open-slide's.

## Notes

The doc template's four operations (its README has the rules):

```sh
fragment call <deck> notes
fragment call <deck> annotate --input '{"anchor": "p2", "text": "Shorter title?"}'
fragment call <deck> resolve --input '{"id": 1, "resolved": true}'
```

A note stays on its page number: reordering pages leaves notes where
they were. A note being written keeps the page it was started on.

## Exports

open-slide's export menu makes PDF, PPTX and HTML in the viewer's own
browser; nothing runs on a server.

## Caching

`fragment.json`'s `immutable` keeps the build's hashed `assets/` for
good; pages and files of 1 MiB or more are always checked again.
