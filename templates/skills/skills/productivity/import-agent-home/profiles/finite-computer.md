# Profile: finite.computer

Read this with `SKILL.md` when the layout is `finite-computer`: an agent
your owner kept on finite.computer (the old Finite), exported as a zip of
its home. Its layout file is `finite-computer.layout.json` beside this
one.

You only read the copy. The old agent and its sites keep running on the
old Finite until your owner turns them off.

**What the zip may hold beside the home.** The export adds a few small
files of facts. Each is optional; when one is missing, take the safer
choice.
- `agent/profile.json`: the old agent's `name`, and its `picture_url`.
- `workspace/finitebrain/<id>.json`: a brain's `name`.
- `sites/<site>.json`: a site's old address (`url`), whether it was
  public (`visibility`), the folder it served (`path`), and whether it was
  a single-page app (`spa`).

**Name and picture.** Take the old name only if your owner's answers say
to keep it, from `agent/profile.json`. Fetch its picture only if it is
there too; leave it if it doesn't download as an image of at most
256 KiB.

## What its parts become

The survey's `parts` lists them: `brains` (each a folder under
`workspace/finitebrain/`, only the brains your owner owns) and `sites`
(each a folder under `sites/`). Do them after step 3 (files), before
step 4 (personality). Load `brain-finite` before the brains, and
`apps-finite` before the sites.

**Run each block below as one command, as written.** Your terminal
starts every command in your work folder, and a variable or a `cd` never
carries over to the next: each block sets what it needs and names every
folder whole.

### Brains, as brains

For each brain folder `<id>`: its name is in
`workspace/finitebrain/<id>.json`, or else the first heading of its
`index.md`, or else its id. Make the brain once, as `SKILL.md`'s rules
say (its full name in `progress.md` right after):

```sh
fragment create <a short label from its name> --visibility members --template brain --title "<its name>"
```

Then copy its files into a work folder of the brain's own, leaving out
what the old brain made for itself (`AGENTS.md`, `_index.md` and `.keep`
in each folder, `_wiki/` folders, hidden folders):

```sh
W="$HOME/home-import/work/brains/<id>" H="$HOME/home-import/home/workspace/finitebrain/<id>" && \
fragment home extract <source> workspace/finitebrain/<id> --to "$HOME/home-import/home" --layout <L> && \
mkdir -p "$W" && fragment sync <its name> --dir "$W" && \
tar -C "$H" --exclude=AGENTS.md --exclude=_index.md --exclude=.keep --exclude=_wiki --exclude='*/.*' -cf - . | tar -C "$W" -xf - && \
ls "$W" && find "$W" -type f -not -path "$W/.fragment/*" | wc -l
```

Each top-level folder becomes one of the brain's wikis (`raw/`, `wiki/`,
`index.md`, `log.md`).

**Assets.** A note whose frontmatter `resource` names a file of the old
computer: `/data/<path>` (or `file:///data/<path>`) is the home's
`<path>`. Copy it out (`fragment home extract <source> <path> --to
"$HOME/home-import/home" --layout <L>`), move it to
`"$HOME/home-import/work/brains/<id>/<wiki>/raw/assets/<a clean name>"`,
and rewrite the note's frontmatter as `brain-finite` says (`resource:
raw/assets/<file>`, and `finite_asset` with its `content_type`, `size`
and `content_hash: sha256:<hex>`). Leave a note whose resource is a web
address, or a file the home doesn't hold, as it is.

Then push it, only if its folder holds nothing but the brain: the old
brain's own top level and `fragment.json`. Anything else stops it:

```sh
W="$HOME/home-import/work/brains/<id>" H="$HOME/home-import/home/workspace/finitebrain/<id>" && \
if ls "$W" | grep -vxF -e fragment.json -f <(ls "$H" | grep -vxF -e AGENTS.md -e _index.md -e .keep -e _wiki); then echo "STOP: $W holds more than the brain: not pushed"; else fragment sync <its name> --dir "$W" && fragment call <its name> index_status; fi
```

If it says STOP, push nothing, and say in the report what it listed.
Otherwise check that search finds it:

```sh
fragment call <its name> search --input '{"q": "<a word from one of its pages>"}'
```

The old brain's folder permissions don't carry over: everyone the brain
is shared with reads all of it. Invite no one. Your owner reinvites their
teammates once those teammates have moved.

### Sites, as apps

Each `sites/<site>/` is the site's files, as its repository held them.
Make each app once, as `SKILL.md`'s rules say:

```sh
fragment create <site> --visibility members --template blank --title "<its title>"
```

The app's label is the site's name made valid: lowercase letters, digits
and single dashes (never `--`), at most 57 characters.

**The folder served stays inside the site.** It is `sites/<site>.json`'s
`path`, else its `finite.toml`'s `path`, else the whole folder (`.`). That
`path` is the home's data: use it only if it is a plain relative folder
(`.`, `public`, `site/dist`: no `..`, not starting with `/`), and only
where `realpath` puts it inside the site; otherwise serve the whole site
folder, and say so in the report. One command checks it and copies what
it serves into the app's `site/`, leaving out `finite.toml`:

```sh
W="$HOME/home-import/work/sites/<site>" && \
fragment home extract <source> sites/<site> --to "$HOME/home-import/home" --layout <L> && \
root="$(realpath -e -- "$HOME/home-import/home/sites/<site>")" && \
{ served="$(realpath -e -- "$root/<path>")" || served="$root"; } && \
case "$served/" in "$root/"*) ;; *) served="$root" ;; esac && \
mkdir -p "$W" && fragment sync <its name> --dir "$W" && \
rm -rf -- "${W:?}/site" && mkdir -p "$W/site" && cp -R -- "$served/." "$W/site/" && rm -f -- "$W/site/finite.toml" && \
echo "served: $served" && ls "$W" && find "$W/site" -type f | wc -l
```

Before you deploy, find what only worked on Finite Sites, and fix it,
in the files under `"$HOME/home-import/work/sites/<site>/site/"`:
- **Old addresses.** Links and assets pointing at
  `https://<site>.finite.site/...` should become relative.
- **Old platform calls.** Remove calls to `finite.site` APIs, and an
  `llms.txt` that teaches `fsite`, or adapt them to the app.
- **Deep links.** This platform serves `index.html` only at `/` and at
  folders, so a single-page app's `/about` is not found. Switch it to
  hash routes (`#/about`) when you can.
- **Code under the wrong name.** Browsers here refuse a script not named
  `.js` (`.mjs`, `.cjs`) and a stylesheet not named `.css`. A page that
  loads `<script src="data.json">` comes up empty: rename the file, and
  every place that names it.
- **A site that expects a server:** move what works, and note what
  doesn't.

Open `file://$HOME/home-import/work/sites/<site>/site/index.html` with
your browser tool and see that it renders. Then deploy it, only if its
folder holds nothing but the app (`fragment.json`, `README.md` and
`site`):

```sh
W="$HOME/home-import/work/sites/<site>" && \
if ls "$W" | grep -vxF -e fragment.json -e README.md -e site; then echo "STOP: $W holds more than the app: not deployed"; else fragment deploy <its name> --dir "$W" && fragment status <its name>; fi
```

If it says STOP, deploy nothing, and say in the report what it listed.

**Public.** Make a site public only if its `sites/<site>.json` says
`"visibility": "public"` and your owner's answers say to keep public
sites public: `fragment visibility <its name> public`. Then open its
address and check it. With no `.json`, keep it private, and name it in
the report so your owner can make it public.

Write each old address (`sites/<site>.json`'s `url`, else
`https://<site>.finite.site`) with its new one into
`~/home-import/site-map.json` (`{"https://garden.finite.site":
"https://garden--k3x9.finite.place"}`). Steps 4 and 5 use it.

## The old platform, and this one

When a skill's `attention` line, a job's prompt, the SOUL or a memory
names the old platform, change it as this table says. Drop it when this
table has no row for it.

| On finite.computer | Here |
|---|---|
| `fsite`, Finite Sites, `*.finite.site` | apps: `fragment create`, `fragment deploy` (`apps-finite`); an old address becomes its new one from `site-map.json` |
| `fbrain`, Finite Brain, `.finitebrain` | brains: `fragment sync <brain>` and its `search` (`brain-finite`) |
| Finite Chat, topics, rooms | your chat with your owner; a file goes out as `MEDIA:<path>` (`chat-attachments-finite`) |
| `hermes cron`, the `cronjob` tool | `fragment schedule add --routine` |
| `/data/workspace`, `/data/agent`, `FINITE_*`, `FINITECHAT_*` | your home folder; those variables don't exist here |
| `/nix/store/...`, `LD_LIBRARY_PATH` and other Nix workarounds, `nix-shell` | gone: there is no Nix here. Run a tool by its name; a missing Python package is `pip install`, which stays across restarts |
| `playwright install`, a Chromium of its own | not needed: Playwright runs this computer's Chromium |
| Google token files, `gws auth`, local OAuth | `GOOGLE_OAUTH_ACCESS_TOKEN` (`google-workspace-finite`) |
| `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, a named model | your model tier (`model-council-finite`) |
| one of the old platform's managed skills, by name | this platform's skill for the same job; drop the reference if there is none |

In the SOUL and memories, also drop what was only about how the old
platform worked: publishing with `fsite`, brain syncs, Finite Chat
topics, Telegram setup, and paths under `/data`.

The survey lists the old platform's own managed skills as `managed`
(unchanged, so this platform's own replace them) or `modified-managed`
(edited there: keep only the edit, as `SKILL.md` says).

## What's theirs to do

Tell your owner, in the report:
- **Google:** if they used Google with the old agent, they connect it
  again in Settings. You can't do it for them.
- **Telegram and OpenRouter:** these come over only if they said yes when
  they moved. Otherwise their Telegram bot is paired again from Settings.
- **Sharing:** sites and brains are shared again with people, by them.
- **Teammates:** they invite teammates to a brain once those teammates
  have moved.
- **Old sites:** these keep serving at their old addresses until they are
  turned off. Give them the new addresses.
