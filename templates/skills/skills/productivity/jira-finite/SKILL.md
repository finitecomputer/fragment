---
name: jira-finite
description: Jira issues, comments, transitions and projects through the person's connected Atlassian account, with `fragment api atlassian` and Jira's REST API. No OAuth setup, no API token on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Jira, Atlassian, Issues, Tickets, Projects, Project Management]
  fragment:
    connection: atlassian
---

# Jira

Jira issues, comments, transitions and projects on your owner's Atlassian
sites. Your computer holds no Atlassian credential: `fragment api
atlassian` sends the connection's placeholder for you, and the computer
swaps in a short-lived token for your owner's account. There is no app to
make and no API token to paste: never ask the person for an Atlassian
password, an API token or a client secret, and never run `acli auth login`.

What you do in Jira is done as your owner: an issue you create or a comment
you write shows their name.

## First: the site's cloud id

The token goes to `api.atlassian.com` alone. Every site's Jira is there,
under its cloud id, never at the site's own host (`acme.atlassian.net`).
Learn the cloud id once, and use it for every call after:

```bash
fragment api atlassian GET https://api.atlassian.com/oauth/token/accessible-resources
# → [{"id": "1324a887-45db-1bf4-1e99-ef0ff456d421", "name": "acme", "url": "https://acme.atlassian.net", "scopes": ["read:jira-work", …]}]
J="https://api.atlassian.com/ex/jira/1324a887-45db-1bf4-1e99-ef0ff456d421/rest/api/3"
```

- A site: connected, go on. One site may be listed once for each product:
  Jira's is the entry whose `scopes` name `jira`.
- Several sites: use the one your owner named (by `name` or `url`), or ask.
  They picked each at Atlassian's consent, one each time they connected;
  they add a site by connecting again (Settings → Connections: Disconnect,
  then Connect) and picking it.
- `[]`: no site was granted. They connect Atlassian again and pick one.
- `not_connected`: your owner has not connected Atlassian, or must
  authorize it again. Ask them to connect it in Settings, under
  Connections, then try again: your computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left Atlassian
  out, or the call went to another host. Say so, and stop.
- 401 `Unauthorized` on this read: Atlassian refused the token (the person
  removed the app's access, say). Ask them to reconnect Atlassian.

The answer is Jira's JSON on stdout. A refusal is one line of JSON on
stderr, `{error, status, message, hint}`, and exit 1. Atlassian's and
Jira's own refusals have no code worth reading: go by `status`, and by
`message`, which holds Jira's `errorMessages` and `errors` (the field, and
what is wrong with it). An issue's link for people is
`<the site's url>/browse/ENG-123`.

## Read only

Your owner picks, as they connect Atlassian, whether you may only read
(issues, comments, projects, people) or read and write (also create issues,
comment and move them on their workflow). Connected read only, Atlassian
refuses every change with a 401 whose message is `Unauthorized; scope does
not match`. That is not a dead token, whatever the hint says: do not ask
for a reconnect. If a change is refused so while reads work, tell your
owner, in plain words, that they may have connected Atlassian read only,
and that they can allow changes in Settings → Connections → Allow changes.
Do not retry it until they say they have; go on reading. A 403 is Jira's
own: their Jira permissions don't allow it in that project.

## Reading

```bash
# search by JQL: name the fields, or Jira answers ids alone
fragment api atlassian POST "$J/search/jql" --body '{"jql": "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC",
  "maxResults": 25, "fields": ["summary", "status", "assignee", "priority", "issuetype", "updated"]}'

# one issue, with its description and comments
fragment api atlassian GET "$J/issue/ENG-123" --param fields=summary,status,assignee,reporter,priority,issuetype,project,labels,created,updated,duedate,parent,description,comment

# where an issue may go now: each transition's id, name and the status it goes to
fragment api atlassian GET "$J/issue/ENG-123/transitions"

# projects, and people (their accountId, to assign)
fragment api atlassian GET "$J/project/search" --param query=eng --param maxResults=50 --param orderBy=name
fragment api atlassian GET "$J/user/search" --param query=ada --param maxResults=10

# your owner: {account_id, name, email}
fragment api atlassian GET https://api.atlassian.com/me
```

- An issue is named by its key (`ENG-123`) or its id; a project by its key
  (`ENG`) or id.
- JQL must be bounded: it names something to search by (`project = ENG`,
  `assignee = currentUser()`), not `ORDER BY` alone. Inside the JSON body,
  write its strings with single quotes (`text ~ 'login'`).
- `search/jql` answers `{issues, nextPageToken, isLast}`, at most 100 at a
  time: send `"nextPageToken"` back in the body for the next page, until
  `isLast`. Other lists (`project/search`, an issue's comments) page by
  `startAt` and `maxResults`, and say `isLast` or `total`.
- Descriptions and comment bodies answer as Atlassian Document Format
  (ADF), a tree of nodes: the words are its `text` nodes.

## Changing

A description or a comment is written as ADF: a `doc` of paragraphs, a
`{"type": "hardBreak"}` between a paragraph's lines.

```bash
# create (type: Task, Bug, Story, Epic, Subtask; a subtask or an epic's child names "parent": {"key": "ENG-1"})
fragment api atlassian POST "$J/issue" --body '{"fields": {"project": {"key": "ENG"}, "issuetype": {"name": "Bug"}, "summary": "Login fails",
  "description": {"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Steps…"}]}]},
  "priority": {"name": "High"}, "labels": ["login"], "assignee": {"accountId": "<account id>"}}}'

# comment
fragment api atlassian POST "$J/issue/ENG-123/comment" --body '{"body": {"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Fixed in #42."}]}]}}'

# move it on its workflow, by a transition's id (read them first): answers nothing (204)
fragment api atlassian POST "$J/issue/ENG-123/transitions" --body '{"transition": {"id": "31"}}'

# change fields: answers nothing (204)
fragment api atlassian PUT "$J/issue/ENG-123" --body '{"fields": {"summary": "Login fails on Safari"}}'
```

A long body is easier as a file: `--body @issue.json`. Anything else of the
site's Jira is the same call with another path under
`/ex/jira/<cloud id>/rest/` (`/rest/agile/1.0/board`). Jira's API docs are
at https://developer.atlassian.com/cloud/jira/platform/rest/v3/.

## Rules

1. **Confirm before you create, comment, transition or reassign** for your
   owner, unless they asked for exactly that: show what you will write, and
   where.
2. **Read before you write**: find the issue and its transitions before
   moving it, so a change lands on the right one.
3. **Never delete** unless asked for that issue by key.
4. **Ask for what you need**: a bounded JQL, `maxResults` and `fields` keep
   answers small, and Atlassian limits requests (429): wait a minute, then
   try once more.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Atlassian in Settings → Connections |
| `forbidden` | Your owner left Atlassian off this agent's connections, or the call went to a host that is not `api.atlassian.com` |
| 401 `Unauthorized`, reads too | Your owner reconnects Atlassian in Settings → Connections |
| 401 `scope does not match` | A change refused while reads work: connected read only (they allow changes in Settings → Connections). On a read: the cloud id is not a site they picked |
| 403 | Their Jira permissions don't allow it in that project |
| 404 | The key is wrong, or the issue is one they may not see |
| 400 | The message says which field: an issue type, priority or field the project doesn't have, or a JQL that is not bounded |
| `redirected` | Atlassian answered a redirect (an attachment's content), which is never followed with the token: say so, and stop |
