# Profile: finite.computer

Read this with `SKILL.md` when the layout is `finite-computer`: an agent
your owner kept on finite.computer (the old Finite), exported as a zip of
its home. Its layout file is `finite-computer.layout.json` beside this
one.

You only read the copy. The old agent and its sites keep running on the
old Finite until your owner turns them off, but for one change: a site
your owner makes public here sends its old address on to its new one
("Sites on finite.site").

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
Copy it out first:

```sh
fragment home extract <source> sites/<site> --to "$HOME/home-import/home" --layout <L>
```

A site the zip doesn't hold may still be on finite.site: once the zip's
are done, see "Sites on finite.site" below. It puts each one where this
puts a zip's, and the steps from here on are the same for both.

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
sites public, or, when they asked for their old sites alone, they named
it to make public: `fragment visibility <its name> public`. Then open its
address and check it, and point its old address at it ("Sites on
finite.site", below). With no `.json`, keep it private, and name it in
the report so your owner can make it public.

Write each old address (`sites/<site>.json`'s `url`, else
`https://<site>.finite.site`) with its new one into
`~/home-import/site-map.json` (`{"https://garden.finite.site":
"https://garden--k3x9.finite.place"}`). Steps 4 and 5 use it.

### Sites on finite.site

The old agent's sites are also on finite.site, each a git repository it
owned. From there, bring over each site the zip didn't, and send a site's
old address on to its new one once your owner makes it public. Do this
after the zip's sites, or alone when your owner asks for their old sites
later (`SKILL.md`, step 0).

**The key.** finite.site knows the old agent by its Nostr key. A move
puts that key in your owner's Vault for you, and it never comes to you:
each request you send to finite.site carries its placeholder, and your
computer has the Vault sign it on the way. Find the placeholder:

```sh
curl -fsS http://api.fragment.internal/api/computer | jq -r --arg a "$FRAGMENT_AS_AGENT" '.agents[] | select(.fragment == $a) | .credentials[] | select((.provider | startswith("nostr-")) and (.hosts | index("finite.site"))) | "\(.provider) \(.placeholder)"'
```

- **One line:** its second word is `<the placeholder>` below.
- **Several:** use the one named after you: `nostr-` and your
  fragment's name (`$FRAGMENT_AS_AGENT`), or its start.
- **None:** the key isn't here for you, or it is kept but sent nowhere.
  Stop this part. Send nothing to finite.site, never look for the key, and
  never ask your owner for it. Tell them, in your message or the report:
  "I can't reach your old sites on finite.site yet: your old agent's key
  has to be set up there for me first. Ask Finite's team, then ask me
  again." The rest of an import goes on.

**Its sites.** List them:

```sh
ph="<the placeholder>" && mkdir -p "$HOME/home-import" && \
curl -sS -o "$HOME/home-import/finite-site.json" -w 'finite.site: %{http_code}\n' -H "Authorization: Nostr $ph" "https://finite.site/api/v2/projects" && \
jq -c '.projects[] | select(.role == "owner" and .site != null) | {slug, site: .site.name, url: .site.url, status: .site.status, visibility: .site.visibility, branch: .site.branch, redirect_to: .site.redirect_to}' "$HOME/home-import/finite-site.json"
```

- **200:** each line is a site the old agent owned. Leave out one whose
  `status` isn't `published` (nothing is served there) and one whose
  `redirect_to` is set (it moved already). None left: say the old agent
  had no other sites on finite.site. But when the zip held sites and none
  is listed at all, finite.site doesn't know this key as the old agent's:
  stop this part as with no key.
- **401, or another 4xx:** finite.site didn't take the signature. Stop
  this part as with no key, and name the status in the report.
- **5xx, or no answer:** wait a minute and run it again, up to five
  times. Still failing: say finite.site didn't answer, and stop this part.

**Which.** A site's folder in the zip is its slug (`sites/<slug>/`).
- **In an import:** every site listed whose slug the zip didn't hold.
  For one it held, the listing's `url` is its old address: put that in
  `site-map.json` in place of any other for it.
- **Alone:** the ones your owner named, else every one. A site that
  `site-map.json` names (its `url`, with or without the last `/`), or
  that `progress.md` says is deployed, came over before: make nothing
  for it again, and only point its old address, if your owner now makes
  it public. One `progress.md` names but not as deployed was cut short:
  go on from its first step not done, with the app named there.

**Bring one over.** finite.site gives a login for its repository alone:
it is used for this one clone, into `/tmp`, then deleted. Its files then
come into your home as a zip's do, through `fragment home extract`,
which leaves out every file whose name says it may hold a password or
key:

```sh
ph="<the placeholder>" slug="<its slug>" branch="<its branch>" && \
case "$slug" in "" | *[!a-z0-9-]*) echo "STOP: not a plain slug" ;; *) \
  t="/tmp/finite-site-$FRAGMENT_AS_AGENT-$slug" H="$HOME/home-import/home/sites/$slug" && \
  rm -rf -- "$t" "$H" && mkdir -p "$t/src/sites" && chmod 700 "$t" && \
  curl -fsS -X POST -H "Authorization: Nostr $ph" -H "content-type: application/json" -d '{}' -o "$t/auth.json" "https://finite.site/api/v2/projects/$slug/git-auth" && \
  jq -r '"https://\(.username | @uri):\(.password | @uri)@finite.site"' "$t/auth.json" > "$t/login" && \
  git -c credential.helper= -c credential.helper="store --file=$t/login" clone --quiet --depth 1 --branch "$branch" "https://finite.site/$slug.git" "$t/src/sites/$slug"; \
  ok=$?; rm -f -- "$t/auth.json" "$t/login"; rm -rf -- "$t/src/sites/$slug/.git"; \
  [ "$ok" = 0 ] && fragment home extract "$t/src" "sites/$slug" --to "$HOME/home-import/home" --layout hermes && \
  jq -c --arg slug "$slug" '.projects[] | select(.slug == $slug) | .site | {url, visibility, path, spa}' "$HOME/home-import/finite-site.json" > "$H.json" && \
  echo "left out unread:" && comm -23 <(find "$t/src/sites/$slug" -type f -printf '%P\n' | sort) <(find "$H" -type f -printf '%P\n' | sort); \
  ok=$?; rm -rf -- "$t"; \
  if [ "$ok" = 0 ]; then ls -A "$H"; else echo "FAILED: $slug did not come over whole"; fi ;; \
esac
```

It lists what it left out unread, then the site's files. Name each left
out in the report, as `SKILL.md` says of `source.skipped`, and never go
looking for them. If it says STOP or FAILED, bring that site over no
further, and say so in the report. Otherwise go on from "Make each app
once" above, `<site>` its slug and its title the listing's `site`.
Write each step in `progress.md` beside its slug as it is done: its
files came, its app made (its full name), deployed. Only a deployed site
has come over.

**Point the old address.** Only for a site your owner made public here,
once its new address answers. `<its slug>` is the one `finite-site.json`
lists beside the site's old address (its `url`), and `<its new address>`
is `https://` and its host, with nothing after
(`https://garden--k3x9.finite.place`):

```sh
ph="<the placeholder>" slug="<its slug>" to="<its new address>" && \
curl -sS -w '\nfinite.site: %{http_code}\n' -X POST -H "Authorization: Nostr $ph" -H "content-type: application/json" -d "$(jq -cn --arg to "$to" '{to: $to}')" "https://finite.site/api/v2/projects/$slug/site/redirect"
```

A 200 answers `redirect_to`, the new address. Check it: `curl -sS -o
/dev/null -w '%{http_code} %{redirect_url}\n' "<its url>"` says `302`
and the new address. Every page of the old address now goes to the
same page at the new one. A site kept private gets no redirect: its old
address keeps serving as it did, to those it was shared with. In a move,
every site stays private, so none is pointed. With no key (above),
nothing is pointed: say so, and that the old address still serves.

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
- **Old sites:** one made public here sends its old address's visitors
  to its new one. The others keep serving at their old addresses until
  they are turned off. Give them the new addresses.
