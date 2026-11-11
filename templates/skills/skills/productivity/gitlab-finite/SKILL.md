---
name: gitlab-finite
description: GitLab.com projects, issues, merge requests, CI pipelines and repository files through the person's connected GitLab account, with the bundled helper or GitLab's REST API. No OAuth setup, no access token on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [GitLab, Git, Issues, Merge Requests, Code Review, CI/CD, Pipelines]
---

# GitLab

Your owner's GitLab.com account: its projects, issues, merge requests, CI
pipelines and repository files, through GitLab's REST API
(`https://gitlab.com/api/v4`). Your computer holds no GitLab credential:
`GITLAB_OAUTH_ACCESS_TOKEN` holds the connection's placeholder (it names
you), it goes as a bearer token to `gitlab.com`, and the computer swaps in a
short-lived token for your owner's account. There is no app to make and no
token to paste: never ask the person for a GitLab password or a personal
access token, and never set `GITLAB_TOKEN`.

GitLab.com only. A self-managed GitLab (one at a host of its own, such as
`gitlab.example.com`) is not connected here: say so if asked.

What you do in GitLab is done as your owner: an issue you open or a comment
you write shows their name.

## Connected?

```bash
GLAPI="python3 ${HERMES_SKILL_DIR}/scripts/gitlab_api.py"
$GLAPI check
```

- `{"status": "connected", "username": …, "name": …}`: go on.
- `not_connected`: your owner has not connected GitLab, or must authorize it
  again (`GITLAB_OAUTH_ACCESS_TOKEN` is then unset). Ask them to connect it
  in the shell's Settings, under Connections, then check again: your
  computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left GitLab out. Say
  so, and stop.
- `invalid_token`: GitLab refused the token (the person revoked the app in
  GitLab, say). Ask them to reconnect GitLab in Settings.

The connection (`gitlab`) is swapped on `gitlab.com` only, and only as a
bearer token. It is one GitLab account: the one signed in to gitlab.com when
your owner connected it.

## Read only

Your owner picks, as they connect GitLab, whether you may only read
(GitLab's `read_api`: every group, project, issue, merge request, pipeline
and file they can see) or read and write (`api`). Read and write is broad:
it is everything your owner may do through GitLab's API, in every group and
project they belong to, repositories included. You could open and close
issues, comment, approve and merge merge requests, run and cancel pipelines,
commit to branches and, where they are an owner, delete a project. Take the
care that calls for (Rules).

Connected read only, GitLab refuses every change: 403
`insufficient_scope`. If a change is refused so while reads work, tell your
owner, in plain words, that they connected GitLab read only, and that they
can allow changes in Settings → Connections → Allow changes. Do not retry it
until they say they have; go on reading. A 403 that is not
`insufficient_scope` (`not_allowed`) is GitLab's own permission: your
owner's role in that project or group does not allow it.

## The helper

```bash
$GLAPI projects list --search api --max 20          # yours, latest activity first
$GLAPI projects get group/project
$GLAPI issues list --project group/project --search "login" --labels bug
$GLAPI issues list --mine                           # assigned to you, in every project
$GLAPI issues get group/project 42                  # with its comments
$GLAPI issues create group/project --title "Fix login" --description "Steps…" --labels bug --assign-me
$GLAPI issues comment group/project 42 --body "Fixed in !17."
$GLAPI mrs list --project group/project --state opened
$GLAPI mrs list --reviewing                         # waiting on your review
$GLAPI mrs get group/project 17                     # with its comments and its pipeline
$GLAPI mrs diffs group/project 17
$GLAPI mrs comment group/project 17 --body "Looks good to me."
$GLAPI pipelines list group/project --ref main --status failed
$GLAPI pipelines get group/project 123456
$GLAPI jobs list group/project 123456               # a pipeline's jobs
$GLAPI jobs get group/project 987654
$GLAPI jobs log group/project 987654 --tail 200     # where a failed job failed
$GLAPI files tree group/project src --ref main
$GLAPI files get group/project src/app.py --ref main
$GLAPI files archive group/project --ref main --out project.tar.gz
```

- A project is its path, as in its URL (`group/subgroup/project`), or its
  numeric id. The helper URL-encodes a path (`group%2Fsubgroup%2Fproject`).
- An issue or a merge request is its number in its project (`#42`, `!17`:
  GitLab's `iid`).
- Lists default to open ones (`--state`), and without `--project` to those
  you opened; `--mine` is those assigned to you, `--reviewing` the merge
  requests you review.
- A ref is a branch, tag or commit; the project's default branch if none.
- Descriptions and comments are Markdown.

### A repository's code

`git clone` and `git push` over HTTPS do not work with this connection: the
computer swaps the placeholder only as a bearer token, and refuses it as the
password in a Git URL. Instead:

- Read a file (`files get`) or a folder (`files tree`), at any ref.
- Download the whole tree at a ref (`files archive`): a snapshot of its
  files, without history. GitLab.com allows 5 a minute.
- A public project clones as any does, with no token:
  `git clone https://gitlab.com/group/project.git`.
- Connected read and write, change files through the API: one commit of
  several files (`POST /api/v4/projects/:id/repository/commits`), with
  `request` below.

### Anything else

Any other call (branches, commits, labels, milestones, approvals, merges)
through the helper's `request`, from `/api/v4/`, or straight to the API:
the computer swaps the token in.

```bash
$GLAPI request GET "/api/v4/projects/group%2Fproject/repository/branches?per_page=50"
$GLAPI request PUT /api/v4/projects/group%2Fproject/issues/42 --json '{"state_event": "close"}'
$GLAPI request POST /api/v4/projects/group%2Fproject/repository/commits --json '{"branch": "fix-typo", "start_branch": "main", "commit_message": "Fix a typo", "actions": [{"action": "update", "file_path": "README.md", "content": "…"}]}'

curl -s https://gitlab.com/api/v4/user -H "Authorization: Bearer $GITLAB_OAUTH_ACCESS_TOKEN"
```

Send the variable with `Bearer `: it is an OAuth token, and the computer
refuses it anywhere else (a `PRIVATE-TOKEN` header, a URL's password). A
project's path in a request is URL-encoded (`group%2Fproject`), as is a
file's (`src%2Fapp.py`). Lists come a page at a time: `per_page` (at most
100) and `page`. GitLab's REST API docs: https://docs.gitlab.com/api/rest/.

## Output Format

Every command prints JSON. Issues and merge requests are `{iid, title,
state, web_url, labels, updated_at, author, assignees}`, merge requests with
`source_branch`, `target_branch` and `draft`; `get` adds `description` and
`notes` (the comments, oldest first), a merge request its `reviewers`,
`detailed_merge_status` and `head_pipeline`. `projects list` and `jobs list`
keep a few fields of each; the rest (a project, pipelines, a job, diffs,
trees) come as GitLab answers them. `files get` prints
`{project, path, ref, content}`, `jobs log` `{project, job, lines, log}`
(its colors removed), and a command with `--out` `{saved, bytes}`. A failure
is one JSON line on stderr, `{error, status, message, hint}`, and exit 1.

## Rules

1. **Confirm before you create, close, merge, comment or commit** for your
   owner, unless they asked for exactly that: show what you will write, and
   where.
2. **Check the connection before first use** (`$GLAPI check`).
3. **Never delete** (a branch, an issue, a project) unless asked for that
   one by name.
4. **Read before you write**: find the issue or merge request (`list`,
   `get`) before changing it, so a change lands on the right one.
5. **Respect rate limits**: GitLab.com allows 2,000 API requests a minute,
   and 5 archives. Ask for what you need with `--max` and filters.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects GitLab in Settings → Connections |
| `forbidden` | Your owner left GitLab off this agent's connections |
| `invalid_token`, `unauthorized` | Your owner reconnects GitLab in Settings → Connections |
| `insufficient_scope` | Connected read only: they allow changes in Settings → Connections |
| `not_allowed` | Your owner's role in that project or group does not allow it |
| `not_found` | The project, number or path is wrong, or your owner cannot see it (GitLab answers 404 for a private project they are not in) |
| `bad_project`, `bad_iid`, `bad_path`, `bad_ref`, `bad_json` | The helper refused it before asking GitLab: fix it |
| `binary` | The file is not text: `files get … --out FILE` |
| `rate_limited` | Wait a minute, then ask for less |
| `redirected` | GitLab answered a redirect, which the helper never follows with the token: say so, and stop |
