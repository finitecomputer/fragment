# Publishing and recovering a deck

Load `apps-finite` for the CLI, visibility, sharing and first reveal.
Work from the open-slide workspace root. Keep `base: '/'` and the default
source directories in `open-slide.config.ts`.

## Build for Fragment's exact paths

Fragment serves exact files and directory indexes, with no SPA fallback.
The pinned open-slide router uses `/s/<id>` and `/s/<id>/presenter`, plus
the home, theme and asset views. Copy the built HTML entry to those paths
after **every** successful build; Vite clears its output when rebuilding.
Its absolute asset URLs keep these copies pointing at the same bundles.
Use a POSIX shell for this block:

```sh
npm run build -- --out-dir site && (
  set -e
  for entry in slides/*/index.tsx; do
    [ -f "$entry" ] || continue
    deck_id=${entry#slides/}
    deck_id=${deck_id%/index.tsx}
    mkdir -p "site/s/$deck_id/presenter"
    cp site/index.html "site/s/$deck_id/index.html"
    cp site/index.html "site/s/$deck_id/presenter/index.html"
  done
  mkdir -p site/themes site/assets
  cp site/index.html site/themes/index.html
  cp site/index.html site/assets/index.html
  for theme in themes/*.md; do
    [ -f "$theme" ] || continue
    theme_id=${theme#themes/}
    theme_id=${theme_id%.md}
    mkdir -p "site/themes/$theme_id"
    cp site/index.html "site/themes/$theme_id/index.html"
  done
)
```

Do not deploy if the build or route preparation fails. For a new workspace,
add `fragment.json` beside `package.json`, for example:

```json
{"name":"my-talk","meta":{"title":"My talk"}}
```

Use the chosen label/title. Create the fragment once, then deploy:

```sh
fragment create my-talk
fragment deploy my-talk --dir . --note "First presentation draft"
fragment open my-talk
```

`fragment open` returns the share link when visibility is `link`. For a
direct deck link, use `/s/<id>` on that same host and preserve its `view`
query parameter. Test it in a browser, including a hard reload and
`/s/<id>/presenter`; navigating from the home page alone misses bad routes.
Use `apps-finite`'s sharing rules; publication does not mean public access.

## Revise without losing the source

Sync the whole workspace, not just `site/`: `slides/`, assets, themes,
`open-slide.config.ts`, `package.json` and `package-lock.json` must survive
with the fragment. The CLI excludes `node_modules` and dotfiles (including
`.agents/` and `.gitignore`) and honors `.gitignore` in a git repo; leave
`site/` and source unignored. Regenerate the workspace authoring skills
from the locked package after pulling; they are not synced files.
No Vite dev server or npm process is needed to view the published deck.

After an edit, rerun the build and route block, then `fragment deploy
my-talk --dir . --note "Describe the revision"`. Keep the fragment and deck
id stable so existing links still work. Verify the changed page at that URL.

To recover on a fresh computer, use a new empty folder:

```sh
mkdir -p ~/apps/my-talk-restored
fragment sync my-talk --dir ~/apps/my-talk-restored --mode pull
cd ~/apps/my-talk-restored
npm ci
npm run sync:skills
```

Read the recovered source and upstream skills, edit, and run the same build
and route block before deploying. If the lockfile is absent, report that
reproducible recovery is missing instead of silently resolving new versions.
