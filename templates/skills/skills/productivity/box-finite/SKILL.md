---
name: box-finite
description: Box files and folders (list, search, download, upload, move, copy, comments, shared links) through the person's connected Box account, with `fragment connection call box` and Box's API. No OAuth setup, no key on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Box, Files, Folders, Documents, Cloud Storage, File Sharing]
  fragment:
    connection: box
---

# Box

Your owner's Box account: its files and folders, their comments and shared
links. Your computer holds no Box credential:
`fragment connection call box` sends the connection's placeholder for you,
and the computer swaps in a short-lived token for your owner's account.
Never ask the person for a Box password, a developer token or a client
secret, and never run `box login`.

What you do in Box is done as your owner, with what their Box user may do:
a file you upload or a comment you write shows their name.

## Connected?

```bash
BOX=https://api.box.com/2.0
fragment connection call box GET $BOX/users/me --param fields=id,name,login
```

An answer with `login`: connected, go on. The token goes to `api.box.com`
(the API) and `upload.box.com` (uploads) only.

The answer is Box's JSON on stdout. A refusal is one line of JSON on stderr
and exit 1. Do what its `hint` says; its `body` is Box's own answer: `code`
(`not_found`, `item_name_in_use`) and `message`. Where Box sends no body,
go by `status`.

On an older computer: `fragment connection call` came with an image newer
than a computer awake since before it. If the command is `unrecognized`,
your computer takes that image when it next restarts. Until then, send the
same calls with `curl -H "Authorization: Bearer $BOX_OAUTH_ACCESS_TOKEN"`:
the computer swaps that the same way.

## Read only

Your owner picks, as they connect Box, whether you may only read or read
and write. Connected read only, you may list, search and read details,
comments and shared links. Box refuses every change and, by its own rule,
downloading a file's contents too: a 403, with
`access_denied_insufficient_permissions` or no code. Tell your owner, in
plain words, that they may have connected Box read only, and can allow
changes in Settings → Connections → Allow changes. Do not retry until they
have; go on reading.

## Reading

```bash
# a folder's items (0 is All Files), a page at a time
fragment connection call box GET $BOX/folders/0/items --param fields=type,id,name,size,modified_at --param usemarker=true --param limit=100
# one item's details (folders/<id> for a folder); --param fields=shared_link for its shared link
fragment connection call box GET $BOX/files/456
# names, descriptions, contents, comments and tags; every --param but query is optional
fragment connection call box GET $BOX/search --param 'query=quarterly plan' --param type=file --param file_extensions=pdf,docx \
  --param ancestor_folder_ids=123 --param fields=type,id,name,size,modified_at,parent --param limit=30 --param offset=0
fragment connection call box GET $BOX/files/456/comments --param fields=message,created_by,created_at --param limit=100 --param offset=0
```

- Box names every file, folder and comment by a number; `0` is the
  person's All Files. Find one by search, or by listing its folder.
- A folder's items page by marker: add `--param marker=<next_marker>` from
  the last answer, until it names none. Search (at most 200 a page) and a
  file's comments page by offset: the next is `offset` plus `limit`, while
  that is under `total_count`, however few entries a page holds.

## Downloading

```bash
fragment connection call box GET $BOX/files/456 --param fields=name,size
fragment connection call box GET $BOX/files/456/content --header 'Accept: */*' --out ./plan.pdf
```

Box answers a download with a redirect to a signed address on another host
(`dl.boxcloud.com`). `fragment connection call` follows it there without
the token, and `--out` saves the file whole or not at all: a download that
breaks off (`incomplete`) keeps nothing, and leaves a file already there
as it was.

- It answers `{"saved": …, "bytes": …}`: hold `bytes` to the `size` Box
  stated.
- `"status": 202` and no bytes is Box still preparing a file just uploaded:
  try again shortly.

## Changing

```bash
fragment connection call box POST $BOX/folders --body '{"name": "Q4 plans", "parent": {"id": "0"}}'
fragment connection call box PUT $BOX/files/456 --body '{"parent": {"id": "789"}}'   # move; folders/<id> for a folder
fragment connection call box POST $BOX/folders/123/copy --body '{"parent": {"id": "0"}, "name": "Q4 plans (copy)"}'
fragment connection call box POST $BOX/comments --body '{"item": {"type": "file", "id": "456"}, "message": "Looks good to me."}'
fragment connection call box PUT $BOX/files/456 --param fields=shared_link --body '{"shared_link": {"access": "company"}}'
fragment connection call box PUT $BOX/files/456 --param fields=shared_link --body '{"shared_link": null}'   # remove it
```

A shared link's `access`: `open` is anyone with the link, `company` your
owner's company, `collaborators` those already invited; `{}` takes the
account's default. Any other call (versions, collaborations, tasks, trash)
is the same command with another path under `/2.0/`. Box's API reference
is at https://developer.box.com/reference/.

## Uploading

Box takes a new file as a multipart form on `upload.box.com`: an
`attributes` part (its name and folder), then the file.
`fragment connection call` sends one body as it is given, so write the
form to a file and send that:

```bash
B="form-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
{ printf '%s\r\nContent-Disposition: form-data; name="attributes"\r\n\r\n%s\r\n' "--$B" '{"name": "notes.txt", "parent": {"id": "123"}}'
  printf '%s\r\nContent-Disposition: form-data; name="file"; filename="file"\r\nContent-Type: application/octet-stream\r\n\r\n' "--$B"
  cat ./notes.txt
  printf '\r\n%s\r\n' "--$B--"; } > /tmp/box-upload.form
fragment connection call box POST https://upload.box.com/api/2.0/files/content --body @/tmp/box-upload.form --content-type "multipart/form-data; boundary=$B"
```

It answers `{entries: [the new file]}`. A file is at most 50 MB this way,
and a name already in the folder is `item_name_in_use`. A new version, or a
larger file, is Box's own upload API (https://developer.box.com/guides/uploads).
If an upload ends `timed_out` or `unreachable`, list the folder before
trying again: it may have landed.

## Rules

1. **Confirm before you upload, move, copy, comment or share** for your
   owner, unless they asked for exactly that: show what you will change,
   and where. A shared link opens a file to others: say who.
2. **Never delete** unless asked for that item by name: an enterprise's
   settings may delete it for good, not to Box's trash.

## Troubleshooting

| Box's answer | Fix |
|---------|-----|
| 403, `access_denied_insufficient_permissions` | Read only (they allow changes in Settings → Connections), or their Box user may not do this to this item |
| `not_found` | The id is wrong, or their Box user cannot see it |
| `item_name_in_use` | Pick another name, or another folder |
| `rate_limit_exceeded` (429) | Wait, then ask again; ask for less at a time |
