---
name: salesforce-finite
description: Salesforce records (accounts, contacts, leads, opportunities, cases, custom objects) through the person's connected Salesforce org, with `fragment api salesforce`, SOQL, SOSL and the REST API. No OAuth setup, no key on the computer.
version: 2.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Salesforce, CRM, Accounts, Contacts, Leads, Opportunities, SOQL]
  fragment:
    connection: salesforce
---

# Salesforce

Your owner's Salesforce org: its accounts, contacts, leads, opportunities,
cases and custom objects. Your computer holds no Salesforce credential:
`fragment api salesforce` sends the connection's placeholder for you, and
the computer swaps in a short-lived token for your owner's account. Never
ask the person for a password, a security token or a session id, and never
run `sf org login`.

What you do in Salesforce is done as your owner, with what their Salesforce
user may do: a record you create or change shows their name.

## First: the org's host

The token goes to `login.salesforce.com` (who the connection is), and to
the org's own host, `<mydomain>.my.salesforce.com`, alone. Learn that host
once, and use it for every call after:

```bash
fragment api salesforce GET https://login.salesforce.com/services/oauth2/userinfo
# → {"name": …, "email": …, "organization_id": …, "urls": {"rest": "https://acme.my.salesforce.com/services/data/v{version}/", …}}
SF="https://acme.my.salesforce.com/services/data/v63.0"   # the host in urls.rest, and the API version
```

- An answer with `urls`: connected, go on.
- `urls.rest` names no host under `my.salesforce.com` (an org without My
  Domain): say so, and stop.
- `forbidden` on a call to an org's host: it went to another org's (the
  message names the connected one's). Use that host.

The answer is Salesforce's JSON on stdout. A refusal is one line of JSON on
stderr and exit 1. Do what its `hint` says; its `body` is Salesforce's own
answer, a list of `{errorCode, message, fields?}` (`MALFORMED_QUERY`,
`INVALID_FIELD`, `REQUIRED_FIELD_MISSING`).

## Reading

```bash
# SOQL: --param encodes the query for you
fragment api salesforce GET "$SF/query" --param 'q=SELECT Id, Name, Industry FROM Account ORDER BY LastModifiedDate DESC LIMIT 10'

# SOSL: search across objects
fragment api salesforce GET "$SF/search" --param 'q=FIND {Acme} IN NAME FIELDS RETURNING Account(Id, Name), Contact(Id, Name, Email)'

# one record, and only the fields you need
fragment api salesforce GET "$SF/sobjects/Account/001xx000003DGb2AAG" --param fields=Id,Name,Industry,Owner.Name

# an object's fields, types and picklist values (long: it is saved to a file, and the file named)
fragment api salesforce GET "$SF/sobjects/Opportunity/describe"

# every object, and the org's API limits
fragment api salesforce GET "$SF/sobjects"
fragment api salesforce GET "$SF/limits"
```

- SOQL names fields by their API names (`Owner.Name`, `Custom_Field__c`):
  `describe` lists an object's. String literals take single quotes.
- `query` answers `{totalSize, done, records, nextRecordsUrl?}`: the first
  page, up to 2,000 records. For the next, GET the org's host followed by
  `nextRecordsUrl`. Ask for what you need with `WHERE` and `LIMIT` first.
- A record's id is 15 or 18 letters and digits; an object's name its API
  name (`Account`, `Invoice__c`).

## Changing

```bash
# create: answers {id, success, errors}
fragment api salesforce POST "$SF/sobjects/Contact" --body '{"LastName": "Lovelace", "FirstName": "Ada", "Email": "ada@example.com", "AccountId": "001xx000003DGb2AAG"}'

# update: answers nothing (204) when it is done
fragment api salesforce PATCH "$SF/sobjects/Opportunity/006xx000001a2b3AAA" --body '{"StageName": "Closed Won"}'
```

Any other REST resource under `/services/` on the org's host (Reports,
Composite, UI API) is the same call with another path.

## Rules

1. **Confirm before you create, change or delete records** for your owner,
   unless they asked for exactly that: show the object, the record and the
   fields you will write.
2. **Never delete** unless asked for that record by name; never
   mass-update.
3. **Respect the org's API limits**: each org has a daily request
   allowance (`$SF/limits`); query for what you need in one SOQL statement
   rather than many gets.

## Troubleshooting

| Salesforce's `errorCode` | Fix |
|---------|-----|
| `INVALID_SESSION_ID` (401) | Salesforce refused the token (the person revoked the app, or their org's admin did): your owner reconnects Salesforce in Settings → Connections |
| `INSUFFICIENT_ACCESS_OR_READONLY` | Their Salesforce user may not do this: their org's admin grants it |
| `MALFORMED_QUERY`, `INVALID_FIELD` | Check the API names with `describe` |
| `REQUEST_LIMIT_EXCEEDED` | The org's daily allowance is spent (`$SF/limits`): say so, and stop |
