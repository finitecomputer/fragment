# Browser-bound Pipes connections

A connection's grant reaches WorkOS Pipes only through the browser that
asked for it. An authorization link that someone else finishes would put
their provider account in the asker's Pipes account, or the asker's in
theirs. So Fragment runs the authorization-code flow itself, with PKCE
(S256) and a confidential client, from the catalog row's
`connection_oauth`. A row without it refuses to connect; nothing falls
back to WorkOS's own authorize URL. The routes are docs/api.md's
(Connections); the rules are `fragment_core::connection_signin` and
`cell/src/registry/connection_signin.rs`.

- **Start** (`POST /api/connections/{provider}/authorize`) is a browser's
  signed in to the platform, never a signed key's. The registry keeps
  the state's hash, the sealed PKCE verifier, the platform session, the
  WorkOS subject, the provider and the row's configuration hash for ten
  minutes, and a host-only HttpOnly cookie (`fragment_connection`) binds
  them to that browser.
- **The callback**, on the platform's own host, requires that cookie
  (`auth::trip_bound`), the same live session, provider and unexpired
  attempt, and consumes the attempt before any exchange: another
  browser, person or session cannot use it, and a replay finds nothing.
  The session and subject are checked again before the import.
- **Tokens** go from the provider's token endpoint straight to Pipes'
  import for that subject: never to the browser, never kept by Fragment,
  never retried. An import whose outcome is unknown consumes the attempt;
  the person connects again. A grant whose answer names no `expires_in`
  (Salesforce's never does) is imported as expired, so Pipes refreshes it
  before its first use.
- **The OAuth client** must be the one the deployment's WorkOS
  integration uses, or Pipes cannot refresh. Its secret is a store
  secret (docs/secrets.md).
- **The account's label** is the email of the ID token answered beside
  the grant: read without its signature (it came from the token endpoint
  over TLS: OpenID Connect Core 3.1.3.7), its `aud` checked, an
  unverified email dropped, and shown to its person, never used for
  access.

## Access

A person picks, as they connect, what their agents may do with the
account: read only, or read and write. A row's `connection_oauth.access`
names the scopes each asks for (`read.scopes`, `write.scopes`), and the
connect asks the provider for the picked one's alone (`POST …/authorize`
`{access}`, required: the platform never picks for them), so the provider
enforces it: a token of a read-only grant cannot write. Read and write
asks for at least one scope read only does not (the catalog refuses a row
otherwise), and that is how Settings tells a grant's access
(`connection_signin::access_of`): read only while every scope it holds,
but signing in, is one read only asks for, else read and write (a scope
the row no longer names, or one the provider added, is never taken for
reading), so it is said as the grant holds it, never as asked. A read-only account offers Allow changes (a reconnect asking for
read and write). Going back to read only is a disconnect, then a connect
as read only: a provider that keeps what was granted before (Google's
`include_granted_scopes`) would keep the write scopes through a
reconnect, and the account would still say read and write.

Some providers' OAuth has no read-only scope: Salesforce's `api` is
everything its user may do through the API. Such a row names read and
write alone (`access.write`, no `read`): Settings offers one Connect and
says no access, and `POST …/authorize` refuses `read` (400). The platform
does not mimic a read only the provider lacks. What its agents may do is
the provider account's own permissions, so a person who wants them to do
less connects as an account that may do less: a dedicated user (a service
account) with only the permissions they mean to give. The row's note says
so.

## Services

A row may name the services a person picks among as they connect
(`connection_oauth.services`: each an id, a name, and the scopes that are
its, of either access): Gmail, Drive and Calendar; Outlook mail, OneDrive
and Contacts. Settings shows each, ticked, beside the connect; a connect
names those ticked (`POST …/authorize` `{access, services}`; none named is
every one), and asks the provider for the picked access's scopes of those
services alone, beside the scopes no service names (signing in). The
pending row keeps the pick, and the callback reads the grant against it.
Microsoft's consent can't leave a permission out, so this is how a person
does there (and leaves out one their organization's admin must approve);
Google's can, and its services are the same choice made before its page.
What the grant then lacks of its access is said as a partial grant's is
(below): a reconnect asks for the services ticked then.

## Partial grants

A person may untick permissions on the provider's consent screen. A
grant of anything beyond signing in connects with what it holds, and the
import records its scopes exactly; one of signing in alone
(`IDENTITY_SCOPES`) imports nothing (`?refused=no-scopes`). Settings names
what its access asks for that is missing, and a reconnect asks only for
the rest.

## Disconnecting

A disconnect revokes the grant at the provider where the row names
`revoke` (RFC 7009), then deletes the account at Pipes. Before either,
the registry fences the account in one turn: attempts still pending end,
callbacks still exchanging import nothing, and one already importing
refuses the fence (429). The fence lasts (`connection_fences`) until a
later connect is known to have imported, so an import that lands late is
never taken for a connection. Google's revocation ends the Google
account's whole grant to the client, so it also ends another person's
connection of that same account.

## An account's own host

Where a row names an `account_host`, the import that makes an account
also hands Pipes the token answer's field the row names (`metadata`,
Salesforce's `instance_url`), when it is an https URL of a host under the
row's suffix; a grant naming none such is not imported (its calls there
would all be refused). WorkOS takes `metadata` on create alone, so a
connect where an account is already is never an update: the account is
deleted and made again, its host the new grant's. An update would keep
the earlier grant's host beside the new token: another org's, where the
person connected another. Such a row connects one account. The swap reads it back from Pipes'
credential (`/data-integrations/{slug}/credentials`) for each request to a
host under the suffix, and sends the token there only when the hosts are
the same (docs/computers.md, "An account's own host").

## Several accounts

Where a row's `accounts_max` is above 1, a person connects several
accounts at one provider, and each agent uses one: the person's pick in
Settings, else the oldest connected. Fragment tells accounts apart by
the ID token's `sub`: a grant of an account already listed reauthorizes
it, never adds a second. One callback at a time decides at a person's
accounts at a provider. The lasting fence is the account's own; one
with no identifier can't be told from the person's others, so it leaves
none while they have others there, and the last of theirs fences the
provider whole (`fragment_core::connection_accounts::lasting`).
