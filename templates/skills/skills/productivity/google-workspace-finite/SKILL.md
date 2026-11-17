---
name: google-workspace-finite
description: Gmail, Calendar, Drive, Contacts, Sheets, Docs, and Slides through the person's connected Google account, with `gws` (Google's Workspace CLI, in your computer's image) or `fragment api google`. No OAuth setup, no keys on the computer.
version: 3.0.0
author: Nous Research (rewritten for fragment)
license: MIT
metadata:
  hermes:
    tags: [Google, Gmail, Calendar, Drive, Sheets, Docs, Slides, Contacts, Email]
    homepage: https://github.com/NousResearch/hermes-agent
  fragment:
    connection: google
---

# Google Workspace

Gmail, Calendar, Drive, Contacts, Sheets, Docs, and Slides through your
owner's connected Google account. Your computer holds no Google credential:
`GOOGLE_OAUTH_ACCESS_TOKEN` holds the connection's placeholder (it names
you), it goes as a bearer token to Google's own hosts, and the computer
swaps in a short-lived token for your owner's account. There is no OAuth
setup to run, no client secret and no token file: never run `gws auth
setup` or `gws auth login`, never ask the person for a credential, and never
create a Google Cloud project for them.

**Use `gws` first.** It is Google's Workspace CLI
(github.com/googleworkspace/cli), on your PATH, and it takes the
placeholder as its access token by itself: every Google API it knows, as
`gws <service> <resource> <method>`, JSON out. Where `gws` is not on your
PATH, `fragment api google` calls any Google REST API the same way
(below, "Without gws").

Hermes' standalone `google-workspace` skill is omitted from this image:
its OAuth setup and token-file helpers are for a different authentication
contract. Use this skill and `gws` here.

For Google Docs tasks, load `skill_view("gws-docs")` if not already loaded
for Docs command and schema discovery. This skill owns the connection
and authentication.

For Google Slides tasks, load `skill_view("gws-slides")` if not already
loaded for Slides command and schema discovery. This skill owns the
connection and authentication.

## Connected?

```bash
fragment api google GET https://www.googleapis.com/oauth2/v2/userinfo
```

- `{"email": …}`: connected, as that account. Go on.
- `not_connected`: your owner has not connected Google, or must authorize
  it again (`GOOGLE_OAUTH_ACCESS_TOKEN` is then unset, and `gws` says it
  has no credentials, exit 2). Ask them to connect it in the shell's
  Settings, under Connections, then check again: your computer is given it
  within seconds.
- `forbidden`: your owner narrowed your connections and left Google out. Say
  so, and stop.

The deployment's Google connection (`google`) is swapped for the hosts
`gmail.googleapis.com`, `www.googleapis.com` (Drive, Calendar),
`people.googleapis.com`, `sheets.googleapis.com`, `docs.googleapis.com` and
`slides.googleapis.com` only. An API on any other host (Tasks, Chat, Forms,
Apps Script, and the `gws workflow` helpers that use them) gets the
placeholder itself, which Google refuses (401): do not use those. The
scopes the connection asks for, read only and read and write, are in
`references/google-workspace-access.json`.

## Not allowed

Your owner picks, as they connect Google, whether you may only read
(search and read mail, files, sheets, docs, calendars and contacts) or
read and write (also send and change mail, and create and change files,
sheets, docs and events). Connected read only, every change is refused by
Google with a 403 like the ones below: tell your owner, in plain words,
that they connected Google read only, and that they can allow changes in
Settings → Connections → Allow changes. Do not retry it until they say they
have.

Your owner may have allowed only part of Google when they connected it:
Google's consent screen lets them untick Gmail, Drive, Sheets and the rest
one by one, and the connection holds what they left ticked. A call outside
it is refused by Google with a 403: `insufficient authentication scopes`,
`ACCESS_TOKEN_SCOPE_INSUFFICIENT` or `insufficientPermissions`. Tell your
owner, in plain words, that they didn't allow that part of Google (say
which: Gmail, Drive, Sheets, Docs, Calendar or Contacts) when connecting
it, and that they can add it in Settings → Connections → Reconnect to add.
The connection asks for no Slides permission of its own (Google allows
Slides with Drive's), so a Slides call refused this way is Drive left
unticked: say Drive. Do not retry the call until they say they have; go on
with what they did allow.

## gws

```bash
gws gmail +triage                                   # unread inbox: sender, subject, date
gws gmail users messages list --params '{"userId": "me", "q": "is:unread newer_than:1d", "maxResults": 10}'
gws gmail users messages get --params '{"userId": "me", "id": "MESSAGE_ID", "format": "full"}'
gws gmail +send --to user@example.com --subject "Hello" --body "Message text"
gws gmail +reply --message-id MESSAGE_ID --body "Thanks, that works for me."
gws calendar +agenda                                # upcoming events, in the account's timezone
gws calendar +insert --summary "Standup" --start 2026-03-01T10:00:00-06:00 --end 2026-03-01T10:30:00-06:00
gws drive files list --params "{\"q\": \"name contains 'report'\", \"pageSize\": 10}"
gws drive +upload ./report.pdf
gws sheets +read --spreadsheet SHEET_ID --range 'Sheet1!A1:D10'
gws sheets spreadsheets values append --params '{"spreadsheetId": "SHEET_ID", "range": "Sheet1!A1", "valueInputOption": "USER_ENTERED"}' --json '{"values": [["Name", "Score"], ["Alice", 95]]}'
gws docs documents get --params '{"documentId": "DOC_ID"}'
gws docs +write --document DOC_ID --text "A line to append"
gws slides presentations get --params '{"presentationId": "DECK_ID"}'
gws slides presentations create --json '{"title": "Q4 review"}'
gws slides presentations batchUpdate --params '{"presentationId": "DECK_ID"}' --json '{"requests": [{"createSlide": {"slideLayoutReference": {"predefinedLayout": "TITLE_AND_BODY"}}}]}'
gws people people connections list --params '{"resourceName": "people/me", "personFields": "names,emailAddresses", "pageSize": 20}'
```

- `gws <service> --help` lists a service's methods and its `+` helpers;
  `gws schema <service>.<resource>.<method>` shows what a method takes and
  answers.
- `--dry-run` shows the request without sending it; `--page-all` follows
  pages (one JSON line a page).
- Quote a Sheets range in single quotes: `!` is the shell's.
- Exit 2 is an auth error: check the connection (above).

## References

- `references/gmail-search-syntax.md`: Gmail search operators (is:unread, from:, newer_than:, etc.)
- `references/google-workspace-access.json`: the scopes the connection asks for, read only and read and write

`gws docs +write` appends to a doc, and `gws slides presentations
batchUpdate` edits a deck (`gws schema
slides.presentations.batchUpdate` shows its requests). Neither uses Apps
Script.

## Login emails

When the human explicitly asks you to retrieve or use a login code for the
current task, you may read the connected mailbox and use the newest
matching message. Verify the connected address, service/sender, and
freshness; do not echo the token. Ask the human only when mailbox access
fails or the match is ambiguous.

## Without gws

Where `gws` is not on your PATH, the same APIs with `fragment api google
<METHOD> <url>`: it sends the connection's placeholder for you, `--param`
encodes a query parameter, `--body` is the JSON sent, and the answer is
Google's JSON. A refusal is one line of JSON on stderr and exit 1: its
`hint` says what to tell your owner, and its `body` is Google's own answer
(`error.status`, `error.message`).

```bash
G="fragment api google"
# Gmail: search (ids), then read one; --param encodes the query
$G GET https://gmail.googleapis.com/gmail/v1/users/me/messages --param 'q=is:unread newer_than:1d' --param maxResults=10
$G GET https://gmail.googleapis.com/gmail/v1/users/me/messages/MESSAGE_ID --param format=full
# Gmail: send. `raw` is the whole message (RFC 2822), base64url
RAW=$(printf 'To: user@example.com\r\nSubject: Hello\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMessage text' | base64 | tr -d '\n=' | tr '+/' '-_')
$G POST https://gmail.googleapis.com/gmail/v1/users/me/messages/send --body "{\"raw\": \"$RAW\"}"
# Calendar: the next events, and a new one (times with their offset)
$G GET https://www.googleapis.com/calendar/v3/calendars/primary/events --param timeMin=2026-03-01T00:00:00Z --param singleEvents=true --param orderBy=startTime --param maxResults=10
$G POST https://www.googleapis.com/calendar/v3/calendars/primary/events --body '{"summary": "Standup", "start": {"dateTime": "2026-03-01T10:00:00-06:00"}, "end": {"dateTime": "2026-03-01T10:30:00-06:00"}}'
# Drive: search, and download a file whole
$G GET https://www.googleapis.com/drive/v3/files --param "q=name contains 'report'" --param pageSize=10 --param 'fields=files(id,name,mimeType,modifiedTime,webViewLink)'
$G GET https://www.googleapis.com/drive/v3/files/FILE_ID --param alt=media --out report.pdf
# Sheets: read a range, append rows
$G GET "https://sheets.googleapis.com/v4/spreadsheets/SHEET_ID/values/Sheet1!A1:D10"
$G POST "https://sheets.googleapis.com/v4/spreadsheets/SHEET_ID/values/Sheet1!A:C:append" --param valueInputOption=USER_ENTERED --body '{"values": [["new", "row", "data"]]}'
# Docs, Slides, Contacts
$G GET https://docs.googleapis.com/v1/documents/DOC_ID
$G GET https://slides.googleapis.com/v1/presentations/DECK_ID
$G GET https://people.googleapis.com/v1/people/me/connections --param personFields=names,emailAddresses,phoneNumbers --param pageSize=20
```

Google's own client libraries, given `GOOGLE_OAUTH_ACCESS_TOKEN` as their
access token, work the same way: the computer swaps the token in.

## Rules

1. **Never send email, share files, accept invites, or create/delete events
   without confirming with the user first.** Show the draft content and ask
   for approval.
2. **Check the connection before first use** (above, "Connected?").
3. **Use the Gmail search syntax reference** for complex queries: load it with `skill_view("google-workspace-finite", file_path="references/gmail-search-syntax.md")`.
4. **Calendar times must include timezone**: always ISO 8601 with offset (e.g., `2026-03-01T10:00:00-06:00`) or UTC (`Z`).
5. **Respect rate limits**: avoid rapid-fire sequential API calls. Batch reads when possible.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected`, or `gws` exits 2 | Your owner connects Google in Settings → Connections |
| `401` from a Google API | That API's host is not one the connection is swapped for: it is not offered |
| `forbidden` | Your owner left Google off this agent's connections |
| A 403 `insufficient authentication scopes` | Your owner didn't allow that part of Google when connecting it: they add it in Settings → Connections → Reconnect to add (above, "Not allowed") |
| A 403 `SERVICE_DISABLED` ("… has not been used in project … or it is disabled"), or `Access Not Configured` | That API is not enabled for the deployment's Google client: tell the person, and stop |
