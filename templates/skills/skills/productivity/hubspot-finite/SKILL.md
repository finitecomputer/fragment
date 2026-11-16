---
name: hubspot-finite
description: HubSpot CRM (contacts, companies, deals, tickets, their associations and notes, and owners) through the person's connected HubSpot account, with `fragment api hubspot` and HubSpot's API. No OAuth setup, no private app token on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [HubSpot, CRM, Contacts, Companies, Deals, Tickets, Sales]
  fragment:
    connection: hubspot
---

# HubSpot

Your owner's HubSpot account: its contacts, companies, deals and tickets,
how they are associated, the notes on them, and the account's owners (who
records are assigned to). Your computer holds no HubSpot credential:
`fragment api hubspot` sends the connection's placeholder for you, and the
computer swaps in a short-lived token for your owner's account. Never ask
the person for a HubSpot password, a private app token or a personal access
key, and never run `hs auth`.

The account is your owner's team's: a record you create or change, or a note
you add, is there for everyone in it to see. The connection reaches the
whole account, not only the records your owner's HubSpot user may see.

## Connected?

```bash
H="https://api.hubapi.com"
fragment api hubspot GET $H/account-info/2026-09/details
# → {"portalId": …, "timeZone": …, "companyCurrency": …, "uiDomain": "app.hubspot.com", …}
```

- An answer with `portalId`: connected, go on.
- `not_connected`: your owner has not connected HubSpot, or must authorize
  it again. Ask them to, in Settings under Connections, then try again.
- `forbidden`: your owner narrowed your connections and left HubSpot out.
  Say so, and stop.
- Status 401: HubSpot refused the token (someone uninstalled the app from
  the HubSpot account, say). Ask them to reconnect HubSpot in Settings.

The token goes to `api.hubapi.com` alone: never call `api.hubspot.com`,
where the placeholder goes as it is and HubSpot refuses it. The answer is
HubSpot's JSON on stdout; a refusal is one line of JSON on stderr, `{error,
status, message, hint}`, and exit 1. Tell HubSpot's own refusals apart by
`status` and `message`: their `error` is often only `error`, or empty.

## Read only

Your owner picks, as they connect HubSpot, whether you may only read
(contacts, companies, deals, tickets, notes and owners) or read and write
(also create and change records, associate them and add notes). Connected
read only, HubSpot refuses every change: status 403, its message about the
scopes it requires. If a change is refused so while reads work, tell your
owner, in plain words, that they may have connected HubSpot read only and
can allow changes in Settings → Connections → Allow changes. Do not retry
it until they say they have; go on reading.

## Reading

```bash
O="$H/crm/objects/2026-09"

# search a type by text (names, emails, phones, domains), by filters, or both
fragment api hubspot POST $O/contacts/search --body '{"query": "ada lovelace", "limit": 10, "properties": ["firstname", "lastname", "email", "company"]}'
fragment api hubspot POST $O/deals/search --body '{"limit": 20, "properties": ["dealname", "amount", "dealstage", "closedate"],
  "filterGroups": [{"filters": [{"propertyName": "dealstage", "operator": "EQ", "value": "closedwon"}, {"propertyName": "amount", "operator": "GT", "value": "10000"}]}],
  "sorts": [{"propertyName": "closedate", "direction": "DESCENDING"}]}'

# one record, and the ids of the records it is associated with
fragment api hubspot GET $O/deals/123456 --param properties=dealname,amount,dealstage,hubspot_owner_id --param associations=contacts,companies

# the companies a contact is associated with: here a type is its id
fragment api hubspot GET $O/0-1/501/associations/0-2 --param limit=100

# the notes on a deal, newest first (associations.contact, .company, .deal or .ticket)
fragment api hubspot POST $O/notes/search --body '{"limit": 10, "properties": ["hs_note_body", "hs_timestamp", "hubspot_owner_id"],
  "filterGroups": [{"filters": [{"propertyName": "associations.deal", "operator": "EQ", "value": "123456"}]}],
  "sorts": [{"propertyName": "hs_timestamp", "direction": "DESCENDING"}]}'

# owners: an owner's id, for hubspot_owner_id
fragment api hubspot GET $H/crm/owners/2026-09 --param email=ada@example.com

# a type's properties and their options; the deal pipelines and their stages
fragment api hubspot GET $H/crm/properties/2026-09/0-1
fragment api hubspot GET $H/crm/pipelines/2026-09/0-3
```

- The types are `contacts`, `companies`, `deals` and `tickets`; their ids
  `0-1`, `0-2`, `0-3` and `0-5`. A record's id is its digits.
- Properties go by their internal names (`dealstage`, `hubspot_owner_id`,
  `lifecyclestage`), not their labels. Name the `properties` you want:
  HubSpot answers only a few otherwise. A deal's stage is its stage's id,
  as the pipelines list them.
- A filter's operator is `EQ`, `NEQ`, `GT`, `GTE`, `LT`, `LTE`, or
  `CONTAINS_TOKEN` (a token it contains, `*` a wildcard). The filters of
  one group are all of them. A value is a string; a date's is its
  milliseconds since 1970, UTC.
- A list answers `paging.next.after` while there is more: pass it as
  `"after"` in a search's body, or `--param after=…` on a GET. A search
  pages at most 200 records.

## Changing

```bash
# create, and update: an empty value clears a property
fragment api hubspot POST $O/contacts --body '{"properties": {"email": "ada@example.com", "firstname": "Ada", "lastname": "Lovelace"}}'
fragment api hubspot PATCH $O/deals/123456 --body '{"properties": {"dealstage": "contractsent", "amount": "12000"}}'

# associate contact 501 with deal 123456 (HubSpot's default association)
fragment api hubspot PUT $O/0-1/501/associations/default/0-3/123456

# add a note to a deal
fragment api hubspot POST $O/notes --body '{"properties": {"hs_timestamp": "2026-11-16T15:04:05Z", "hs_note_body": "Called Ada: they sign Friday."},
  "associations": [{"to": {"id": "123456"}, "types": [{"associationCategory": "HUBSPOT_DEFINED", "associationTypeId": 214}]}]}'
```

- A note needs `hs_timestamp`: now, in UTC. Its `associationTypeId` says
  what it is on: 202 a contact, 190 a company, 214 a deal, 228 a ticket.
  `hubspot_owner_id` among its properties says which owner it is by.

Any other HubSpot API on `api.hubapi.com` is the same call with another
path. Use the 2026-09 paths (`/crm/objects/2026-09/…`), as HubSpot's docs
at https://developers.hubspot.com/docs/api-reference name them.

## Rules

1. **Confirm before you create or change a record, associate records or add
   a note** for your owner, unless they asked for exactly that: show the
   record and what you will write.
2. **Never delete** unless asked for that record by name; never change many
   records at once.
3. **Respect rate limits**: HubSpot allows about 110 requests every 10
   seconds for an account, and 5 searches a second. Search for what you
   need, with its `properties`, rather than getting records one by one.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects HubSpot in Settings → Connections |
| `forbidden` | Your owner left HubSpot off this agent's connections, or the call went to a host other than `api.hubapi.com` |
| Status 401 | Your owner reconnects HubSpot in Settings → Connections |
| Status 403, scopes required | Read only (they allow changes in Settings → Connections), or a part of HubSpot the connection does not reach |
| Status 400 | A property's name or value is wrong: list the type's properties |
| Status 404 | The id is wrong, or not in this account |
| Status 429 | Wait a few seconds, then ask for less at a time |
| `redirected` | HubSpot answered a redirect, which is never followed with the token: say so, and stop |
