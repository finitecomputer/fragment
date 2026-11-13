---
name: jira-finite
description: Jira issues, comments, transitions and projects through the person's connected Atlassian account, with the bundled helper or Jira's REST API. No OAuth setup, no API token on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Jira, Atlassian, Issues, Tickets, Projects, Project Management]
---

# Jira

Jira issues, comments, transitions and projects on your owner's Atlassian
sites. Your computer holds no Atlassian credential:
`ATLASSIAN_OAUTH_ACCESS_TOKEN` holds the connection's placeholder (it names
you), it goes as a bearer token to `api.atlassian.com`, and the computer
swaps in a short-lived token for your owner's account. There is no app to
make and no API token to paste: never ask the person for an Atlassian
password, an API token or a client secret, and never run `acli auth login`.

What you do in Jira is done as your owner: an issue you create or a comment
you write shows their name.

## Connected?

```bash
JAPI="python3 ${HERMES_SKILL_DIR}/scripts/jira_api.py"
$JAPI check
```

- `{"status": "connected", "name": …, "email": …, "sites": [{id, name, url, jira}]}`:
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
site's Jira is there, at `/ex/jira/{cloudid}/rest/…`, never at the site's
own host (`acme.atlassian.net`): the helper finds the cloud id. It reaches
the sites your owner picked at Atlassian's consent, one each time they
connect; Atlassian keeps the earlier ones, so they add a site by connecting
again (Settings → Connections: Disconnect, then Connect) and picking it.
Where there are several, name one with `--site` (its name, its host or its
cloud id).

## Read only

Your owner picks, as they connect Atlassian, whether you may only read
(issues, comments, projects, people) or read and write (also create issues,
comment and move them on their workflow). Connected read only, Atlassian
refuses every change: 401 `scope_does_not_match` ("Unauthorized; scope does
not match"). If a change is refused so while reads work, tell your owner,
in plain words, that they may have connected Atlassian read only, and that
they can allow changes in Settings → Connections → Allow changes. Do not
retry it until they say they have; go on reading. A 403 `no_permission` is
Jira's own: their Jira permissions don't allow it in that project.

## The helper

```bash
$JAPI sites                                                 # each site's cloud id, name, url and scopes
$JAPI search 'assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC' --max 20
$JAPI search 'project = ENG AND text ~ "login"' --page <nextPageToken>
$JAPI issue get ENG-123                                     # with its description and comments as text
$JAPI issue create --project ENG --type Bug --summary "Login fails" --description "Steps…" --assign-me
$JAPI issue comment ENG-123 --body "Fixed in #42."
$JAPI transitions ENG-123                                   # where it may go now
$JAPI transition ENG-123 --to Done --comment "Shipped."
$JAPI projects --query eng
$JAPI users "ada"                                           # account ids, for --assignee
$JAPI request PUT /rest/api/3/issue/ENG-123 --json '{"fields": {"summary": "…"}}'
```

- An issue is named by its key (`ENG-123`) or its id; a project by its key
  (`ENG`) or id.
- `search` takes JQL, and Jira asks for a bounded query: one that names
  something to search by (`project = ENG`, `assignee = currentUser()`), not
  `ORDER BY` alone. It answers 25 at a time; pass `nextPageToken` back as
  `--page` for the next (`isLast` says when there are no more).
- Descriptions and comments are Atlassian Document Format (ADF): the helper
  makes your text one (blank lines between paragraphs). For headings, lists
  or mentions, give an ADF document as JSON with `--adf`.
- `transition --to` matches the transition's name or the status it goes to,
  ignoring case; `transitions` lists them.
- `request` reaches any of the site's Jira REST API from `/rest/`
  (`/rest/api/3/…`, `/rest/agile/1.0/board`); its query goes after `?`, and
  `--json` is its body. Jira's API docs are at
  https://developer.atlassian.com/cloud/jira/platform/rest/v3/.

## Output Format

Every command prints JSON. Issues are `{key, id, summary, status, type,
priority, assignee: {displayName, accountId}, updated, url}`; `issue get`
adds `reporter`, `project`, `labels`, `created`, `duedate`, `parent`,
`description` and `comments` (their text; `--raw` gives Jira's answer as it
is); `search` answers `{issues, nextPageToken, isLast}`; `issue create`
answers `{key, id, url}`. A failure is one JSON line on stderr, `{error,
status, message, hint}`, and exit 1.

## Rules

1. **Confirm before you create, comment, transition or reassign** for your
   owner, unless they asked for exactly that: show what you will write, and
   where.
2. **Check the connection before first use** (`$JAPI check`).
3. **Read before you write**: find the issue (`search`, `issue get`) and its
   transitions before moving it, so a change lands on the right one.
4. **Never delete** unless asked for that issue by key.
5. **Ask for what you need**: a bounded JQL, `--max` and `--fields` keep
   answers small, and Atlassian limits requests.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Atlassian in Settings → Connections |
| `forbidden` | Your owner left Atlassian off this agent's connections |
| `unauthorized` | Your owner reconnects Atlassian in Settings → Connections |
| `scope_does_not_match` | A change refused while reads work: connected read only (they allow changes in Settings → Connections) |
| `no_permission` | Their Jira permissions don't allow it in that project |
| `not_found` | The key is wrong, or the issue is one they may not see |
| `choose_site` / `no_such_site` | Name a site with `--site`; `$JAPI sites` lists them |
| `no_site` | No Jira site was granted: they connect Atlassian again and pick one |
| `invalid_input` | Jira says which field: an issue type, priority or field the project doesn't have |
| `rate_limited` | Wait as the message says, then try once more |
| `redirected` | Atlassian answered a redirect (an attachment's content), which the helper never follows with the token: say so, and stop |
