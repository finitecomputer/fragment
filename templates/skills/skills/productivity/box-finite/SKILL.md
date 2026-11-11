---
name: box-finite
description: Box files and folders (list, search, download, upload, move, copy, comments, shared links) through the person's connected Box account, with the bundled helper or Box's API. No OAuth setup, no key on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Box, Files, Folders, Documents, Cloud Storage, File Sharing]
---

# Box

Your owner's Box account: its files and folders, their comments and shared
links. Your computer holds no Box credential: `BOX_OAUTH_ACCESS_TOKEN` holds
the connection's placeholder (it names you), it goes as a bearer token to
Box's API, and the computer swaps in a short-lived token for your owner's
account. There is no app to make and no key to paste: never ask the person
for a Box password, a developer token or a client secret, and never run
`box login`.

What you do in Box is done as your owner, with what their Box user may do:
a file you upload or a comment you write shows their name.

## Connected?

```bash
BOX="python3 ${HERMES_SKILL_DIR}/scripts/box_api.py"
$BOX check
```

- `{"status": "connected", "name": …, "login": …}`: go on.
- `not_connected`: your owner has not connected Box, or must authorize it
  again (`BOX_OAUTH_ACCESS_TOKEN` is then unset). Ask them to connect it in
  the shell's Settings, under Connections, then check again: your computer
  is given it within seconds.
- `forbidden`, with the hint that you were narrowed: your owner narrowed
  your connections and left Box out. Say so, and stop.
- `invalid_token`: Box refused the token (the person removed the app's
  access in Box, say). Ask them to reconnect Box in Settings.

The connection (`box`) is swapped for `api.box.com` (the API) and
`upload.box.com` (uploads) only. A file's bytes come from Box's download
URL on another host (`dl.boxcloud.com`), which needs no token: the helper
fetches it with none, and so must you.

## Read only

Your owner picks, as they connect Box, whether you may only read or read
and write. Connected read only, you may list, search and read files' and
folders' details, comments and shared links; Box refuses every change
(upload, new folder, move, copy, comment, shared link) and, by its own
rule, downloading a file's contents too (`insufficient_scope`, or
`access_denied_insufficient_permissions`, 403). If a change or a download
is refused so while listing works, tell your owner, in plain words, that
they may have connected Box read only, and that they can allow changes in
Settings → Connections → Allow changes. Do not retry it until they say they
have; go on reading.

## The helper

```bash
$BOX folders list                                # All Files (folder 0), 100 items a page
$BOX folders list 123 --limit 50 --marker JV9I…  # the next page: the last one's next_marker
$BOX folders get 123
$BOX search "quarterly plan" --type file --ext pdf,docx --in 123
$BOX files get 456
$BOX files download 456 --out ./plan.pdf         # or --out a folder; its own name here, unnamed
$BOX files upload ./notes.txt --folder 123        # at most 50 MB
$BOX folders create "Q4 plans" --parent 0
$BOX move file 456 --to 789
$BOX copy folder 123 --to 0 --name "Q4 plans (copy)"
$BOX comments list 456
$BOX comments add 456 --message "Looks good to me."
$BOX share get file 456
$BOX share create file 456 --access company      # open, company or collaborators
$BOX share remove file 456
```

- Box names every file, folder and comment by a number; `0` is the
  person's All Files. Find one by `search`, or by listing its folder.
- `folders list` answers a page and its `next_marker`: pass it as
  `--marker` for the next page, until it is `null`.
- `search` matches names, descriptions, contents, comments and tags; `--in`
  keeps it under one folder; `--limit` (at most 200) and `--offset` page
  through it.
- `download` refuses to overwrite a file unless `--overwrite`; Box may
  answer `not_ready` for a file just uploaded: download it again shortly.
- `upload` makes a new file; a name already in the folder is
  `item_name_in_use`. A new version of a file, or one over 50 MB, is Box's
  own upload API (its docs: https://developer.box.com/guides/uploads).
- A shared link's `--access`: `open` is anyone with the link, `company`
  your owner's company, `collaborators` those already invited. Unnamed, it
  is the account's default.

### Anything else

Any other call on Box's API (versions, collaborations, tasks, metadata,
trash), through the helper or straight to it: the computer swaps the token
in.

```bash
$BOX request GET "/2.0/files/456/versions"
$BOX request PUT /2.0/files/456 --json '{"description": "Signed copy"}'

curl -s https://api.box.com/2.0/users/me -H "Authorization: Bearer $BOX_OAUTH_ACCESS_TOKEN"
```

`request` reaches any path from `/2.0/` on `api.box.com`, its query after
`?`, and `--json` is its body. Box's API reference is at
https://developer.box.com/reference/. Never send the variable to another
host: a download's redirect needs none.

## Output Format

Every command prints JSON: Box's own objects (`type`, `id`, `name`, `size`,
`modified_at` for items; `folders list` adds `next_marker`, `search`
`total_count` and `offset`); `download` answers `{status, id, path, bytes}`.
A failure is one JSON line on stderr, `{error, status, message, hint}`, and
exit 1; Box's own error codes come through as Box names them
(`not_found`, `item_name_in_use`, `insufficient_scope`).

## Rules

1. **Confirm before you upload, move, copy, comment or share** for your
   owner, unless they asked for exactly that: show what you will change,
   and where. A shared link opens a file to others: say who.
2. **Check the connection before first use** (`$BOX check`).
3. **Never delete** unless asked for that item by name: an enterprise's
   settings may delete it for good, not to Box's trash.
4. **Ask for what you need**: list a folder a page at a time, and search
   within the folder you mean.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Box in Settings → Connections |
| `forbidden` (narrowed) | Your owner left Box off this agent's connections |
| `invalid_token` | Your owner reconnects Box in Settings → Connections |
| `insufficient_scope`, `access_denied_insufficient_permissions` | Read only (they allow changes in Settings → Connections), or their Box user may not do this to this item |
| `not_found` | The id is wrong, or their Box user cannot see it |
| `item_name_in_use` | Pick another name, or another folder |
| `not_ready` | Box is still preparing a file just uploaded: download it again shortly |
| `rate_limit_exceeded` | Wait, then ask again; ask for less at a time |
| `redirected` | Box answered a redirect, which the helper never follows with the token: download files with `files download` |
