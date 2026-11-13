---
name: hubspot-finite
description: HubSpot CRM (contacts, companies, deals, tickets, their associations and notes, and owners) through the person's connected HubSpot account, with the bundled helper or HubSpot's API. No OAuth setup, no private app token on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [HubSpot, CRM, Contacts, Companies, Deals, Tickets, Sales]
---

# HubSpot

Your owner's HubSpot account: its contacts, companies, deals and tickets,
how they are associated, the notes on them, and the account's owners (who
records are assigned to). Your computer holds no HubSpot credential:
`HUBSPOT_OAUTH_ACCESS_TOKEN` holds the connection's placeholder (it names
you), it goes as a bearer token to `api.hubapi.com`, and the computer swaps
in a short-lived token for your owner's account. There is no app to make and
no key to paste: never ask the person for a HubSpot password, a private app
token or a personal access key, and never run `hs auth`.

The account is your owner's team's: a record you create or change, or a note
you add, is there for everyone in it to see. The connection reaches the
whole account, not only the records your owner's HubSpot user may see.

## Connected?

```bash
HSAPI="python3 ${HERMES_SKILL_DIR}/scripts/hubspot_api.py"
$HSAPI check
```

- `{"status": "connected", "portal_id": …, "ui_domain": …}`: go on.
- `not_connected`: your owner has not connected HubSpot, or must authorize
  it again (`HUBSPOT_OAUTH_ACCESS_TOKEN` is then unset). Ask them to connect
  it in the shell's Settings, under Connections, then check again: your
  computer is given it within seconds.
- `forbidden`: your owner narrowed your connections and left HubSpot out.
  Say so, and stop.
- `invalid_authentication`: HubSpot refused the token (someone uninstalled
  the app from the HubSpot account, say). Ask them to reconnect HubSpot in
  Settings.

The connection (`hubspot`) is swapped for `api.hubapi.com` only: call
HubSpot's API there, never at `api.hubspot.com`, where the placeholder goes
as it is and HubSpot refuses it.

## Read only

Your owner picks, as they connect HubSpot, whether you may only read
(contacts, companies, deals, tickets, notes and owners) or read and write
(also create and change records, associate them and add notes). Connected
read only, HubSpot refuses every change (403 `missing_scopes`, its message
naming a `.write` scope it needs). If a change is refused so while reads
work, tell your owner, in plain words, that they may have connected HubSpot
read only, and that they can allow changes in Settings → Connections →
Allow changes. Do not retry it until they say they have; go on reading.

## The helper

```bash
$HSAPI search contacts "ada lovelace"                       # text in names, emails, phones, companies
$HSAPI search deals --where dealstage=closedwon --where "amount>10000" --sort closedate --desc --max 20
$HSAPI search contacts --where "email~*@acme.com" --properties firstname,lastname,email
$HSAPI search companies --where domain=acme.com --properties name,domain,industry
$HSAPI get deals 123456 --associations contacts,companies
$HSAPI create contacts --set email=ada@example.com --set firstname=Ada --set lastname=Lovelace
$HSAPI update deals 123456 --set dealstage=contractsent --set amount=12000
$HSAPI associations contacts 501 companies                  # the companies a contact is associated with
$HSAPI associate contacts 501 deals 123456                  # HubSpot's default association
$HSAPI notes deals 123456 --max 10                          # newest first
$HSAPI note deals 123456 --body "Called Ada: they sign Friday."
$HSAPI owners --email ada@example.com                       # an owner's id, for hubspot_owner_id
$HSAPI request GET /crm/pipelines/2026-09/0-3               # any HubSpot API call: here, deal stages
```

- The types are `contacts`, `companies`, `deals` and `tickets`; a record's
  id is its digits (`hs_object_id`).
- Properties go by their internal names (`dealstage`, `hubspot_owner_id`,
  `lifecyclestage`), not their labels. An account's own, and each one's
  options: `request GET /crm/properties/2026-09/0-1` (contacts; `0-2`
  companies, `0-3` deals, `0-5` tickets). A deal's stage is its stage's id,
  as `request GET /crm/pipelines/2026-09/0-3` lists them.
- `--where` is `property=value`, or `!=`, `>`, `>=`, `<`, `<=`, and `~` for
  a token it contains (`*` a wildcard); several are all of them. A date's
  value is its milliseconds since 1970, UTC.
- A list answers `after` while there is more: pass it as `--after`.
- `request` reaches any HubSpot API on `api.hubapi.com`, its query after `?`,
  and `--json` is its body. Use the 2026-09 paths (`/crm/objects/2026-09/…`),
  as HubSpot's docs at https://developers.hubspot.com/docs/api-reference
  name them.

## Output Format

Every command prints JSON: records as HubSpot answers them (`id`,
`properties`, `createdAt`, `updatedAt`, and `associations` where asked);
`search`, `notes`, `associations` and `owners` answer `{results, total?,
after}`. A failure is one JSON line on stderr, `{error, status, message,
hint}`, and exit 1; HubSpot's own categories come through lower case
(`missing_scopes`, `validation_error`, `object_not_found`, `rate_limit`).

## Rules

1. **Confirm before you create or change a record, associate records or add
   a note** for your owner, unless they asked for exactly that: show the
   record and what you will write.
2. **Check the connection before first use** (`$HSAPI check`).
3. **Never delete** unless asked for that record by name; never change many
   records at once.
4. **Respect rate limits**: HubSpot allows about 110 requests every 10
   seconds for an account, and 5 searches a second. Search for what you
   need, with `--properties`, rather than getting records one by one.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects HubSpot in Settings → Connections |
| `forbidden` | Your owner left HubSpot off this agent's connections |
| `invalid_authentication`, `expired_authentication` | Your owner reconnects HubSpot in Settings → Connections |
| `missing_scopes` | Read only (they allow changes in Settings → Connections), or a part of HubSpot the connection does not reach |
| `validation_error` | A property's name or value is wrong: list the type's properties with `request` |
| `object_not_found` | The id is wrong, or not in this account |
| `rate_limit` | Wait a few seconds, then ask for less at a time |
| `redirected` | HubSpot answered a redirect, which the helper never follows with the token: say so, and stop |
