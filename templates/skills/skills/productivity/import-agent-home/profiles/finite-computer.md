# Profile: finite.computer

Read this with `SKILL.md` when the layout is `finite-computer`: an agent
your owner kept on finite.computer (the old Finite), staged for you by
the platform. Its layout file is `finite-computer.layout.json` beside
this one.

The old agent is stopped and its copy is read-only. Its old sites keep
serving at their old addresses until they are turned off.

## What its parts become

The survey's `parts` lists them: `brains` (each a folder under
`workspace/finitebrain/`, only the brains your owner owns) and `sites`
(each a folder under `sites/`). Do them after step 3 (files), before
step 4 (personality). Load `brain-finite` before the brains, and
`apps-finite` before the sites.

### Brains, as brains

For each brain folder `<id>`:

```sh
fragment home extract <source> workspace/finitebrain/<id> --to ~/home-import/home --layout <L>
fragment create <a short label from its name> --visibility members --template brain --title "<its name>"
mkdir -p ~/home-import/work/brains/<id> && cd ~/home-import/work/brains/<id>
fragment sync <its name> --dir .
```

Its name is in your owner's answers, or else in the first heading of its
`index.md`. Copy its files in (`~/home-import/home/workspace/finitebrain/<id>/`),
leaving out what the old brain made for itself:
- `AGENTS.md`, `_index.md` and `.keep` in each folder;
- `_wiki/` folders;
- hidden folders.

Each top-level folder becomes one of the brain's wikis (`raw/`, `wiki/`,
`index.md`, `log.md`).

**Assets.** A note whose frontmatter `resource` names a file in the
copy: move that file to `<wiki>/raw/assets/<a clean name>`, and rewrite
the note's frontmatter as `brain-finite` says (`resource:
raw/assets/<file>`, and `finite_asset` with its `content_type`, `size`
and `content_hash: sha256:<hex>`). Leave a note whose resource is a web
address as it is.

Then push it, and check that search finds it:

```sh
fragment sync <its name> --dir .
fragment call <its name> index_status
fragment call <its name> search --input '{"q": "<a word from one of its pages>"}'
```

The old brain's folder permissions don't carry over: everyone the brain
is shared with reads all of it. Invite no one. Your owner reinvites their
teammates once those teammates have moved.

### Sites, as apps

Each `sites/<site>/` is the site's files, as its repository held them.
If its `finite.toml` names a `path`, that folder is the one served; `.`
or none means the whole folder. Leave out `finite.toml` itself.

```sh
fragment home extract <source> sites/<site> --to ~/home-import/home --layout <L>
fragment create <site> --visibility members --template blank --title "<its title>"
mkdir -p ~/home-import/work/sites/<site> && cd ~/home-import/work/sites/<site>
fragment sync <its name> --dir .
rm -rf site && mkdir site && cp -R ~/home-import/home/sites/<site>/<served>/. site/
rm -f site/finite.toml
```

The app's label is the site's name made valid: lowercase letters, digits
and single dashes (never `--`), at most 57 characters.

Before you deploy, find what only worked on Finite Sites, and fix it:
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

Open `site/index.html` with your browser tool at its `file://` path and
see that it renders. Then:

```sh
fragment deploy <its name> --dir .
fragment status <its name>
```

**Public.** Make a site public only if your owner's answers say to keep
it public: `fragment visibility <its name> public`. Then open its
address and check it.

Write each old address with its new one into
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
