# Google OAuth: how Finite connects, uses and deletes a Google account

For the CASA assessment (OWASP ASVS, run by TAC Security) of Finite's
restricted Gmail and Drive scopes. It describes the code on `master` as of
2026-11-23. File references are to this repository. The wire contract is
docs/api.md ("Connections"); the design is docs/pipes-browser-binding.md.

Tickets: FIN-580 (the OAuth flow), FIN-585 (tokens reach only Google),
FIN-588 (deletion). Sessions are in docs/security/sessions.md (FIN-581).

## Summary

- Finite runs Google's authorization-code flow itself, with PKCE (S256),
  a confidential client and a one-use `state` bound to the browser, the
  platform session and the provider. The redirect URI is one fixed
  string: `https://finite.chat/api/connections/google/callback`.
- Finite never stores a Google token. The code is exchanged in the
  server, and the tokens go straight to WorkOS Pipes, which holds and
  refreshes them. No token is sent to a browser, a log or the agent's
  computer.
- An agent uses Google only through a placeholder. The platform swaps in
  a fresh access token on each request, and only on a request to one of
  the Google API hosts the catalog names. A request to any other host is
  refused before it leaves.
- Disconnecting revokes the grant at Google and deletes the account at
  WorkOS. Deleting a person's account (an operator wipe) does the same
  for every connection once PR #1480, from this review, merges.

## The parties

| Party | Holds | Code |
| --- | --- | --- |
| The browser | the platform session cookie and, for ten minutes, the `state` cookie | `cell/src/auth.rs` |
| The cell (Cloudflare Worker) | the Google client secret and the WorkOS API key, both in Cloudflare Secrets Store (docs/secrets.md) | `cell/src/connections.rs`, `cell/src/keys.rs` |
| The registry (a Durable Object) | each pending connect: the `state`'s SHA-256, the PKCE verifier sealed, the session's hash, the WorkOS user, the provider, the access asked for, ten minutes | `cell/src/registry/connection_signin.rs` |
| WorkOS Pipes | the access and refresh tokens, per WorkOS user | `cell/src/keys.rs` (`pipes_import`, `pipes_token`, `pipes_delete`) |
| The person's computer (a container) | a placeholder per agent (`fcx_google_<tag>`), never a token | `crates/core/src/swap.rs` |

## The flow, step by step (FIN-580)

### 1. Start: `POST /api/connections/google/authorize`

`cell/src/connections.rs:276-324`.

- Only a person may connect, never an agent (`connections.rs:199`), and
  only from the platform's own page with their session cookie. A request
  signed with a CLI key is refused (`connections.rs:290`). A session
  counts on an API call only with the shell's header,
  `Sec-Fetch-Site: same-origin`, and for a POST the platform's exact
  `Origin` (`cell/src/lib.rs:242`, `shell_session`). That is the
  cross-site request forgery defence for this route.
- The body must say which access the person picked:
  `{"access": "read"}` or `{"access": "write"}`. The platform never picks
  for them (`connections.rs:298-301`).
- The registry makes the round trip (`registry/connection_signin.rs:49`,
  `begin_connection`):
  - `state`: 32 random bytes from the platform's CSPRNG, hex
    (`registry/signin.rs:90`, `fresh_token`). Only its SHA-256 is stored.
  - PKCE verifier: 32 random bytes, base64url. It is sealed with a key
    scoped to the registry and to this `state` before it is stored. The
    challenge is its S256 hash.
  - The row records the session's hash, the person's WorkOS user, the
    provider, the access, a hash of the provider's OAuth configuration,
    and expiry in ten minutes (`roundtrip::TTL_S`,
    `crates/core/src/roundtrip.rs:22`).
  - At most 8 pending connects per session and a fleet-wide cap
    (`PENDING_PER_SESSION_MAX`, `signins_pending_max`); past them, 429.
  - It refuses to start when the client secret is not bound, so a
    misconfigured deployment never sends a person to Google.
- The answer is Google's authorize URL (`crates/core/src/connection_signin.rs:117`,
  `authorize_url`) with `client_id`, `redirect_uri`, `response_type=code`,
  `scope`, `state`, `code_challenge` and `code_challenge_method=S256`.
  The catalog row may add only non-protocol parameters
  (`access_type=offline`, `include_granted_scopes=true`,
  `prompt=select_account consent`); a row that tries to set a protocol
  parameter is refused when the catalog loads
  (`connection_signin.rs:71`, `check`).
- The same answer sets the `state` cookie: `__Host-fragment_connection`,
  `HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=600`
  (`cell/src/auth.rs:118`, `trip_begun`).

### 2. Google's consent screen

The person picks the Google account (`prompt=select_account`) and sees
every scope. Google's granular consent lets them untick scopes. A grant of
only the sign-in scopes (`openid`, `email`, `profile`) connects nothing
(`?refused=no-scopes`).

### 3. Callback: `GET /api/connections/google/callback?code=&state=`

`connections.rs:605-658`, then `registry/connection_signin.rs:83-218`.

- Served only on the platform's own host (`connections.rs:607`).
- **Bound to the browser:** the `state` in the URL must equal the
  `state` cookie (constant-time compare, `roundtrip.rs:65`,
  `same_state`), else nothing happens (`auth.rs:154`, `trip_bound`).
- **Bound to the session:** the request must carry a live platform
  session (`connections.rs:640`), and the pending row is taken only for
  the same `state` hash, the same session hash, the same provider, and
  before it expires. That is one SQL statement that deletes the row as it
  reads it (`TAKE_PENDING`, `crates/core/src/connection_signin.rs:14`),
  so the `state` is spent before any network call. A replay, a second
  tab, another person or another session finds nothing.
- An `error` from Google, or a malformed `code`, spends the attempt and
  connects nothing (`connections.rs:641`).
- The code is exchanged at `https://oauth2.googleapis.com/token` with
  the same `redirect_uri` string, the verifier, the client id and the
  client secret (`keys.rs:367`, `connection_exchange`). The request
  follows no redirect, is not retried, and no part of the answer is
  logged (`keys.rs:378`, `confidential_request`).
- Before the import the registry checks again that the session is still
  live, that it is still the same WorkOS user, and that the attempt has
  not expired (`registry/connection_signin.rs:132-134`). A sign-out or
  account wipe during the exchange connects nothing.
- The grant must include a refresh token. Its scopes are recorded as
  Google granted them, not as asked (`connection_signin.rs:205`, `tokens`).
- The tokens go to Pipes' import for that WorkOS user
  (`keys.rs:409`, `pipes_import`). The `Tokens` type has no `Debug`, so
  it cannot be printed by accident (`connection_signin.rs:183`).
- Every refusal redirects (303) to `?refused=<word>`, a page with no
  code in its URL, so an unspent code leaves the address bar and
  history. The `state` cookie is cleared.

### Redirect URI exactness

One function makes it: `connections.rs:415`, `callback_url`, which is
`<platform>/api/connections/<provider>/callback`. `<platform>` is the
deployment's configured origin, never a request's `Host`. The authorize
request and the code exchange both use it, so Google compares the same
string twice. The registry rebuilds it the same way at the exchange
(`registry/connection_signin.rs`, `finish_taken`).

Outside the code, in Google Cloud Console (not checkable from the
repository; FIN-580 lists it): the Google OAuth client should list only
`https://finite.chat/api/connections/google/callback` as a redirect URI,
and the WorkOS redirect URI and the `workos.com` authorized domain should
be removed, since WorkOS never runs this flow.

### Scopes: Read only and Read and write

The Google row (`deploy/example.jsonc:333`, and on PR #1323
`deploy/providers/google.jsonc`) names two scope sets. The connect asks
Google for the chosen one alone, so Google enforces it: a token from a
read-only grant cannot write.

| | Read only | Read and write |
| --- | --- | --- |
| Gmail | `gmail.readonly` | `gmail.readonly`, `gmail.send`, `gmail.modify` |
| Drive | `drive.readonly` | `drive` |
| Sheets, Docs | `.readonly` | full |
| Calendar | `calendar.readonly` | `calendar` |
| Contacts | `contacts.readonly` | `contacts.readonly` |
| Sign-in | `openid`, `userinfo.email`, `userinfo.profile` | the same |

(As written on PR #1323, which moves the row into
`deploy/providers/google.jsonc` and adds names people read in Settings.
The deployed list is in the private deploy config.)

What Settings shows is read from the grant, never from what was asked
(`connection_signin::access_of`). Two things to know:

- `include_granted_scopes=true` makes Google keep earlier grants. A
  person who connected with Read and write and then reconnects with Read
  only still holds the write scopes, and Settings says Read and write.
  Going back to Read only is a disconnect, then a connect
  (docs/pipes-browser-binding.md, "Access").
- An unticked scope is shown as missing, and a reconnect asks only for
  what is missing.

### Where tokens live, and who can read them

- **Finite stores no Google token.** No table in the registry, the
  computer or a fragment holds one. The pending row holds only the
  sealed PKCE verifier, and it is deleted when the callback comes or
  after ten minutes.
- **WorkOS Pipes stores them**, keyed by the person's WorkOS user, and
  refreshes the access token with the same OAuth client.
- **Who can get a token from Pipes:** anything holding the WorkOS API
  key. Only the cell reads that key, from Cloudflare Secrets Store
  (`keys.rs`, `store::WORKOS_KEY`; docs/secrets.md). The cell asks Pipes
  for a token in exactly two places:
  - each swapped agent request (`cell/src/computer.rs:4126`,
    `connection_token`), only for an agent of the account's owner, not
    narrowed away from Google, and only while the connection is
    connected and not fenced by a disconnect;
  - a disconnect, to revoke the grant (`connections.rs:446`,
    `revoke_grant`).
- The token is never returned to the browser, the computer or the agent.
  It is put in the outgoing request's `Authorization` header by the cell
  and nowhere else. The swap's log line names the provider and host,
  never a token or placeholder tag (`computer.rs`, `egress_swap`).
- People with admin access to the WorkOS dashboard are inside the same
  trust boundary as the API key.

### Refresh

Pipes refreshes. The cell asks Pipes for a token on every swapped request
(`pipes_token`), and never caches one. A grant Google has revoked or
expired comes back `needs_reauthorization`, and the swap refuses with
`not_connected` until the person reconnects.

### Revoke

See Disconnect, below.

## Tokens reach only Google (FIN-585)

How an agent's request gets a token:

1. The agent's computer has a placeholder in `GOOGLE_OAUTH_ACCESS_TOKEN`,
   `fcx_google_<tag>`. The tag is an HMAC of (computer, agent, provider)
   under a key derived from the platform's host secret
   (`crates/core/src/swap.rs`, `TagKey`). The computer cannot forge
   another agent's.
2. The computer's container has its outbound traffic to each catalog
   host intercepted, on HTTP and HTTPS (`cell/intercepts.mjs:28`,
   `swapped`). For Google these are only `gmail.googleapis.com`,
   `www.googleapis.com`, `people.googleapis.com`,
   `sheets.googleapis.com`, `docs.googleapis.com` and
   `slides.googleapis.com`. Catalog hosts must be exact names; a glob
   such as `*.googleapis.com` is refused at load
   (`crates/core/src/catalog.rs`, test at `catalog.rs:1083`).
3. The cell (`computer.rs:4808`, `egress_swap`) takes the host from the
   intercepted request and plans the swap (`swap.rs:385`, `Plan::of`).
   A Google placeholder sent to any host that is not one of Google's row
   is refused (`swap.rs:399`, `WrongHost`, 403) before any token is
   asked for. So is a placeholder in a place the row does not name (the
   row names only the `Authorization` header), a forged tag, or an agent
   not on this computer.
4. The request goes out to `https://<that host><path><query>`
   (`computer.rs:4917`). The host is the one just checked; the path comes
   from a parsed URL, so it cannot change the authority. It always goes
   over HTTPS, even when the agent sent HTTP.
5. Redirects are not followed (`computer.rs:4922`,
   `RequestRedirect::Manual`). A redirect goes back to the agent with the
   placeholder, never the token.
6. Hop-by-hop headers, `Host` and the platform's own `x-fragment-*`
   headers are dropped before sending (`computer.rs:4788`).

Other requests that carry Google credentials, all to fixed catalog URLs
that must be HTTPS (`connections.rs:392`, `reachable`):

- the code exchange, to the row's `exchange` URL;
- the revocation, to the row's `revoke` URL;
- WorkOS calls, to the WorkOS API.

No user- or agent-supplied URL is used for any of them. The catalog is
the deployment's config, not user input.

**Conclusion:** we found no path by which a person, an agent or a
fragment can cause a Google token to be sent to a non-Google host.
`FRAGMENT_SWAP_UPSTREAM` reroutes swapped requests to a test fake. It is
set only by the local test harness (`crates/devstack/src/lib.rs:768`),
and the deploy tooling has no way to set it, but the cell itself does
not refuse it on a hosted deployment. Hardening is filed as FIN-647
(defence in depth, not an open path).

**Internal and metadata addresses:** an agent's other traffic leaves
the container directly to the internet (Cloudflare Containers). It
carries no Google token, since only the cell's swap adds one. Platform
services are reachable from the container only by intercepted names
(`api.fragment.internal`, `model.fragment.internal`). Whether a
container can reach link-local or cloud metadata addresses
(`169.254.169.254`) is Cloudflare's network, and was not tested here
(gap; FIN-648).

## Disconnect and deletion (FIN-588)

### Disconnect: `DELETE /api/connections/google[?account=<id>]`

`connections.rs:325-372`, `disconnect` at `connections.rs:425`.

1. **Fence first.** The registry ends every pending connect for that
   account in every session, cancels any callback still exchanging a
   code, and records a lasting fence. An import that lands later is
   never treated as connected (`registry/connection_signin.rs:250`,
   `fence_connection`).
2. **Revoke at Google.** The cell asks Pipes for a current access token
   and posts it to `https://oauth2.googleapis.com/revoke` (RFC 7009;
   `connections.rs:446`, `revoke_grant`). Google then revokes the refresh
   token too. A failure here is logged without the token and does not
   stop step 3. The answer's `revoked: false` makes Settings tell the
   person to remove access in their Google account
   (`cell/shell/connections.js:148`).
3. **Delete at WorkOS.** The account is deleted at Pipes
   (`keys.rs:465`, `pipes_delete`). A 404 counts as done, so the
   disconnect can be repeated.
4. The person's computer is told at once, so swaps stop at the next
   request, not when a cached state expires.

Google's revocation ends that Google account's whole grant to Finite's
client. If two Finite people connected the same Google account, one
disconnect ends both (docs/pipes-browser-binding.md).

### What is cached after a disconnect

- No token anywhere in Finite.
- The person's computer keeps the connection's state, the account's
  label (its email) and its granted scopes to show in Settings. These are
  replaced on the disconnect.
- The registry keeps the fence row (issuer, WorkOS user, provider, and
  for several accounts the Google `sub`) until a later connect imports,
  or until the account is wiped.
- **Google data an agent already fetched** (an email's text, a file) is
  wherever the agent put it: its computer's disk, its chat transcripts,
  or a fragment it wrote to. A disconnect does not find or delete that.
  The person deletes those chats, files or fragments, or asks for their
  account to be deleted (below). A written process for deletion requests
  is FIN-646.

### Deletion of the whole account (operator wipe)

`cell/src/wipe.rs`, docs/api.md ("Operators: wiping a person").

**Defect found and fixed (FIN-588).** Until this review a wipe deleted
everything Finite holds but left every WorkOS Pipes account in place.
docs/api.md said so: "A wipe leaves … their WorkOS user and its Pipes
connections". So after a person's account was deleted:

- their Google grant stayed live at Google, and its refresh token stayed
  at WorkOS;
- their next sign-in with the same WorkOS user made a new Finite person
  who found Google already connected, with the old grant.

No other person could reach those tokens: Pipes keys them by the WorkOS
user, which only that same person signs in as. But it broke the promise
that deleting an account deletes the Google data and grant, which
Google's user data policy requires.

The fix (PR #1480; until it merges, `master` still behaves as above):
the wipe's first step, `computer`, now ends every connection of
the catalog before anything else, while the registry still knows the
person's WorkOS user (`connections::wipe`, which calls the same
`disconnect` as above for each account Pipes lists, connected or not;
`fragment_core::connection_accounts::wiped`). A failure stops the step,
and the next wipe call tries again. Each part may be repeated.

## Findings

| # | Ticket | Severity | Finding | Where | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | FIN-588 | High | An account wipe left the person's Google grant live at Google and its tokens at WorkOS, and the next sign-in found them connected again | `cell/src/wipe.rs`, docs/api.md | Fixed, PR #1480 |
| 2 | FIN-588 | Low | A connect cancelled after Google issued tokens (a disconnect, sign-out or wipe during the code exchange) drops the tokens without revoking them. No one holds the refresh token, but Google keeps listing Finite under the account's third-party access | `registry/connection_signin.rs:132-182` | Filed, FIN-645 |
| 3 | FIN-588 | Medium | No written process for deleting Google-derived data an agent saved (chats, files, fragments) when someone asks | process | Filed, FIN-646 |
| 4 | FIN-585 | Low | `FRAGMENT_SWAP_UPSTREAM` sends every swapped request, tokens included, to another URL; the cell does not refuse it outside a local fleet | `cell/src/config.rs:460`, `computer.rs:4912` | Filed, FIN-647 (defence in depth) |
| 5 | FIN-585 | Info | Reachability of link-local and metadata addresses from a computer was not tested | Cloudflare Containers | Filed, FIN-648 (needs the real platform) |
| 6 | FIN-580 | Info | The Google client in Cloud Console should list only the cell's callback, without the WorkOS redirect URI or `workos.com` domain | Google Cloud Console | Manual, on FIN-580 |
| 7 | FIN-580 | Info | A reconnect as Read only keeps earlier write scopes (`include_granted_scopes`); Settings shows the true access, and going back is disconnect then connect | docs/pipes-browser-binding.md | By design, documented |

Checked and found sound: `state` (random, one use, hashed at rest, bound
to the browser, the session and the provider, ten minutes); PKCE S256
with a sealed verifier; one fixed redirect URI; scopes chosen by the
person and enforced by Google; code exchange server-side with no
redirects, retries or logged answers; tokens never in Finite storage,
the browser, the computer or logs; swap only to exact Google hosts, over
HTTPS, without following redirects; disconnect fences, revokes at Google
and deletes at WorkOS.
