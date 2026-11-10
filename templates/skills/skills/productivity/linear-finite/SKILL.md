---
name: linear-finite
description: Linear issues, comments, teams and projects through the person's connected Linear account, with the bundled helper or Linear's GraphQL API. No OAuth setup, no API key on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Linear, Issues, Tickets, Projects, Project Management]
---

# Linear

Linear issues, comments, teams and projects through your owner's connected
Linear account. Your computer holds no Linear credential:
`LINEAR_OAUTH_ACCESS_TOKEN` holds the connection's placeholder (it names
you), it goes as a bearer token to Linear's API, and the computer swaps in a
short-lived token for your owner's account. There is no OAuth app to make and
no API key to paste: never ask the person for a Linear key, and never set
`LINEAR_API_KEY`.

What you do in Linear is done as your owner: an issue you create or a comment
you write shows their name.

## Connected?

```bash
LAPI="python3 ${HERMES_SKILL_DIR}/scripts/linear_api.py"
$LAPI check
```

- `{"status": "connected", "email": …, "workspace": …}`: go on.
- `not_connected`: your owner has not connected Linear, or must authorize it
  again (`LINEAR_OAUTH_ACCESS_TOKEN` is then unset). Ask them to connect it
  in the shell's Settings, under Connections, then check again: your
  computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left Linear out. Say
  so, and stop.
- `authentication_error`: Linear refused the token (the person revoked the
  app in Linear, say). Ask them to reconnect Linear in Settings.

The connection (`linear`) is swapped for `api.linear.app` (the GraphQL API)
and `uploads.linear.app` (files attached to issues) only. It is one Linear
workspace: the one your owner picked when they connected it.

## The helper

```bash
$LAPI teams                                           # each team's id, key (ENG) and name
$LAPI issues search "login bug" --max 10              # full-text search
$LAPI issues list --mine --state "In Progress"        # yours, by state
$LAPI issues list --team ENG --max 25                 # a team's, newest change first
$LAPI issues get ENG-123                              # with its description, labels and comments
$LAPI issues create --team ENG --title "Fix login" --description "Steps…" --priority 2 --assign-me
$LAPI issues update ENG-123 --state Done
$LAPI issues update ENG-123 --title "…" --description "…" --priority 1
$LAPI issues comment ENG-123 --body "Fixed in #42."
$LAPI projects --max 25
```

- An issue is named by its identifier (`ENG-123`) or its id.
- A state is its team's workflow state's name (`Todo`, `In Progress`,
  `Done`), matched ignoring case.
- Priority: 0 none, 1 urgent, 2 high, 3 medium, 4 low.
- Descriptions and comments are Markdown.

### Anything else

Any other query or mutation (cycles, labels, attachments, project updates),
through the helper or straight to the API: the computer swaps the token in.

```bash
$LAPI graphql 'query($n: Int) { cycles(first: $n) { nodes { number startsAt endsAt } } }' --variables '{"n": 3}'

curl -s https://api.linear.app/graphql \
  -H "Authorization: Bearer $LINEAR_OAUTH_ACCESS_TOKEN" -H "Content-Type: application/json" \
  -d '{"query": "{ viewer { name email } }"}'
```

Send the variable with `Bearer `: it is an OAuth token, not a personal API
key. Linear's API docs are at https://linear.app/developers; the schema
answers introspection through the helper's `graphql`.

## Output Format

Every command prints JSON. Issues are `{identifier, title, url,
priorityLabel, updatedAt, state: {name}, team: {key}, assignee}`; `get` adds
`description`, `dueDate`, `project`, `labels` and `comments`; `create` and
`update` answer `{success, issue}`. A failure is one JSON line on stderr,
`{error, status, message, hint}`, and exit 1.

## Rules

1. **Confirm before you create, close, reassign or comment** for your owner,
   unless they asked for exactly that: show what you will write, and where.
2. **Check the connection before first use** (`$LAPI check`).
3. **Read before you write**: find the issue (`search`, `get`) and its team's
   states before changing it, so an update lands on the right one.
4. **Respect rate limits**: Linear limits requests per hour, and search to
   30 a minute. Ask for what you need in one query rather than many.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Linear in Settings → Connections |
| `forbidden` | Your owner left Linear off this agent's connections |
| `authentication_error` | Your owner reconnects Linear in Settings → Connections |
| `no_such_team` / `no_such_state` | List them: `$LAPI teams`, or the issue's team's states with `graphql` |
| `invalid_input`, `Entity not found` | The identifier or id is wrong, or not in the connected workspace |
