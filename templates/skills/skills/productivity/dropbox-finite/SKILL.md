---
name: dropbox-finite
description: Dropbox files and folders (list, search, download, upload, move, copy, temporary links) through the person's connected Dropbox account, with `fragment api dropbox` and Dropbox's API. No OAuth setup, no app key or token on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Dropbox, Files, Folders, Cloud Storage, Documents]
  fragment:
    connection: dropbox
---

# Dropbox

Your owner's Dropbox: its files and folders, and the team folders of a
Dropbox Business account. Your computer holds no Dropbox credential:
`fragment api dropbox` sends the connection's placeholder for you, and the
computer swaps in a short-lived token for your owner's account. Never ask
the person for a Dropbox password, an app key or a token, and never run
`dbxcli login`.

What you do in Dropbox is done as your owner: a file you upload, move or
delete is theirs, and their other devices sync it.

## The call

Every Dropbox call is a POST. Its arguments are a JSON body; a call that
takes none sends no body:

```bash
API=https://api.dropboxapi.com/2          # calls
CONTENT=https://content.dropboxapi.com/2  # a file's bytes, up and down
fragment api dropbox POST $API/users/get_current_account
```

An answer with `email`: connected, go on.

The answer is Dropbox's JSON on stdout. A refusal is one line of JSON on
stderr and exit 1. Do what its `hint` says; its `body` is Dropbox's own
answer: go by how its `error_summary` starts (`path/not_found/…`,
`to/conflict/file/…`).

The token goes to those two hosts only. An account whose `root_info` names
a `root_namespace_id` unlike its `home_namespace_id` is in a team: paths
start in the member's own folder unless a call adds
`--header 'Dropbox-API-Path-Root: {".tag": "root", "root": "<root_namespace_id>"}'`.

On an older computer: `fragment api` came with an image newer than a
computer awake since before it. If the command is `unrecognized`, your
computer takes that image when it next restarts. Until then, send the same
calls with `curl -H "Authorization: Bearer $DROPBOX_OAUTH_ACCESS_TOKEN"`: the computer swaps
that the same way.

## Read only

Your owner picks, as they connect Dropbox, whether you may only read (list,
search, download, links) or read and write. Connected read only, Dropbox
refuses every change with `missing_scope` (its `error_summary`). It is a
401, so the hint says to reconnect: wrong for this one. Tell your owner, in plain words, that they
connected Dropbox read only, and can allow changes in Settings →
Connections → Allow changes. Do not retry until they have; go on reading.

## Reading

```bash
# a folder's entries ("" is the root); "recursive": true for all under it
fragment api dropbox POST $API/files/list_folder --body '{"path": "/Work", "limit": 100}'
fragment api dropbox POST $API/files/list_folder/continue --body '{"cursor": "AAH…"}'
# names and contents; each option is optional
fragment api dropbox POST $API/files/search_v2 --body '{"query": "quarterly plan", "options": {"path": "/Work", "file_extensions": ["pdf"], "max_results": 25}}'
fragment api dropbox POST $API/files/search/continue_v2 --body '{"cursor": "…"}'
# one file's or folder's metadata: a file's size and rev
fragment api dropbox POST $API/files/get_metadata --body '{"path": "/Work/plan.pdf"}'
# a link to a file's content, good for four hours
fragment api dropbox POST $API/files/get_temporary_link --body '{"path": "/Work/plan.pdf"}'
```

- A path starts with `/` (`""` is the root), and Dropbox matches it
  ignoring case. An id (`id:a4ayc_80_OEAAAAAAAAAXw`, from any answer) names
  a file or folder wherever it moves; a path may follow it.
- `list_folder` answers `{entries, cursor, has_more}`, and `search_v2`
  `{matches, cursor, has_more}` with each entry at `metadata.metadata`.
  While `has_more`, pass `cursor` to the `continue` route, as it came.

## Downloading and uploading

```bash
fragment api dropbox POST $CONTENT/files/download --header 'Dropbox-API-Arg: {"path": "/Work/plan.pdf"}' --out ./plan.pdf
fragment api dropbox POST $CONTENT/files/upload --header 'Dropbox-API-Arg: {"path": "/Work/notes.txt", "mode": "add"}' \
  --body @./notes.txt --content-type application/octet-stream
```

- Both go to the content host, their arguments in the `Dropbox-API-Arg`
  header: JSON on one line, in ASCII. Write any other character as
  `\uXXXX` (`é` is `é`), or the name arrives wrong.
- Always download with `--out`. It saves a whole file or nothing (one cut
  short is `incomplete`), and replaces a file already at that path: pick a
  free one. Hold the `bytes` it answers to the `size` `get_metadata` gave.
- An upload is at most 150 MiB (a larger file needs Dropbox's upload
  session routes). `"mode": "add"` never replaces a file: a conflict is
  refused, or renamed with `"autorename": true`. `"overwrite"` replaces
  whatever is there. `{".tag": "update", "update": "<rev>"}` replaces only
  the revision you read, so a change someone made since is not lost.
- A temporary link and a shared link are on other hosts and need no token:
  fetch them with an ordinary download tool (`wget -O`), never with
  `fragment api`.

## Changing

```bash
fragment api dropbox POST $API/files/create_folder_v2 --body '{"path": "/Work/2026"}'
fragment api dropbox POST $API/files/move_v2 --body '{"from_path": "/Work/draft.txt", "to_path": "/Archive/draft.txt"}'
fragment api dropbox POST $API/files/copy_v2 --body '{"from_path": "/Work/plan.pdf", "to_path": "/Work/plan-v2.pdf"}'
fragment api dropbox POST $API/files/delete_v2 --body '{"path": "/Work/old.txt"}'
```

- `delete_v2` removes a file, or a folder with all it holds. Dropbox keeps
  deleted files for a time, so the person can restore one on dropbox.com.
  Never call `files/permanently_delete`.
- A call has 120 seconds. If a change or an upload ends `timed_out`,
  `unreachable` or `incomplete`, look first (`get_metadata`,
  `list_folder`): it may have happened.

Any other route (`files/list_revisions`, `users/get_space_usage`,
`sharing/list_shared_links`) is the same call with another path. Dropbox's
API is documented at https://docs.dropboxapi.com/dropbox-api.

## Rules

1. **Confirm before you upload, move, rename or replace anything** for your
   owner, unless they asked for exactly that: show what goes where.
2. **Never delete** unless your owner asked for that file or folder by name.
3. **Read before you write**: `get_metadata` or `list_folder` the place
   first, so a move or an upload replaces nothing by surprise.

## Troubleshooting

| Dropbox's `error_summary` starts | Fix |
|---------|-----|
| `invalid_access_token`, `expired_access_token` | Dropbox refused the token (the person removed the app, say): your owner reconnects Dropbox in Settings → Connections |
| `missing_scope` | Read only: they allow changes in Settings → Connections |
| `path/not_found`, `path_lookup/not_found` | The path is wrong, or in a team folder: list its parent, or add `Dropbox-API-Path-Root` |
| `path/conflict/…`, `to/conflict/…` | Something is there already: pick another name, or `"autorename": true` |
| `too_many_requests` (429) | Wait the `retry_after` seconds it names, then try once more |
