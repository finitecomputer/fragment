---
name: dropbox-finite
description: Dropbox files and folders (list, search, download, upload, move, copy, temporary links) through the person's connected Dropbox account, with the bundled helper or Dropbox's API. No OAuth setup, no app key or token on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Dropbox, Files, Folders, Cloud Storage, Documents]
---

# Dropbox

Your owner's Dropbox: its files and folders, and the team folders of a
Dropbox Business account. Your computer holds no Dropbox credential:
`DROPBOX_OAUTH_ACCESS_TOKEN` holds the connection's placeholder (it names
you), it goes as a bearer token to Dropbox's API, and the computer swaps in a
short-lived token for your owner's account. There is no app to make and no
token to paste: never ask the person for a Dropbox password, an app key or a
token, and never run `dbxcli login`.

What you do in Dropbox is done as your owner: a file you upload, move or
delete is theirs, and their other devices sync it.

## Connected?

```bash
DBX="python3 ${HERMES_SKILL_DIR}/scripts/dropbox_api.py"
$DBX check
```

- `{"status": "connected", "name": …, "email": …, "team_space": false}`: go on.
- `not_connected`: your owner has not connected Dropbox, or must authorize it
  again (`DROPBOX_OAUTH_ACCESS_TOKEN` is then unset). Ask them to connect it
  in the shell's Settings, under Connections, then check again: your
  computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left Dropbox out.
  Say so, and stop.
- `invalid_access_token`: Dropbox refused the token (the person removed the
  app from their Dropbox's connected apps, say). Ask them to reconnect
  Dropbox in Settings.

The connection (`dropbox`) is swapped for `api.dropboxapi.com` (calls) and
`content.dropboxapi.com` (uploads and downloads) only. A temporary link
(`link`) and a shared link are on other hosts and need no token: fetch them
without one, never with the placeholder.

`"team_space": true` means the account is in a Dropbox team whose folders
sit above the member's own: paths start in the member's folder unless you
pass `--root <root_namespace_id>` (from `check`) before the command, as in
`$DBX --root 3235640 list`.

## Read only

Your owner picks, as they connect Dropbox, whether you may only read (list,
search, download, links) or read and write (also upload, make folders, move,
copy and delete). Connected read only, Dropbox refuses every change with
`missing_scope` (401), naming the scope it needs (`files.content.write`). If
a change is refused so while reads work, tell your owner, in plain words,
that they connected Dropbox read only, and that they can allow changes in
Settings → Connections → Allow changes. Do not retry it until they say they
have; go on reading.

## The helper

```bash
$DBX list                                     # the root's entries
$DBX list /Work --max 200                     # a folder's; --recursive for all under it
$DBX list --cursor AAH…                       # the next page, while has_more
$DBX search "quarterly plan" --path /Work --ext pdf
$DBX get /Work/plan.pdf                       # its metadata, with its rev
$DBX download /Work/plan.pdf --to ./plan.pdf
$DBX upload ./notes.txt --to /Work/notes.txt  # never replaces a file
$DBX upload ./notes.txt --to /Work/notes.txt --mode update --rev a1c10ce0dd78
$DBX mkdir /Work/2026
$DBX move /Work/draft.txt /Archive/draft.txt
$DBX copy /Work/plan.pdf /Work/plan-v2.pdf
$DBX delete /Work/old.txt
$DBX link /Work/plan.pdf                      # a link to its content, for four hours
$DBX request POST /2/files/list_revisions --json '{"path": "/Work/plan.pdf", "limit": 5}'
```

- A path starts with `/` (`""` or `/` is the root), and Dropbox matches it
  ignoring case. An id (`id:a4ayc_80_OEAAAAAAAAAXw`, from any answer) names
  a file or folder wherever it moves; a path may follow it
  (`id:a4ayc…/notes.txt`).
- `upload` takes a file of at most 150 MiB. `--mode add` (the default)
  never replaces a file: a conflict is refused, or renamed with
  `--autorename`. `--mode overwrite` replaces whatever is there;
  `--mode update --rev <rev>` replaces only the revision you read (`get`
  shows its `rev`), so a change someone made since is not lost.
- `download` saves to a local file (its name, here, by default) and never
  replaces one unless you pass `--overwrite`. It saves only a whole file:
  one cut short (`incomplete_answer`) saves nothing and leaves a local file
  already there as it was.
- `delete` removes a file, or a folder with all it holds. Dropbox keeps
  deleted files for a time (as long as the account's plan says), so the
  person can restore one on dropbox.com. Never call
  `/2/files/permanently_delete`.
- `request` reaches any RPC route on `api.dropboxapi.com` (from `/2/`: file
  revisions, space usage, shared links, file requests), all POST; `--json` is
  its arguments. Dropbox's API is documented at
  https://docs.dropboxapi.com/dropbox-api. Uploads and downloads go through
  the helper's own commands.

## Output Format

Every command prints JSON. `list` answers `{entries, has_more, cursor}` and
`search` `{matches, has_more, cursor}`, each entry `{".tag": "file" |
"folder", name, path_display, id}` and, for a file, `size`,
`server_modified` and `rev`; `get` answers Dropbox's whole metadata;
`download` `{saved_to, bytes, metadata}`; `link` `{link, expires,
metadata}`. A failure is one JSON line on stderr, `{error, status, message,
hint}`, and exit 1; Dropbox's own errors come through as its
`error_summary` (`path/not_found`, `path/conflict/file`, `to/conflict`,
`missing_scope`).

## Rules

1. **Confirm before you upload, move, rename or replace anything** for your
   owner, unless they asked for exactly that: show what goes where.
2. **Never delete** unless your owner asked for that file or folder by name.
3. **Check the connection before first use** (`$DBX check`).
4. **Read before you write**: `get` or `list` the place first, so a move or
   an upload lands where it should and replaces nothing by surprise.
5. **Ask for what you need**: a folder's own entries before `--recursive`,
   and `--max` to keep answers small.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Dropbox in Settings → Connections |
| `forbidden` | Your owner left Dropbox off this agent's connections |
| `invalid_access_token`, `expired_access_token` | Your owner reconnects Dropbox in Settings → Connections |
| `missing_scope` | Read only: they allow changes in Settings → Connections |
| `path/not_found`, `path_lookup/not_found` | The path is wrong, or in a team folder: `list` its parent, or pass `--root` (see Connected?) |
| `path/conflict/…`, `to/conflict/…` | Something is there already: pick another name, or `--autorename` |
| `too_many_requests` | Dropbox is limiting requests: wait, then try once more |
| `invalid_root` | The account's team changed: run `check`, and pass its `root_namespace_id` |
| `incomplete_answer` | The connection broke mid-answer: nothing was saved; try once more, but after a change `get` or `list` first, since it may have happened |
| `redirected` | Dropbox answered a redirect, which the helper never follows with the token: say so, and stop |
