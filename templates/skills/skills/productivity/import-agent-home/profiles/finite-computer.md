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
- `workspace/finitebrain/<id>.json`: a brain's `name`, and whose it is:
  its `owner` (`agent`, `person`, `other` or `unknown`), and its `kind`
  (`personal`, or `organization` for a team's). A move's stage adds its
  `versions` there too: the old brain's own record of which version of
  each page the copy holds, and which folders the old agent could read.
  `finite-computer.brains.py`, beside this profile, reads it ("Brains, as
  brains"); you never need to.
- `sites/<site>.json`: a site's old address (`url`), whether it was
  public (`visibility`), the folder it served (`path`), and whether it was
  a single-page app (`spa`).

**Name and picture.** Take the old name only if your owner's answers say
to keep it, from `agent/profile.json`. Fetch its picture only if it is
there too; leave it if it doesn't download as an image of at most
256 KiB.

## What its parts become

The survey's `parts` lists them: `brains` (each a folder under
`workspace/finitebrain/`, every brain the old agent could read) and `sites`
(each a folder under `sites/`). Do them after step 3 (files), before
step 4 (personality). Load `brain-finite` before the brains, and
`apps-finite` before the sites.

Then, in every import, do "Sites on finite.site" (below), even when
`parts` lists no sites: the old agent's sites are there, and a move's
stage never holds `sites/`.

**Run each block below as one command, as written.** Your terminal
starts every command in your work folder, and a variable or a `cd` never
carries over to the next: each block sets what it needs and names every
folder whole.

### Brains, as brains

For each brain folder `<id>`: its name is in
`workspace/finitebrain/<id>.json`, or else the first heading of its
`index.md`, or else its id. That file also says whose it is.

**Whose it is.** The old agent kept a working copy of every brain it
could read, not only its own. Make only its own and your owner's here.
A team's brain has one home: two would split what the team writes.
- **The old agent's own, or your owner's** (`owner` is `agent` or
  `person`): make it, as below.
- **A team's, or someone else's** (`other`): don't make it. Someone on
  the team brings it over and invites your owner, and you come in with
  them. Until then, keep a reference copy ("A brain you don't make",
  below). Keep one even if it is here already, in `fragment list`, until
  you have checked it ("Once you are in the real one").
- **Made by an earlier import** (`SKILL.md`, "You may be someone
  already"): don't make it again, and never copy into it by hand: what
  it holds now may be newer than the old copy. Keep a reference copy, and
  bring it up to date from that copy ("Once you are in the real one"),
  which takes only what the old brain changed since.
- **No one can say** (`unknown`, or no `owner`): keep a reference copy
  too, and ask your owner in the report whether it is theirs or a
  team's. Say if it was a team's (`kind`): a team's brain holds what
  teammates wrote too, and no one is added to a new one by itself. If
  their team is bringing it over, you wait for that one. Don't recommend
  either way, and don't decide from its size.

Make the brain once, as `SKILL.md`'s rules say (its full name in
`progress.md` right after):

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

**Record it.** Write down which version of each page the brain now holds,
in its `imported-from-finite.json`, so a teammate who moves later brings
in only what changed since ("Once you are in the real one"):

```sh
W="$HOME/home-import/work/brains/<id>" H="$HOME/home-import/home/workspace/finitebrain/<id>" && \
{ fragment home extract <source> workspace/finitebrain/<id>.json --to "$HOME/home-import/home" --layout <L> || true; } && \
python3 -I "<profiles>/finite-computer.brains.py" start <id> "$H.json" "$W" "$H"
```

It shares only the folders the brain holds now: a folder made later in
the old brain, or one left out, never comes in by a catch-up. It takes
out of the push what would show a folder it doesn't share (`removed`):
the files of one your old agent couldn't read, and the assets a
left-out folder's notes name (they are in its parent's `raw/assets/`).
An asset no note names stops it: ask your owner whose it is. `refused` lists pages `fragment sync`
can't carry (a path over 300 bytes, say): they stay out, so name them in
the report. Running it again before the push is safe. If it says `NO VERSIONS` (an export, a
stage from before versions, or no name file), there is nothing to
record: go on. If it says `STOP`, push nothing, and say in the report
what it said.

Then push it, only if its folder holds nothing but the brain: the old
brain's own top level, `fragment.json` and its record. Anything else
stops it:

```sh
W="$HOME/home-import/work/brains/<id>" H="$HOME/home-import/home/workspace/finitebrain/<id>" && \
if ls "$W" | grep -vxF -e fragment.json -e imported-from-finite.json -f <(ls "$H" | grep -vxF -e AGENTS.md -e _index.md -e .keep -e _wiki); then echo "STOP: $W holds more than the brain: not pushed"; else fragment sync <its name> --dir "$W" && fragment call <its name> index_status; fi
```

If it says STOP, push nothing, and say in the report what it listed.
Otherwise search it once:

```sh
fragment call <its name> search --input '{"q": "<a word from one of its pages>"}'
```

A big brain's index takes a while to take in the sync. If the answer
says `"indexing": true`, don't wait or ask again: its `changed` job
finishes the index by itself from the synced files. Look once at
`fragment runs <its name>`. If its newest `changed` run (the first
listed) is `held` or `blocked`, the index stopped: say so in the report.
Otherwise say this brain's search is still catching up, and go on. If
it isn't indexing and still finds nothing, say that in the report.

The old brain's folder permissions don't carry over: everyone the brain
is shared with reads all of it. Invite no one. Your owner reinvites their
teammates once those teammates have moved.

**A brain you don't make.** Keep its files as a reference copy, in a
folder of its own that outlasts the import, leaving out what the old
brain made for itself:

```sh
C="$HOME/old-finite-brains/<id>" H="$HOME/home-import/home/workspace/finitebrain/<id>" && \
fragment home extract <source> workspace/finitebrain/<id> --to "$HOME/home-import/home" --layout <L> && \
mkdir -p "$C" && \
tar -C "$H" --exclude=AGENTS.md --exclude=_index.md --exclude=.keep --exclude=_wiki --exclude='*/.*' -cf - . | tar -C "$C" -xf - && \
{ fragment home extract <source> workspace/finitebrain/<id>.json --to "$HOME/home-import/home" --layout <L> && cp "$H.json" "$C.json" && python3 -I "<profiles>/finite-computer.brains.py" copied <id> "$C.json" "$H" || echo "no versions: this copy is compared by hand"; } && \
ls "$C" && find "$C" -type f | wc -l
```

The copy's `<id>.json` beside it (`$HOME/old-finite-brains/<id>.json`)
says which version of each page it holds, and which were staged as that
version's text (`copied`, before any asset note is rewritten): keep it
as long as the copy.

In a move, its files are kept whole in your storage too (`SKILL.md`,
step 3). A copy that won't fit your disk's room comfortably, keep none
of: its note under `~/old-agent/home/workspace/finitebrain/` brings it
back when you need it, and the README line says so.

Bring its assets in as above, into the copy. Then mark what it is:
- In `$HOME/old-finite-brains/README.md`, a line for it: its name, whose
  it was, and that it is a copy from finite.computer (the old Finite),
  to read until the real one holds what it does.
- In one memory, for all of them: that reference copies of brains from
  the old Finite are in that folder, and each goes only once its real
  one holds what the copy does.

Read it when it helps, and never write to it: what the team writes from
now on goes in the real one.

**Once you are in the real one** (it is in `fragment list`), bring it up
to date from your copy before you drop anything. Being listed says you
can read it, not that it holds what your copy does: a teammate may have
brought over an older copy, or part of one, and the team kept writing in
the old brain after. Your copy is how the old brain was when you left
it. Sync the real one into a folder of its own, catch it up, and push
it:

```sh
W="$HOME/old-finite-brains/<id>.real" C="$HOME/old-finite-brains/<id>" && \
mkdir -p "$W" && fragment sync <its name> --dir "$W" && \
python3 -I "<profiles>/finite-computer.brains.py" catch-up <id> "$C.json" "$W" "$C" && \
fragment sync <its name> --dir "$W" && fragment call <its name> index_status
```

Run it, then once more: the second run takes in a teammate catching it
up at the same moment, and otherwise changes nothing. Its answer is the
list below.

It changes only what is safe to change, page by page, by the old brain's
own versions, and only in the folders the real one shares. It brings in:
- **added:** pages the real one doesn't have yet;
- **updated:** pages the old brain changed after the real one's copy was
  taken, which nobody here has changed since;
- **assets:** the files those pages' frontmatter names, when the real one
  lacks them.

It leaves the rest as it is, for a person, and lists it:
- **both:** pages changed or deleted here and changed in the old brain
  since, another page the old brain moved onto a page's path, or a page
  whose asset differs here;
- **gone:** pages the old brain deleted since. It deletes nothing;
- **unsynced:** pages your old agent edited and never synced: the old
  brain never had that text;
- **missing:** pages your copy should hold and doesn't;
- **older:** pages in a folder the real one took from a newer copy than
  yours, which it doesn't hold as yours does (a page the old brain
  deleted since, say);
- **extra:** files your copy holds that the old brain didn't list (a
  page made and never synced, say): only the copy has them;
- **unshared:** folders the real one doesn't share: left out when it
  was made, for only some to read, or made since in the old brain (even
  at a shared folder's path). Never bring one in yourself;
- **refused:** a page's path that climbs out of the brain, goes through
  a link, or is one `fragment sync` never carries (over 300 bytes, a
  name starting with `.`, or one holding `.conflict-`).

It never changes **unread folders** (folders you couldn't read on the old
Finite, so your copy has none of them).

- **`settled` is true:** the real one holds what your copy does. Delete
  the copy, its line in the README, and the memory once no copy is left:

  ```sh
  rm -rf -- "$HOME/old-finite-brains/<id>" "$HOME/old-finite-brains/<id>.json" "$HOME/old-finite-brains/<id>.real"
  ```
- **`settled` is false:** keep the copy. In the report (step 8), list
  what it left for your owner and ask them, or the brain's owner, what
  to do with each: never pick yourself. Say in the README line what is
  left, and why.
- **It says `NO VERSIONS` or `NO RECORD`** (a copy from before versions,
  or a real one made without a record): compare by hand. Compare its
  pages and assets with the copy's, and read a few of each. If it holds
  all of it, the copy goes as above. If some are missing or differ, keep
  only those in the copy, note which in the report, and suggest someone
  on the team adds them. If you can't tell, keep the copy and ask your
  owner in the report.
- **It says `STOP`:** the real one's record is another brain's, or not
  one the catch-up wrote. Change nothing, and say so in the report.

Every teammate who moves later does the same, so the real one comes
together one move at a time. The last to move leaves it holding all of
the old brain, or a list of what still needs a person.

**If no one brings it over**, and your owner runs that team, they may
ask you to set it up. Before you make it, ask your owner two things, and
wait for each answer:
1. **Has anyone invited you to it already?** A teammate may have brought
   it over: two would split what the team writes. If one has, your owner
   accepts that invite, and you catch it up as above instead.
2. **Who should read which folders?** On the old Finite a folder could be
   kept to some of the team; here everyone a brain is shared with reads
   all of it. Name its folders (`ls "$HOME/old-finite-brains/<id>"`), and
   ask which, if any, only some should read. Leave those out of the new
   brain (each can be a brain of its own, made the same way, keeping
   just that folder, for those people), and never copy from a narrower
   brain into a wider one.

Then make it as above, from the copy: the same steps, with `H` the
copy's folder (`$HOME/old-finite-brains/<id>`, so its name file is
`$H.json`) and no `fragment home extract`. Before you record it, remove
each folder you leave out from its work folder (`rm -rf -- "$W/<folder>"`):
the record shares only the folders the brain holds, so no teammate's
catch-up brings another in. Leave the copy as it is. Then ask your owner for their teammates'
emails, and invite each as an editor, so their agents come in with them
and bring in what changed when they move:

```sh
fragment members add <its name> <email> --role editor
```

Each invite waits for its person's yes. Then the copy goes, as in "Once
you are in the real one", but for a folder you left out: keep that until
a brain of its own holds it. Do the same for a brain your owner says is
theirs, but invite only whom they name.

### Sites, as apps

Each `sites/<site>/` is the site's files, as its repository held them.
One whose old address `site-map.json` names came over already: skip it.
Copy each other out first:

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
in every import, after the zip's sites, and when the zip has none: a
move's stage never does, so in a move every site comes from here. Do it
alone when your owner asks later for their old sites, or to point an
old address (`SKILL.md`, step 0).

**The key.** finite.site knows the old agent by its Nostr key. A move
puts that key in your owner's Vault for you, and it never comes to you:
each request you send to finite.site carries its placeholder, and your
computer has the Vault sign it on the way. You never need the key
itself, nor the old `fsite` tool: every step here, pointing an old
address too, works with the placeholder alone. Find the placeholder:

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

Then save its header in a file, once:

```sh
ph="<the placeholder>" && mkdir -p "$HOME/home-import" && \
printf '%s: Nostr %s\n' Authorization "$ph" > "$HOME/home-import/finite-site.auth" && echo saved
```

Every request below sends it from that file
(`-H @"$HOME/home-import/finite-site.auth"`). Never write the header
into a command yourself: your tools show a credential header as `***`,
and a header copied from what they show goes out unsigned.

**Its sites.** List them:

```sh
curl -sS -o "$HOME/home-import/finite-site.json" -w 'finite.site: %{http_code}\n' -H @"$HOME/home-import/finite-site.auth" "https://finite.site/api/v2/projects" && \
jq -c '.projects[] | select(.role == "owner" and .site != null) | {slug, site: .site.name, url: .site.url, status: .site.status, visibility: .site.visibility, branch: .site.branch, spa: .site.spa, redirect_to: .site.redirect_to}' "$HOME/home-import/finite-site.json"
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
- **In an import:** every site listed whose slug the zip didn't hold,
  and that `site-map.json` doesn't name (it came over before).
  For one it held, the listing's `url` is its old address: put that in
  `site-map.json` in place of any other for it.
- **Alone:** the ones your owner named, else every one. A site that
  `site-map.json` names (its `url`, with or without the last `/`), or
  that `progress.md` says is deployed, came over before: make nothing
  for it again, and only point its old address, if your owner now makes
  it public. One `progress.md` names but not as deployed was cut short:
  go on from its first step not done, with the app named there. Asked
  only to point an old address, bring nothing over: point only a site
  that came over before, and name any other in your message.

**Bring one over.** finite.site gives a login for its repository alone:
it is used for this one clone, into `/tmp`, then deleted. Its files then
come into your home as a zip's do, through `fragment home extract`,
which leaves out every file whose name says it may hold a password or
key:

```sh
slug="<its slug>" branch="<its branch>" && \
case "$slug" in "" | *[!a-z0-9-]*) echo "STOP: not a plain slug" ;; *) \
  t="/tmp/finite-site-$FRAGMENT_AS_AGENT-$slug" H="$HOME/home-import/home/sites/$slug" && \
  rm -rf -- "$t" "$H" && mkdir -p "$t/src/sites" && chmod 700 "$t" && \
  curl -fsS -X POST -H @"$HOME/home-import/finite-site.auth" -H "content-type: application/json" -d '{}' -o "$t/auth.json" "https://finite.site/api/v2/projects/$slug/git-auth" && \
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
and only once its new address serves every page the old one did:
pointing it sends each old page to the same path at the new address, so
a page the new one can't serve becomes a broken link for everyone who
kept it. `<its slug>` is the one `finite-site.json` lists beside the
site's old address (its `url`), and `<its new address>` is `https://`
and its host, with nothing after (`https://garden--k3x9.finite.place`).

- **A single-page app** (its listing's `spa` is true, or you moved its
  routes to `#/…` above) is never pointed: its old `/about` would land
  on a page this platform doesn't serve. Its old address keeps serving.
  Say so in the report.
- **Any other site:** first check its pages at the new address. Name up
  to five the old site served besides `/`: its folders with an
  `index.html` (`/blog/`), its other `.html` files (`/about.html`), and
  paths its own links use. Give one of them a query string:

```sh
to="<its new address>" && \
for p in "/" "<a page>" "<a page>" "<a page>?from=old"; do curl -sS -o /dev/null -w "%{http_code} $p\n" "$to$p"; done
```

Every line must say `200`. If one doesn't, don't point it: its old
address keeps serving, and the report names the pages that failed.
Otherwise:

```sh
slug="<its slug>" to="<its new address>" && \
curl -sS -w '\nfinite.site: %{http_code}\n' -X POST -H @"$HOME/home-import/finite-site.auth" -H "content-type: application/json" -d "$(jq -cn --arg to "$to" '{to: $to}')" "https://finite.site/api/v2/projects/$slug/site/redirect"
```

A 200 answers `redirect_to`, the new address. Check it: `curl -sS -o
/dev/null -w '%{http_code} %{redirect_url}\n' "<its url>"` says `302`
and the new address. Every page of the old address now goes to the
same page at the new one. A site kept private gets no redirect: its old
address keeps serving as it did, to those it was shared with. In a move,
every site stays private, so none is pointed until your owner makes one
public here and asks you to point it. With no key (above),
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
- **Team brains:** for each one you kept a reference copy of, that it is
  their team's brain (or whose it is), and that you will join the real
  one once someone on the team brings it over and invites them. Ask
  whether anyone is: if every teammate's agent just waited, no one
  would. If no one is, and they run that team, you can set it up from
  the copy, and they invite the others.
- **Brains no one could place:** ask whether each is theirs or a team's
  ("Whose it is").
- **Old sites:** one made public here sends its old address's visitors
  to its new one. The others keep serving at their old addresses until
  they are turned off. Give them the new addresses. For one kept private
  (in a move, every one), their next step: make it public here, then ask
  you to point its old address at the new one. Not for a single-page
  app, which is never pointed, nor when finite.site had no key for you.
