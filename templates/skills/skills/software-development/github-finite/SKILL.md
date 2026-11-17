---
name: github-finite
description: GitHub repositories, issues, pull requests, reviews and Actions through the person's connected GitHub account, with `fragment connection call github` and git over HTTPS. No token to paste, no `gh auth login`.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [GitHub, Git, Pull Requests, Issues, Code Review, Actions]
  fragment:
    connection: github
---

# GitHub

Your owner's GitHub: the repositories they chose as they connected it,
with their issues, pull requests, reviews and workflow runs. Your computer
holds no GitHub credential: `fragment connection call github` sends the
connection's placeholder for you, and the computer swaps in a short-lived
token for your owner's account. Never ask the person for a personal access
token, and never run `gh auth login`.

What you do on GitHub is done as your owner: a comment, a commit pushed or
a pull request opened shows their name. This is not a fragment's own
repository (that is `git-finite`).

## Connected?

```bash
GH="fragment connection call github"
$GH GET https://api.github.com/user
```

`{"login": …}`: connected, as that account. The connection reaches
`api.github.com`, `uploads.github.com` and `github.com` only.

The answer is GitHub's JSON on stdout. A refusal is one line of JSON on
stderr and exit 1. Do what its `hint` says; its `body` is GitHub's own
answer: `message`, and for a 422 `errors`, which name the field refused.

A 404 on a repository you were told exists usually means your owner did
not choose it when they connected: tell them they can add it in their
GitHub settings (Applications), and do not guess another name.

On an older computer: `fragment connection call` came with an image newer
than a computer awake since before it. If the command is `unrecognized`,
your computer takes that image when it next restarts. Until then, send the
same calls with `curl -H "Authorization: Bearer $GH_TOKEN"`: the computer
swaps that the same way.

## Reading

```bash
# the repositories you may reach
$GH GET https://api.github.com/user/repos --param per_page=50 --param sort=pushed

# issues and pull requests (a pull request is an issue too)
$GH GET https://api.github.com/repos/OWNER/REPO/issues --param state=open --param per_page=30
$GH GET https://api.github.com/repos/OWNER/REPO/pulls/123
$GH GET https://api.github.com/repos/OWNER/REPO/pulls/123/files
$GH GET https://api.github.com/repos/OWNER/REPO/issues/123/comments

# a pull request's diff
$GH GET https://api.github.com/repos/OWNER/REPO/pulls/123 --header 'Accept: application/vnd.github.diff' --out pr-123.diff

# search: issues and pull requests, or code
$GH GET https://api.github.com/search/issues --param 'q=repo:OWNER/REPO is:pr is:open review-requested:@me'
$GH GET https://api.github.com/search/code --param 'q=repo:OWNER/REPO fn main'

# a file (base64 in `content`), a workflow's runs, a run's failed jobs
$GH GET https://api.github.com/repos/OWNER/REPO/contents/path/to/file --param ref=main
$GH GET https://api.github.com/repos/OWNER/REPO/actions/runs --param per_page=10
$GH GET https://api.github.com/repos/OWNER/REPO/actions/runs/RUN_ID/jobs --param filter=latest
```

- Lists come 30 at a time: `per_page` (to 100) and `page`. Ask for the
  page you need, not every page.
- Many things in one call: GraphQL, `$GH POST https://api.github.com/graphql
  --query '…' --variables '{…}'`.
- A job's log (`…/actions/jobs/JOB_ID/logs`) and a repository's archive
  (`…/tarball/REF`) answer a redirect to a signed address on another host:
  `--out FILE` follows it there, without the token, and saves it whole or
  not at all.

## Changing

```bash
$GH POST https://api.github.com/repos/OWNER/REPO/issues --body '{"title": "Login fails on Safari", "body": "Steps…", "labels": ["bug"]}'
$GH POST https://api.github.com/repos/OWNER/REPO/issues/123/comments --body '{"body": "Fixed in #124."}'
$GH PATCH https://api.github.com/repos/OWNER/REPO/issues/123 --body '{"state": "closed"}'
$GH POST https://api.github.com/repos/OWNER/REPO/pulls --body '{"title": "Fix login", "head": "fix-login", "base": "main", "body": "What and why…"}'
$GH POST https://api.github.com/repos/OWNER/REPO/pulls/123/reviews --body '{"event": "COMMENT", "body": "Two notes below."}'
```

## Git

Clone, fetch and push over HTTPS, the token as a header that git sends to
`github.com` alone. Set it once in the clone, never in the address:

```bash
AUTH="http.https://github.com/.extraHeader=Authorization: Bearer $GH_TOKEN"
git -c "$AUTH" clone https://github.com/OWNER/REPO.git && cd REPO
git config "http.https://github.com/.extraHeader" "Authorization: Bearer $GH_TOKEN"
git checkout -b fix-login && git commit -am "Fix login" && git push -u origin fix-login
```

`GH_TOKEN` holds the placeholder, not a token: it is worth nothing
anywhere but on this computer, on its way to GitHub.

## Rules

1. **Confirm before you write for your owner**: a comment, a review, a
   closed issue, a pull request, a push. Show what, and where, unless they
   asked for exactly that.
2. **Never push to a default branch, force-push, merge or delete a
   branch** unless your owner asked for that, by name.
3. **Work on a branch and open a pull request**: your owner reviews it.
4. **Respect rate limits**: 5,000 requests an hour, search 30 a minute. A
   403 or 429 with a `Retry-After` means wait that long.

## Troubleshooting

| GitHub's answer | Fix |
|---------|-----|
| 401 `Bad credentials` | GitHub refused the token (the person revoked the app): your owner reconnects GitHub in Settings → Connections |
| 404 on a repository | Your owner did not choose it as they connected: they add it in GitHub's settings |
| 403 `Resource not accessible by integration` | The connection may not do this on that repository: say so, and stop |
| 422 `Validation Failed` | `body`'s `errors` name the resource, the field and what is wrong with it |
