---
name: box-finite
description: Box files and folders (list, search, download, upload, move, copy, comments, shared links) through the person's connected Box account, with `fragment api box` and Box's API. No OAuth setup, no key on the computer.
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
links. Your computer holds no Box credential: `fragment api box` sends the
connection's placeholder for you, and the computer swaps in a short-lived
token for your owner's account. Never ask the person for a Box password, a
developer token or a client secret, and never run `box login`.

What you do in Box is done as your owner, with what their Box user may do:
a file you upload or a comment you write shows their name.

## Connected?

```bash
BOX=https://api.box.com/2.0
fragment api box GET $BOX/users/me --param fields=id,name,login
```

- An answer with `login`: connected, go on.
- `not_connected`: your owner has not connected Box, or must authorize it
  again. Ask them to connect it in Settings, under Connections, then try
  again: your computer is given it within seconds.
- `forbidden`: your owner left Box out of your connections. Say so, and stop.
- A 401 from Box (`error` empty, or `unauthorized`): Box refused the token
  (the person removed the app's access, say). Ask them to reconnect Box.

The answer is Box's JSON on stdout. A refusal is one line of JSON on
stderr, `{error, status, message, hint}`, and exit 1. Box's own codes come
through as Box names them (`not_found`); where Box sends no body, `error`
is empty: go by `status`. The token goes to `api.box.com` (the API) and
`upload.box.com` (uploads) only.

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
fragment api box GET $BOX/folders/0/items --param fields=type,id,name,size,modified_at --param usemarker=true --param limit=100
# one item's details (folders/<id> for a folder); --param fields=shared_link for its shared link
fragment api box GET $BOX/files/456
# names, descriptions, contents, comments and tags; every --param but query is optional
fragment api box GET $BOX/search --param 'query=quarterly plan' --param type=file --param file_extensions=pdf,docx \
  --param ancestor_folder_ids=123 --param fields=type,id,name,size,modified_at,parent --param limit=30 --param offset=0
fragment api box GET $BOX/files/456/comments --param fields=message,created_by,created_at --param limit=100 --param offset=0
```

- Box names every file, folder and comment by a number; `0` is the
  person's All Files. Find one by search, or by listing its folder.
- A folder's items page by marker: add `--param marker=<next_marker>` from
  the last answer, until it names none. Search (at most 200 a page) and a
  file's comments page by offset: the next is `offset` plus `limit`, while
  that is under `total_count`, however few entries a page holds.

## Downloading

Box answers a download with a redirect to a signed address on another host
(`dl.boxcloud.com`). `fragment api` never follows it: its refusal
(`redirected`) names the address, which an ordinary tool fetches, no token:

```bash
fragment api box GET $BOX/files/456 --param fields=name,size
URL=$(fragment api box GET $BOX/files/456/content 2>&1 | sed -n 's/.*a redirect to \(https:[^ ]*\), which.*/\1/p')
wget -O ./plan.pdf "$URL"
```

- Never fetch the signed address with `fragment api`, nor with an
  `Authorization` header: the token goes to Box's two hosts alone. The
  address lasts minutes: ask for a fresh one each time.
- An empty `$URL`: run the call alone and read what it says. No output and
  exit 0 is Box still preparing a file just uploaded: try again shortly.
- Hold the saved file's size to the `size` Box stated. A download that
  broke off leaves a short file: remove it, and download again.

## Changing

```bash
fragment api box POST $BOX/folders --body '{"name": "Q4 plans", "parent": {"id": "0"}}'
fragment api box PUT $BOX/files/456 --body '{"parent": {"id": "789"}}'   # move; folders/<id> for a folder
fragment api box POST $BOX/folders/123/copy --body '{"parent": {"id": "0"}, "name": "Q4 plans (copy)"}'
fragment api box POST $BOX/comments --body '{"item": {"type": "file", "id": "456"}, "message": "Looks good to me."}'
fragment api box PUT $BOX/files/456 --param fields=shared_link --body '{"shared_link": {"access": "company"}}'
fragment api box PUT $BOX/files/456 --param fields=shared_link --body '{"shared_link": null}'   # remove it
```

A shared link's `access`: `open` is anyone with the link, `company` your
owner's company, `collaborators` those already invited; `{}` takes the
account's default. Any other call (versions, collaborations, tasks, trash)
is the same command with another path under `/2.0/`. Box's API reference
is at https://developer.box.com/reference/.

## Uploading

Box takes a new file as a multipart form on `upload.box.com`: an
`attributes` part (its name and folder), then the file. `fragment api`
sends one body as it is given, so write the form to a file and send that:

```bash
B="form-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
{ printf '%s\r\nContent-Disposition: form-data; name="attributes"\r\n\r\n%s\r\n' "--$B" '{"name": "notes.txt", "parent": {"id": "123"}}'
  printf '%s\r\nContent-Disposition: form-data; name="file"; filename="file"\r\nContent-Type: application/octet-stream\r\n\r\n' "--$B"
  cat ./notes.txt
  printf '\r\n%s\r\n' "--$B--"; } > /tmp/box-upload.form
fragment api box POST https://upload.box.com/api/2.0/files/content --body @/tmp/box-upload.form --content-type "multipart/form-data; boundary=$B"
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

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Box in Settings → Connections |
| `forbidden` | Your owner left Box off this agent's connections |
| 401 from Box | Your owner reconnects Box in Settings → Connections |
| 403, `access_denied_insufficient_permissions` | Read only (they allow changes in Settings → Connections), or their Box user may not do this to this item |
| `not_found` | The id is wrong, or their Box user cannot see it |
| `item_name_in_use` | Pick another name, or another folder |
| `rate_limit_exceeded` (429) | Wait, then ask again; ask for less at a time |
| `redirected` | A download's answer: fetch the address it names with no token (see Downloading). From any other call: say so, and stop |
