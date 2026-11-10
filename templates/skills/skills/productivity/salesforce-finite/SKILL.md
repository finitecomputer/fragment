---
name: salesforce-finite
description: Salesforce records (accounts, contacts, leads, opportunities, cases, custom objects) through the person's connected Salesforce org, with SOQL, SOSL and the REST API through the bundled helper. No OAuth setup, no key on the computer.
version: 1.0.0
author: Finite
license: MIT
metadata:
  hermes:
    tags: [Salesforce, CRM, Accounts, Contacts, Leads, Opportunities, SOQL]
---

# Salesforce

Your owner's Salesforce org: its accounts, contacts, leads, opportunities,
cases and custom objects. Your computer holds no Salesforce credential:
`SALESFORCE_ACCESS_TOKEN` holds the connection's placeholder (it names you),
it goes as a bearer token, and the computer swaps in a short-lived token for
your owner's account. Never ask the person for a password, a security token
or a session id, and never run `sf org login`.

What you do in Salesforce is done as your owner, with what their Salesforce
user may do: a record you create or change shows their name.

## Where the token goes

To `login.salesforce.com` (who the connection is), and to the org's own
host, `<mydomain>.my.salesforce.com`, alone: the computer sends the token to
a host under `my.salesforce.com` only if it is the connected org's. The
helper learns that host from the org's identity (`check` shows it as
`instance`); a call to another org's host is refused (403, naming the
connected org's).

## Connected?

```bash
SFAPI="python3 ${HERMES_SKILL_DIR}/scripts/salesforce_api.py"
$SFAPI check
```

- `{"status": "connected", "email": …, "instance": "https://acme.my.salesforce.com"}`: go on.
- `not_connected`: your owner has not connected Salesforce, or must
  authorize it again (`SALESFORCE_ACCESS_TOKEN` is then unset). Ask them to
  connect it in the shell's Settings, under Connections, then check again.
- `forbidden`: your owner narrowed your connections and left Salesforce out.
  Say so, and stop.
- `invalid_session_id`: Salesforce refused the token (the person revoked the
  app, or their org's admin did). Ask them to reconnect Salesforce in Settings.

## The helper

```bash
$SFAPI query "SELECT Id, Name, Industry FROM Account ORDER BY LastModifiedDate DESC LIMIT 10"
$SFAPI query "SELECT Id, Name, StageName, Amount, CloseDate FROM Opportunity WHERE IsClosed = false" --all
$SFAPI search "FIND {Acme} IN NAME FIELDS RETURNING Account(Id, Name), Contact(Id, Name, Email)"
$SFAPI get Account 001xx000003DGb2AAG --fields Id,Name,Industry,Owner.Name
$SFAPI describe Opportunity                       # its fields, types and picklist values
$SFAPI objects                                    # every object you may query
$SFAPI create Contact --json '{"LastName": "Lovelace", "FirstName": "Ada", "Email": "ada@example.com", "AccountId": "001xx000003DGb2AAG"}'
$SFAPI update Opportunity 006xx000001a2b3AAA --json '{"StageName": "Closed Won"}'
$SFAPI request GET /services/data/v63.0/limits    # any REST call on the org's host
```

- SOQL names fields by their API names (`Owner.Name`, `Custom_Field__c`):
  `describe` lists an object's, and its picklists' values. Quote the query
  in double quotes; its string literals take single quotes.
- `query` answers the first page (up to 2,000 records); `--all` follows
  every page. Ask for what you need with `WHERE` and `LIMIT` first.
- A record's id is 15 or 18 letters and digits; an object's name its API
  name (`Account`, `Invoice__c`).
- `request` reaches any REST resource from `/services/` on the org's host
  (Reports, Composite, UI API): `--json` is its body.

## Output Format

Every command prints JSON. `query` answers `{totalSize, done, records}`,
each record with its `attributes` and the fields asked for; `create`
answers `{id, success, errors}`; `update` `{status: "updated"}`. A failure
is one JSON line on stderr, `{error, status, message, hint}`, and exit 1;
Salesforce's own error codes come through lower case (`malformed_query`,
`invalid_field`, `required_field_missing`, `insufficient_access_or_readonly`).

## Rules

1. **Confirm before you create, change or delete records** for your owner,
   unless they asked for exactly that: show the object, the record and the
   fields you will write.
2. **Check the connection before first use** (`$SFAPI check`).
3. **Never delete** unless asked for that record by name; never mass-update.
4. **Respect the org's API limits**: each org has a daily request allowance
   (`request GET /services/data/v63.0/limits`); query for what you need in
   one SOQL statement rather than many gets.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `not_connected` | Your owner connects Salesforce in Settings → Connections |
| `forbidden` | Your owner left Salesforce off this agent's connections, or the call went to another org's host |
| `invalid_session_id` | Your owner reconnects Salesforce in Settings → Connections |
| `insufficient_access_or_readonly` | Their Salesforce user may not do this: their org's admin grants it |
| `no_org_host` | The org's identity names no host under my.salesforce.com (an org without My Domain): say so, and stop |
| `malformed_query`, `invalid_field` | Check the API names with `describe` |
| `redirected` | Salesforce answered a redirect, which the helper never follows with the token: say so, and stop |
