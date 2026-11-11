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
  the person connects again.
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

## Access the platform keeps

Some providers have no read-only scope: Salesforce's `api` is everything
its user may do through the API. Such a row's read access names the HTTP
methods it may use (`read.methods`, some of `GET` and `HEAD`), and its
scopes may then be read and write's (the catalog refuses identical scopes
otherwise, and methods on write). Where a provider's GET may change
something (Salesforce's Tooling `executeAnonymous` runs Apex on a GET, as
an org's own Apex REST resources may), it names the paths read only may
reach too (`read.paths`: patterns of segments, `*` any one, each reaching
itself and what is under it), and a read-only call is held to both. A path
is read as plainly written or not at all: an encoded `/` or `.`, a `\`,
an empty, `.` or `..` segment reaches no pattern, since a server may read
it as another path. The platform enforces it, not the provider:

- The registry keeps the access picked (`connection_access`). Read only
  is kept before the import (in the turn its session is checked), so a
  reconnect as read only whose import's outcome is unknown, and lands, is
  read only; read and write is kept only in the turn a connect is known to
  have imported, as its fence is lifted, and only while the person who
  began it is still signed in as that WorkOS user (a wipe while Pipes
  answered keeps nothing). None kept is read only
  (`connection_signin::access_held`). A wipe takes it.
- The egress tells the computer each request's method and path
  (`computer/credentials`' `method`, `path`). A call read only may make
  (its methods, at its paths) asks nothing more. Any other asks the registry for the access kept, then,
  never a belief from an earlier read (a downgrade since would leave it
  writing): read only, it is refused (403, saying so) before Pipes is
  asked for a token, so nothing is minted and nothing is sent
  (`connection_signin::call_allowed`). Settings reads it with the
  fences, in the one question a person's states ask already.
- Such a row connects one account (`accounts_max` 1): its access is kept a
  provider, not an account.

A row's paths are its operator's to keep to resources that only read: the
platform holds read only to what the row names, and no further.

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
