---
name: confluence-finite
description: Confluence pages and spaces, and CQL search, through the person's connected Atlassian account, with the bundled helper or Confluence's REST API. No OAuth setup, no API token on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Confluence, Atlassian, Wiki, Pages, Documentation, Knowledge Base]
---

# Confluence

Confluence pages and spaces on your owner's Atlassian sites. Your computer
holds no Atlassian credential: `ATLASSIAN_OAUTH_ACCESS_TOKEN` holds the
connection's placeholder (it names you), it goes as a bearer token to
`api.atlassian.com`, and the computer swaps in a short-lived token for your
owner's account. It is the same connection as Jira's (`jira-finite`): one
Atlassian sign-in. There is no app to make and no API token to paste: never
ask the person for an Atlassian password, an API token or a client secret.

What you do in Confluence is done as your owner: a page you create or change
shows their name in its history.

## Connected?

```bash
CAPI="python3 ${HERMES_SKILL_DIR}/scripts/confluence_api.py"
$CAPI check
```

- `{"status": "connected", "name": …, "email": …, "sites": [{id, name, url, confluence}]}`:
  go on.
- `not_connected`: your owner has not connected Atlassian, or must authorize
  it again (`ATLASSIAN_OAUTH_ACCESS_TOKEN` is then unset). Ask them to
  connect it in the shell's Settings, under Connections, then check again:
  your computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left Atlassian out.
  Say so, and stop.
- `unauthorized`: Atlassian refused the token (the person removed the app's
  access in their Atlassian account, say). Ask them to reconnect Atlassian
  in Settings.

The connection (`atlassian`) is swapped for `api.atlassian.com` only. Every
site's Confluence is there, at `/ex/confluence/{cloudid}/wiki/…`, never at
the site's own host (`acme.atlassian.net/wiki`): the helper finds the cloud
id. It reaches the sites your owner picked at Atlassian's consent, one each
time they connect; Atlassian keeps the earlier ones, so they add a site by
connecting again (Settings → Connections: Disconnect, then Connect) and
picking it. Where there are several, name one with `--site` (its name, its
host or its cloud id).

## Not allowed

Your owner picks, as they connect Atlassian, the services you may use: Jira,
Confluence, or both. Connected without Confluence, `check` says no site has
`confluence`, the helper's commands say `no_site`, and Atlassian refuses a
Confluence call (401 `scope_does_not_match`). Tell your owner, in plain
words, that they didn't allow Confluence, and that they can add it in
Settings → Connections → Reconnect to add. Go on with what they did allow.

## Read only

Your owner picks, as they connect Atlassian, whether you may only read
(pages, spaces, search) or read and write (also create and change pages).
Connected read only, Atlassian refuses every change: 401
`scope_does_not_match` ("Unauthorized; scope does not match"). If a change
is refused so while reads work, tell your owner, in plain words, that they
may have connected Atlassian read only, and that they can allow changes in
Settings → Connections → Allow changes. Do not retry it until they say they
have; go on reading. A 403 `no_permission` is Confluence's own: their
permissions in that space don't allow it.

## The helper

```bash
$CAPI search 'type = page AND text ~ "roadmap" ORDER BY lastmodified DESC' --max 10
$CAPI search 'space = ENG AND title ~ "onboarding"' --cursor <cursor>
$CAPI spaces                                     # each space's id, key and name
$CAPI pages list --space ENG --max 20            # newest change first
$CAPI pages get 123456                           # its body in storage format (XHTML), and its version
$CAPI pages create --space ENG --title "Release notes" --text "What shipped…" --parent 123000
$CAPI pages update 123456 --version 4 --body "<p>…</p>" --message "Fixed the dates"
$CAPI request GET "/wiki/api/v2/pages/123456/footer-comments?limit=20"
```

- A page or a space is named by its id; a space also by its key (`ENG`,
  or `~…` for a person's own).
- `search` takes CQL (Confluence Query Language). It answers 25 at a time;
  pass `cursor` back as `--cursor` for the next, until it is null.
- A body is Confluence's storage format, XHTML (`<p>`, `<h2>`, `<ul><li>`,
  `<a href>`), as `--body`; `--text` takes plain text and makes its blank
  lines paragraphs. `pages get --format view` gives HTML to read.
- **An update names the version it changes.** `pages get` says the page's
  `version`; give it as `--version`, and Confluence refuses the change
  (`conflict`) if someone changed the page since. Without `--version` the
  helper reads the page now and changes that one. A title or body not given
  stays as it is.
- `request` reaches any of the site's Confluence API from `/wiki/api/v2/`
  or `/wiki/rest/api/`; its query goes after `?`, and `--json` is its body.
  Confluence's API docs are at
  https://developer.atlassian.com/cloud/confluence/rest/v2/.

## Output Format

Every command prints JSON. Search results are `{id, type, title, space,
excerpt, lastModified, url}` (`{results, totalSize, cursor}`); spaces
`{id, key, name, type, status, homepageId, url}`; pages `{id, title,
status, spaceId, parentId, version, url}`, `pages get` adding `format` and
`body` (`--raw` gives Confluence's answer as it is). A failure is one JSON
line on stderr, `{error, status, message, hint}`, and exit 1.

## Rules

1. **Confirm before you create or change a page** for your owner, unless
   they asked for exactly that: show its title, where it goes, and what it
   says.
2. **Check the connection before first use** (`$CAPI check`).
3. **Read before you write**: `pages get` the page, change its body, and
   update with the `--version` you read, so you never overwrite someone's
   edit. Keep what the page had that you weren't asked to change.
4. **Never delete** unless asked for that page by name.
5. **Ask for what you need**: a narrow CQL (a space, a type) and `--max`
   keep answers small, and Atlassian limits requests.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Atlassian in Settings → Connections |
| `forbidden` | Your owner left Atlassian off this agent's connections |
| `unauthorized` | Your owner reconnects Atlassian in Settings → Connections |
| `scope_does_not_match` | Connected without Confluence (they add it in Settings → Connections), or, a change refused while reads work, read only (they allow changes there) |
| `no_permission` | Their Confluence permissions don't allow it in that space |
| `not_found`, `no_such_space` | The id or key is wrong, or one they may not see: `$CAPI spaces` |
| `conflict` | The page changed since you read it: get it again and change that version |
| `choose_site` / `no_such_site` | Name a site with `--site`; `$CAPI sites` lists them |
| `no_site` | Confluence wasn't allowed (they add it in Settings → Connections → Reconnect to add), or no site they picked has Confluence |
| `invalid_input` | Confluence says what: often a body that isn't valid storage format |
| `rate_limited` | Wait as the message says, then try once more |
| `redirected` | Atlassian answered a redirect (an attachment's download), which the helper never follows with the token: say so, and stop |
