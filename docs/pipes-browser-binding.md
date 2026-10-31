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

## Partial grants

A person may untick permissions on the provider's consent screen. A
grant of anything beyond signing in connects with what it holds, and the
import records its scopes exactly; one of signing in alone
(`IDENTITY_SCOPES`) imports nothing (`?refused=no-scopes`). Settings names
what is missing, and a reconnect asks only for the rest.

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
