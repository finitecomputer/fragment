---
name: linear-finite
description: Linear issues, comments, teams and projects through the person's connected Linear account, with `fragment api linear` and Linear's GraphQL API. No OAuth setup, no API key on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Linear, Issues, Tickets, Projects, Project Management]
  fragment:
    connection: linear
---

# Linear

Linear issues, comments, teams and projects through your owner's connected
Linear account. Your computer holds no Linear credential: `fragment api
linear` sends the connection's placeholder for you, and the computer swaps
in a short-lived token for your owner's account. There is no OAuth app to
make and no API key to paste: never ask the person for a Linear key, and
never set `LINEAR_API_KEY`.

What you do in Linear is done as your owner: an issue you create or a
comment you write shows their name.

## The one call

Linear is one GraphQL endpoint. Every read and change is this command with
another document:

```bash
L="fragment api linear POST https://api.linear.app/graphql"
$L --query 'query { viewer { name email } organization { name urlKey } }'
```

The answer is Linear's JSON on stdout (`{"data": …}`): a viewer means
connected. Pass values as `--variables '{…}'`, never pasted into the
document.

A refusal is one line of JSON on stderr and exit 1. Do what its `hint`
says; its `body` is Linear's own answer. A GraphQL answer that names
errors is a refusal too (`graphql_errors`): `message` is the first
error's, and each error's `extensions` in `body` says more (`code`, and
`userPresentableMessage`, written for a person).

The connection reaches `api.linear.app` (the GraphQL API) and
`uploads.linear.app` (files attached to issues) only. It is one Linear
workspace: the one your owner picked when they connected it.

On an older computer: `fragment api` came with an image newer than a
computer awake since before it. If the command is `unrecognized`, your
computer takes that image when it next restarts. Until then, send the same
calls with `curl -H "Authorization: Bearer $LINEAR_OAUTH_ACCESS_TOKEN"`: the computer swaps
that the same way.

## Read only

Your owner picks, as they connect Linear, whether you may only read or
read and write (create and change issues, comment). Connected read only,
Linear refuses every change: an error from Linear whose message mentions
`write` or a missing scope, while reads work. Tell your owner, in plain
words, that they may have connected Linear read only, and that they can
allow changes in Settings → Connections → Allow changes. Do not retry it
until they say they have; go on reading.

## Reading

Ask for the fields you need, and `first:` no more than 50 at a time.

```bash
ISSUE='identifier title url priorityLabel updatedAt state { name } team { key } assignee { name email }'

# teams: each one's id, key (ENG) and name
$L --query 'query { teams(first: 100) { nodes { id key name } } }'

# full-text search
$L --query "query(\$term: String!) { searchIssues(term: \$term, first: 10) { nodes { $ISSUE } } }" --variables '{"term": "login bug"}'

# issues by filter, newest change first: mine, a team's, a state's
$L --query "query(\$filter: IssueFilter) { issues(filter: \$filter, first: 25, orderBy: updatedAt) { nodes { $ISSUE } } }" \
  --variables '{"filter": {"assignee": {"isMe": {"eq": true}}, "team": {"key": {"eqIgnoreCase": "ENG"}}, "state": {"name": {"eqIgnoreCase": "In Progress"}}}}'

# one issue, with its description, labels and comments
$L --query "query(\$id: String!) { issue(id: \$id) { id $ISSUE description dueDate project { name } labels { nodes { name } } comments(first: 50) { nodes { body createdAt user { name } } } } }" --variables '{"id": "ENG-123"}'

# projects
$L --query 'query { projects(first: 25, orderBy: updatedAt) { nodes { id name status { name } url targetDate lead { name } } } }'
```

- An issue is named by its identifier (`ENG-123`) or its id, wherever an
  `id` is asked.
- Leave a filter's key out to not filter by it.
- Priority: 0 none, 1 urgent, 2 high, 3 medium, 4 low.
- Descriptions and comments are Markdown.

## Changing

A change names ids, not words, so look them up first.

```bash
# a team's id, by its key
$L --query 'query($key: String!) { teams(filter: { key: { eqIgnoreCase: $key } }) { nodes { id } } }' --variables '{"key": "ENG"}'

# create (assigneeId: the viewer's id, to assign it to your owner)
$L --query 'mutation($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { identifier url } } }' \
  --variables '{"input": {"teamId": "<team id>", "title": "Fix login", "description": "Steps…", "priority": 2}}'

# the workflow states of an issue's team, to move it
$L --query 'query($id: String!) { issue(id: $id) { team { states { nodes { id name } } } } }' --variables '{"id": "ENG-123"}'

# update: any of title, description, priority, stateId, assigneeId
$L --query 'mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success issue { identifier state { name } } } }' \
  --variables '{"id": "ENG-123", "input": {"stateId": "<state id>"}}'

# comment (issueId is the issue's id, not its identifier: read it first)
$L --query 'mutation($input: CommentCreateInput!) { commentCreate(input: $input) { success comment { id url } } }' \
  --variables '{"input": {"issueId": "<issue id>", "body": "Fixed in #42."}}'
```

Anything else (cycles, labels, attachments, project updates) is another
document on the same call. Linear's API docs are at
https://linear.app/developers, and the schema answers introspection.

## Rules

1. **Confirm before you create, close, reassign or comment** for your
   owner, unless they asked for exactly that: show what you will write, and
   where.
2. **Read before you write**: find the issue and its team's states before
   changing it, so an update lands on the right one.
3. **Respect rate limits**: Linear limits requests per hour, and search to
   30 a minute. Ask for what you need in one query rather than many.

## Troubleshooting

| Linear's `extensions.code`, or its message | Fix |
|---------|-----|
| `AUTHENTICATION_ERROR`, or a 401 | Linear refused the token (the person revoked the app, say): your owner reconnects Linear in Settings → Connections |
| `INVALID_INPUT`, `Entity not found` | The identifier or id is wrong, or not in the connected workspace |
| `RATELIMITED` | Wait, then ask for less in one query |
