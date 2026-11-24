---
name: microsoft-finite
description: Microsoft 365 (Outlook mail and calendar, OneDrive files, contacts) through the person's connected Microsoft account, with `fragment connection call microsoft` and Microsoft Graph. No OAuth setup, no key on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Microsoft, Microsoft 365, Outlook, Email, Calendar, OneDrive, Files, Contacts]
  fragment:
    connection: microsoft
---

# Microsoft 365

Your owner's Microsoft account (work, school or personal): Outlook mail
and calendar, OneDrive files and contacts, through Microsoft Graph. Your
computer holds no Microsoft credential:
`fragment connection call microsoft` sends the connection's placeholder
for you, and the computer swaps in a short-lived token for your owner's
account. There is no app to register and no key to paste: never ask the
person for a Microsoft password, a client secret or a token, and never run
`az login` or `m365 login`.

What you do is done as your owner: mail you send comes from them, and an
event you create is on their calendar.

## Connected?

```bash
G="https://graph.microsoft.com/v1.0"
fragment connection call microsoft GET "$G/me" --param '$select=displayName,mail,userPrincipalName'
```

A `displayName`: connected, go on. The token goes to `graph.microsoft.com`
only.

The answer is Graph's JSON on stdout; a list is `{"value": […]}`. A refusal
is one line of JSON on stderr and exit 1. Do what its `hint` says; its
`body` is Graph's own answer: `error.code` (`ErrorItemNotFound`,
`ErrorAccessDenied`) and `error.message`.

On an older computer: `fragment connection call` came with an image newer
than a computer awake since before it. If the command is `unrecognized`,
your computer takes that image when it next restarts. Until then, send the
same calls with
`curl -H "Authorization: Bearer $MICROSOFT_OAUTH_ACCESS_TOKEN"`: the
computer swaps that the same way.

## Read only

Your owner picks, as they connect Microsoft, whether you may only read
(mail, calendar, files and contacts) or read and write (also send and
change mail, create and change events, files and contacts). Connected read
only, Microsoft refuses every change (`ErrorAccessDenied` or
`accessDenied`, 403). If a change is refused so while reads work, tell your owner, in plain
words, that they may have connected Microsoft read only, and that they can
allow changes in Settings → Connections → Allow changes. A work account's
admin may also forbid it: say that it may be their organization's setting.
Do not retry it until they say they have; go on reading.

## Mail

```bash
MSG='$select=id,subject,from,toRecipients,receivedDateTime,isRead,bodyPreview,webLink'

# a folder, newest first: inbox, sentitems, drafts, archive, deleteditems, or a folder's id
fragment connection call microsoft GET "$G/me/mailFolders/inbox/messages" --param '$top=10' --param "$MSG" --param '$orderby=receivedDateTime desc'

# unread only: beside $orderby, a $filter names the ordered property first
fragment connection call microsoft GET "$G/me/mailFolders/inbox/messages" --param '$top=10' --param "$MSG" --param '$orderby=receivedDateTime desc' \
  --param '$filter=receivedDateTime ge 1900-01-01T00:00:00Z and isRead eq false'

# search every folder (no $orderby or $filter beside $search)
fragment connection call microsoft GET "$G/me/messages" --param '$search="invoice from acme"' --param '$top=10' --param "$MSG"

# one message, with its body
fragment connection call microsoft GET "$G/me/messages/<id>" --param '$select=subject,from,toRecipients,ccRecipients,receivedDateTime,body'

# send (contentType: Text or HTML): answers nothing (202) when it is accepted
fragment connection call microsoft POST "$G/me/sendMail" --body '{"message": {"subject": "Notes", "body": {"contentType": "Text", "content": "Here are the notes."},
  "toRecipients": [{"emailAddress": {"address": "ada@example.com"}}], "ccRecipients": []}, "saveToSentItems": true}'
```

## Calendar

```bash
# the next 7 days, recurring events expanded; times answer in UTC
fragment connection call microsoft GET "$G/me/calendarView" --param "startDateTime=$(date -u +%FT%TZ)" --param "endDateTime=$(date -u -d '+7 days' +%FT%TZ)" \
  --param '$top=50' --param '$select=id,subject,start,end,location,organizer,attendees,webLink' --param '$orderby=start/dateTime'

# create: local times and their time zone (a Windows name, "Eastern Standard Time", or an IANA one)
fragment connection call microsoft POST "$G/me/events" --body '{"subject": "Sync", "start": {"dateTime": "2026-11-12T15:00:00", "timeZone": "America/New_York"},
  "end": {"dateTime": "2026-11-12T15:30:00", "timeZone": "America/New_York"}, "attendees": [{"emailAddress": {"address": "ada@example.com"}, "type": "required"}]}'
```

## Files and contacts

```bash
FILE='$select=id,name,size,lastModifiedDateTime,webUrl,folder,file,parentReference'

# OneDrive's root, or a folder by its path (between root:/ and :/)
fragment connection call microsoft GET "$G/me/drive/root/children" --param '$top=50' --param "$FILE"
fragment connection call microsoft GET "$G/me/drive/root:/Documents/Plans:/children" --param '$top=50' --param "$FILE"

# search by name and content (a ' in the words is written twice), and one item
fragment connection call microsoft GET "$G/me/drive/root/search(q='quarterly plan')" --param '$top=25' --param "$FILE"
fragment connection call microsoft GET "$G/me/drive/items/<id>" --param "$FILE"

# contacts
fragment connection call microsoft GET "$G/me/contacts" --param '$top=50' --param '$select=id,displayName,emailAddresses,mobilePhone,companyName'
```

A file's content is not on `graph.microsoft.com`: Graph answers a redirect
to a signed, short-lived URL on another host. `fragment connection call`
follows it there without the token, and `--out` saves the file whole or
not at all:

```bash
fragment connection call microsoft GET "$G/me/drive/items/<id>/content" --header 'Accept: */*' --out plan.docx
```

- A list answers one page (`$top` of them). Where there are more, the
  answer's `@odata.nextLink` is the next page's whole address: GET it as it
  is, with no `--param`.
- An id is used as answered. A message's id changes when it moves to
  another folder: read it again rather than keeping it.
- Any other Graph resource (`/me/mailFolders`, a PATCH or DELETE of an
  item, `/beta/` in place of `/v1.0/`) is the same call with another path.
  Graph's docs are at https://learn.microsoft.com/graph/api/overview.

## Rules

1. **Confirm before you send mail, or create, change or delete anything**
   for your owner, unless they asked for exactly that: show who it goes to,
   and what it says. An event with attendees mails each an invitation.
2. **Never delete** unless asked for that item by name.
3. **Ask for what you need**: `$top` and `$select` keep answers small, and
   Graph limits requests (429): wait, then ask for less at a time.

## Troubleshooting

| Graph's `error.code` | Fix |
|---------|-----|
| `InvalidAuthenticationToken` | Microsoft refused the token (the person removed the app's access, say): your owner reconnects Microsoft in Settings → Connections |
| `ErrorAccessDenied`, `accessDenied` | Read only (they allow changes in Settings → Connections), or their organization does not allow it |
| `ErrorItemNotFound`, `itemNotFound` | The id or path is wrong, or not in this account |
| `InefficientFilter` | Name the `$orderby` property first in the `$filter`, or drop the `$orderby` |
