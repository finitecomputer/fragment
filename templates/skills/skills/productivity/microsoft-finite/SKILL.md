---
name: microsoft-finite
description: Microsoft 365 (Outlook mail and calendar, OneDrive files, contacts) through the person's connected Microsoft account, with the bundled helper or Microsoft Graph. No OAuth setup, no key on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Microsoft, Microsoft 365, Outlook, Email, Calendar, OneDrive, Files, Contacts]
---

# Microsoft 365

Your owner's Microsoft account (work, school or personal): Outlook mail and
calendar, OneDrive files and contacts, through Microsoft Graph. Your
computer holds no Microsoft credential: `MICROSOFT_OAUTH_ACCESS_TOKEN` holds
the connection's placeholder (it names you), it goes as a bearer token to
`graph.microsoft.com`, and the computer swaps in a short-lived token for your
owner's account. There is no app to register and no key to paste: never ask
the person for a Microsoft password, a client secret or a token, and never
run `az login` or `m365 login`.

What you do is done as your owner: mail you send comes from them, and an
event you create is on their calendar.

## Connected?

```bash
MSAPI="python3 ${HERMES_SKILL_DIR}/scripts/microsoft_api.py"
$MSAPI check
```

- `{"status": "connected", "name": …, "email": …}`: go on.
- `not_connected`: your owner has not connected Microsoft, or must authorize
  it again (`MICROSOFT_OAUTH_ACCESS_TOKEN` is then unset). Ask them to
  connect it in the shell's Settings, under Connections, then check again:
  your computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left Microsoft out.
  Say so, and stop.
- `invalidauthenticationtoken`: Microsoft refused the token (the person
  removed the app's access, say). Ask them to reconnect Microsoft in
  Settings.

The connection (`microsoft`) is swapped for `graph.microsoft.com` only. A
file's download (`@microsoft.graph.downloadUrl`) is a link of its own on
another host that needs no token: fetch it without one, never with the
placeholder.

## Not allowed

Your owner picks, as they connect Microsoft, the services you may use:
Outlook mail, Outlook calendar, OneDrive and Contacts. A call to one they
left out is refused by Microsoft (403, `erroraccessdenied` or
`accessdenied`). Tell your owner, in plain words, which they didn't allow,
and that they can add it in Settings → Connections → Reconnect to add
(their organization's admin may need to approve it). Go on with what they
did allow.

## Read only

Your owner picks, as they connect Microsoft, whether you may only read
(mail, calendar, files and contacts) or read and write (also send and
change mail, create and change events, files and contacts). Connected read
only, Microsoft refuses every change (`erroraccessdenied`, `accessdenied`,
403). If a change is refused so while reads work, tell your owner, in plain
words, that they may have connected Microsoft read only, and that they can
allow changes in Settings → Connections → Allow changes. A work account's
admin may also forbid it: say that it may be their organization's setting.
Do not retry it until they say they have; go on reading.

## The helper

```bash
$MSAPI mail list --unread --max 10
$MSAPI mail search "invoice from acme"
$MSAPI mail get AAMkAGI2…
$MSAPI mail send --to ada@example.com --subject "Notes" --body "Here are the notes."
$MSAPI events list --days 7
$MSAPI events create --subject "Sync" --start 2026-11-12T15:00 --end 2026-11-12T15:30 --tz "Eastern Standard Time" --attendee ada@example.com
$MSAPI files list Documents
$MSAPI files search "quarterly plan"
$MSAPI files get 01BYE5RZ…
$MSAPI contacts list
$MSAPI request GET "/v1.0/me/mailFolders?\$top=20"   # any Graph call
```

- Mail folders go by their well-known names (`inbox`, `sentitems`,
  `drafts`, `archive`, `deleteditems`) or their ids.
- `events create` takes local times and a time zone (Windows names, such as
  `Eastern Standard Time`, or IANA ones, such as `America/New_York`).
- `request` reaches any Graph resource from `/v1.0/` or `/beta/`; its query
  goes after `?` (quote `$` in the shell), and `--json` is its body.

## Output Format

Every command prints JSON: lists as Graph answers them (`id`, `subject`,
`from`, `receivedDateTime` for mail; `subject`, `start`, `end` for events;
`name`, `webUrl`, `size` for files). A failure is one JSON line on stderr,
`{error, status, message, hint}`, and exit 1; Graph's own error codes come
through lower case (`erroritemnotfound`, `erroraccessdenied`).

## Rules

1. **Confirm before you send mail, or create, change or delete anything**
   for your owner, unless they asked for exactly that: show who it goes to,
   and what it says.
2. **Check the connection before first use** (`$MSAPI check`).
3. **Never delete** unless asked for that item by name.
4. **Ask for what you need**: `--max` and `$select` keep answers small.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Microsoft in Settings → Connections |
| `forbidden` | Your owner left Microsoft off this agent's connections |
| `invalidauthenticationtoken` | Your owner reconnects Microsoft in Settings → Connections |
| `erroraccessdenied`, `accessdenied` | Read only (they allow changes in Settings → Connections), or their organization does not allow it |
| `erroritemnotfound` | The id is wrong, or not in this account |
| `redirected` | Microsoft answered a redirect, which the helper never follows with the token: say so, and stop |
