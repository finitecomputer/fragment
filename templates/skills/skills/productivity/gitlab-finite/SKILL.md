---
name: gitlab-finite
description: GitLab.com projects, issues, merge requests, CI pipelines and repository files through the person's connected GitLab account, with `fragment connection call gitlab` and GitLab's REST API. No OAuth setup, no access token on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [GitLab, Git, Issues, Merge Requests, Code Review, CI/CD, Pipelines]
  fragment:
    connection: gitlab
---

# GitLab

Your owner's GitLab.com account: its projects, issues, merge requests, CI
pipelines and repository files, through GitLab's REST API. Your computer
holds no GitLab credential: `fragment connection call gitlab` sends the
connection's placeholder for you, and the computer swaps in a short-lived
token for your owner's account. Never ask the person for a GitLab password
or a personal access token, and never set `GITLAB_TOKEN`.

GitLab.com only: a self-managed GitLab (one at a host of its own) is not
connected here, so say so if asked. What you do in GitLab is done as your
owner: an issue you open or a comment you write shows their name.

## Connected?

```bash
G="https://gitlab.com/api/v4"
fragment connection call gitlab GET $G/user
# → {"id": 1234567, "username": "ada", "name": …, "email": …, "web_url": …}
```

An answer with `username`: connected, go on. Its `id` is your owner's.

The token goes to `gitlab.com` alone, and as a bearer token alone, which
is how `fragment connection call` sends it. The answer is GitLab's JSON on
stdout. A refusal is one line of JSON on stderr and exit 1. Do what its
`hint` says; its `body` is GitLab's own answer: `message`
(`404 Project Not Found`), or `error` and `error_description`
(`invalid_token`, `insufficient_scope`).

On an older computer: `fragment connection call` came with an image newer
than a computer awake since before it. If the command is `unrecognized`,
your computer takes that image when it next restarts. Until then, send the
same calls with
`curl -H "Authorization: Bearer $GITLAB_OAUTH_ACCESS_TOKEN"`: the computer
swaps that the same way.

## Read only

Your owner picks, as they connect GitLab, whether you may only read
(everything they can see) or read and write: everything they may do through
GitLab's API, in every group and project they belong to (merge, commit, run
pipelines and, where they are an owner, delete a project). Take the care
that calls for (Rules).

Connected read only, GitLab refuses every change: 403 `insufficient_scope`.
If a change is refused so while reads work, tell your owner, in plain
words, that they connected GitLab read only and can allow changes in
Settings → Connections → Allow changes. Do not retry it until they say they
have; go on reading. Any other 403 is GitLab's own permission: your owner's
role in that project or group does not allow it.

## Reading

```bash
P="$G/projects/group%2Fsubgroup%2Fproject"   # a project: its path, URL-encoded, or its numeric id

# the projects your owner is a member of, latest activity first
fragment connection call gitlab GET $G/projects --param membership=true --param simple=true --param order_by=last_activity_at --param search=api
fragment connection call gitlab GET $P

# a project's issues; with no project, those your owner opened, or (scope) is assigned
fragment connection call gitlab GET $P/issues --param state=opened --param search=login --param labels=bug --param order_by=updated_at
fragment connection call gitlab GET $G/issues --param scope=assigned_to_me --param state=opened
fragment connection call gitlab GET $P/issues/42
fragment connection call gitlab GET $P/issues/42/notes --param sort=asc --param per_page=100    # its comments

# merge requests, the same way (scope=reviews_for_me: waiting on your owner's review)
fragment connection call gitlab GET $P/merge_requests --param state=opened
fragment connection call gitlab GET $P/merge_requests/17             # with detailed_merge_status and head_pipeline
fragment connection call gitlab GET $P/merge_requests/17/diffs

# pipelines, a pipeline's jobs, and a job's log (long: save it, then read its end)
fragment connection call gitlab GET $P/pipelines --param ref=main --param status=failed
fragment connection call gitlab GET $P/pipelines/123456/jobs
fragment connection call gitlab GET $P/jobs/987654/trace --out job.log
```

- An issue or a merge request is its number in its project (`#42`, `!17`:
  GitLab's `iid`). `state` is `opened`, `closed`, `merged` or `all`.
- Lists come a page at a time: `per_page` (20, at most 100) and `page`.
- A note with `"system": true` is GitLab's own record of a change, not a
  comment. A job's log carries color codes and `section_start:` markers.
- A job's artifacts (`$P/jobs/987654/artifacts`) answer a redirect to a
  signed address on another host: `--out FILE` follows it there, without
  the token.

## A repository's code

`git clone` and `git push` over HTTPS do not work with this connection: the
computer swaps the placeholder only as a bearer token, and refuses it as
the password in a Git URL. (A public project clones with no token.) Instead:

```bash
fragment connection call gitlab GET $P/repository/tree --param path=src --param ref=main --param recursive=true --param per_page=100
fragment connection call gitlab GET $P/repository/files/src%2Fapp%2Epy/raw --param ref=main     # a file, printed; --out FILE for one that is not text
fragment connection call gitlab GET $P/repository/archive.tar.gz --param sha=main --out project.tar.gz
```

- A file's path is URL-encoded whole: `/` as `%2F`, `.` as `%2E`. A ref is
  a branch, tag or commit; the project's default branch if none.
- The archive (`tar.gz`, `tar.bz2`, `tar` or `zip`) is the files at a ref,
  without history. `--out` saves the answer whole or not at all: a download
  that breaks off (`incomplete`) leaves the file as it was. Ask again.

## Changing

```bash
# assignee_ids: your owner's id (from /user), to assign it to them
fragment connection call gitlab POST $P/issues --body '{"title": "Fix login", "description": "Steps…", "labels": "bug", "assignee_ids": [1234567]}'
fragment connection call gitlab POST $P/issues/42/notes --body '{"body": "Fixed in !17."}'     # a comment; a merge request's the same way
fragment connection call gitlab PUT $P/issues/42 --body '{"state_event": "close"}'

# change files: one commit of several, here on a new branch from main
fragment connection call gitlab POST $P/repository/commits --body '{"branch": "fix-typo", "start_branch": "main", "commit_message": "Fix a typo",
  "actions": [{"action": "update", "file_path": "README.md", "content": "…"}]}'
```

Anything else (branches, labels, milestones, approvals, merges) is the same
call with another path under `/api/v4`: https://docs.gitlab.com/api/rest/.

## Rules

1. **Confirm before you create, close, merge, comment or commit** for your
   owner, unless they asked for exactly that: show what you will write.
2. **Never delete** (a branch, an issue, a project) unless asked for that
   one by name.
3. **Read before you write**: find the issue or merge request before
   changing it, so a change lands on the right one.
4. **Respect rate limits**: GitLab.com allows 2,000 API requests a minute,
   and 5 archives; on a 429, wait a minute, then ask for less.

## Troubleshooting

| GitLab's answer | Fix |
|---------|-----|
| `invalid_token` | GitLab refused the token (the person revoked the app, say): your owner reconnects GitLab in Settings → Connections |
| 403, `insufficient_scope` | Connected read only: they allow changes in Settings → Connections |
| Another 403 | Your owner's role in that project or group does not allow it |
| 404 | The project, number or path is wrong (is it URL-encoded?), or your owner cannot see it: GitLab answers 404 for a private project they are not in |
