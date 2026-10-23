# Browser-bound Pipes connections

`POST /api/connections/:provider/authorize` is a platform-browser operation.
A signed key cannot start it. A connection needs `connection_oauth` in its
catalog row; without it authorization fails closed. There is no fallback to
WorkOS's `/data-integrations/:provider/authorize` URL.

The Registry creates a random state and S256 PKCE verifier, seals the verifier,
and keeps the state hash with the initiating platform-session hash, provider,
WorkOS issuer/subject, configuration hash and ten-minute expiry. The answer
sets a host-only HttpOnly, SameSite=Lax `fragment_connection` cookie
(`__Host-` and Secure on HTTPS). Starting again replaces that browser's cookie.
At most eight attempts per session and the fleet's pending-sign-in limit in
total are retained. Expired attempts are swept; wiping the person removes them.

The provider returns to the platform's own
`/api/connections/:provider/callback`. `auth::trip_bound` proves possession of
the initiating browser's cookie. The Registry requires the *same live session*,
provider and unexpired transaction and atomically consumes it **before** any
external exchange. Another browser, person or session cannot consume it. The
provider exchanges its one-time code with the saved PKCE verifier and client
credentials. Fragment requires a bounded bearer grant (access token, refresh
token, bounded expiry, at most 32 bounded scopes) of at least one requested
scope beyond signing in (Partial grants, below), rechecks the live session and
WorkOS subject, then imports it into that subject's Pipes account. Tokens are
never returned to the browser or stored by Fragment; WorkOS stores and refreshes
them. No response-body diagnostics, redirects or automatic network retries are
used on the credential-bearing requests.

The documented import is `POST /user_management/users/:user_id/connected_accounts/:slug`.
Only an explicit 409 permits a `PUT` update of that same user's existing account,
with the same body, so a reconnect's grant (its `scopes`, its label) replaces
the earlier one's. An uncertain result consumes the attempt; reconnect rather
than replaying it. The import's `scopes` are the grant's exactly: the token
endpoint's `scope` as it answered, or, when it answers none, the scopes
requested (RFC 6749 5.1).
The import names the account as its person knows it: when the token endpoint
answers an OpenID Connect `id_token` beside the grant (the row asks for `openid`
and the provider's email scope: Google's `https://www.googleapis.com/auth/userinfo.email`),
its `email` becomes the connected account's `account_display_name` (`null` when
there is none, so an update never keeps an earlier account's). The ID token is
read without checking its signature: it came straight from the token endpoint
over TLS (OpenID Connect Core 3.1.3.7, step 6), its `aud` must be the row's
client, an `email_verified: false` email is not used, and the label is only shown
to the person (settings, `GET /api/connections`' `account`), never used to decide
access.
This deliberately supports confidential clients using `client_secret_post`,
authorization code, S256, and a space-delimited scope response. Other provider
protocols require an explicit implementation, not an unbound fallback.

## Partial grants

Fragment asks for its row's whole scope list and is fine with whatever the
person leaves out (Paul, issue #445): Google's consent screen lets them untick
each permission (granular consent), and a grant of some of them connects with
those. Only a grant of nothing the row asks for beyond signing in is refused:
`openid`, `email`, `profile` and Google's
`https://www.googleapis.com/auth/userinfo.email` and `…/userinfo.profile`
(`fragment_core::connection_signin::IDENTITY_SCOPES`), which Google grants
beside anything else and which let an agent do nothing. Such a callback imports
nothing and comes back `?refused=no-scopes`, a page that says to leave at least
one permission ticked. A row that asks for signing in alone takes any grant of
it.

`GET /api/connections` reads the grant back from Pipes (the connected
account's `scopes`, read with no token minted): each connection's `granted`,
and its `missing`, what its row asks for beyond signing in that the grant
lacks. Settings names what is missing in words ("Not allowed: Sheets, Drive")
with a **Reconnect to add**, the same Connect flow. Google's row asks for
`include_granted_scopes=true`, so Google keeps what the person allowed before
and its consent asks only for the rest; the grant it answers names them all,
and the update records it. Pipes refreshes the grant; what it holds is what
the list says. An agent's call outside the grant gets Google's own 403
(insufficient scope), which the Google skill explains to the person: add it in
Settings, under Connections. A connected account Pipes names no scopes for has
none said to be missing.

## Disconnecting

`DELETE /api/connections/:provider` (settings' Disconnect, or a person's key)
deletes the account at Pipes (`DELETE
/user_management/users/:user_id/connected_accounts/:slug`, 204), which ends
every swap of it. Deleting it there leaves the grant at the provider, so a row
may name a revocation endpoint, `connection_oauth.revoke` (RFC 7009; Google's is
`https://oauth2.googleapis.com/revoke`). With one, Fragment first asks Pipes for
an access token for the account and posts it there as `token`; Google revokes
an access token's refresh token with it. The token goes only to that endpoint,
with no redirects or retries, and is in no answer or log. A revocation that
fails is logged and the account is deleted anyway; the answer's `revoked` says
which happened, and settings tells the person to remove the access at the
provider when it is false. An account Pipes no longer gives a token for has no
grant left to revoke this way. Google's revocation ends the Google account's
whole grant to the OAuth client, so when two people here connected the same
Google account, one's Disconnect ends the other's connection at its next
refresh too (Pipes can no longer refresh it).

Before it revokes or deletes anything, a disconnect fences the account in the
Registry (`POST /connection/fence`, one turn, no await between check and act),
so no connect begun before it connects the account after it:

- every attempt at that WorkOS user's account at that provider still pending is
  ended, in each of their sessions (its callback then finds no transaction:
  `not-under-way`);
- each callback past its pending row and still exchanging its code is
  cancelled: it re-checks just before importing and imports nothing. The grant
  its exchange made is not revoked: Google's revocation ends the account's
  whole grant to the client, so it could end a newer connect of the same
  account, and the cancelled callback's tokens are kept nowhere;
- a callback already importing refuses the fence (429, nothing ended), since
  what it sent Pipes cannot be called back; the disconnect asked again, once
  that answered, deletes it;
- and the account stays fenced, in a Registry table (`connection_fences`), until
  a connect of the person's is known to have imported again (Pipes answered it
  200 or 201; the fence goes in that same turn). An import that fails, or
  whose outcome is unknown (a timeout, a 5xx), lifts nothing, so if it lands
  at Pipes after the disconnect deleted the account, it is not taken for a
  connection: `GET /api/connections` reads it as `not_connected` with Pipes
  not asked, and the person's computer, which swaps only a connection its
  states say is connected (the disconnect tells it them at once; a refresh
  begun before that does not draw over them), refuses its swaps and asks
  Pipes for no token. The account lingers at Pipes, used by no one, until the
  person connects again, which replaces it.

Callbacks under way are kept in the Registry's memory, not a table: each one
runs inside the Registry and ends with it, so a Registry that restarts has none
under way, while the lasting fence is a row and outlives it. Another person's
attempts, and the person's attempts at another provider, are untouched. A
connect begun after the fence is a new one and may connect. A disconnect that
cannot tell the person's computer fails, and is asked again (every step of it
may be asked twice). A wipe deletes the person's fences with their sign-ins.

## Evidence and scope

- [Google's web-server OAuth flow](https://developers.google.com/identity/protocols/oauth2/web-server)
  documents state validation, the callback and code exchange.
- [Google's OAuth client](https://github.com/googleapis/google-auth-library-nodejs/blob/main/src/auth/oauth2client.ts)
  exposes S256/code_verifier with the same confidential-client exchange.
- [WorkOS connected-account reference](https://workos.com/docs/reference/pipes/connected-account)
  documents import, refresh-token/access-token/expiry combinations, 409 on an
  existing account, and update. A refresh token without the configured matching
  OAuth client will not make refresh work; the deployment must use the same
  client as its WorkOS integration.
- The published `@workos-inc/widgets` 1.20.1 Pipes implementation requests
  `requireHandoff: true` and exchanges handoff/redirect tokens with a popup.
  Its private GraphQL completion contract is not the public REST contract.
  This change does not infer a vendor guarantee from that implementation or
  call its private API.

The local `connections` e2e lane uses two users and separate browser cookie
sets. It verifies copied URLs, missing/wrong-kind cookies, another user's and
another same-user session, replay, expiry, superseded starts, PKCE code
substitution, restart, reauthorization, and partial grants (one kept as
granted and listed with what it lacks, one of signing in alone refused, and a
reconnect that adds the rest) against real workerd and local provider/Pipes
fakes. Protocol tests reject malformed grants, grants of signing in alone and
configuration overriding binding parameters. Local tests establish our ordering
and attribution, not the live vendor configuration or refresh behavior.

## Rollout still required; this change does none of it

Paul reviews and deploys. Keep the staging provider disabled until the code and
configuration are reviewed. No new GCP IAM grant is required to implement or
test this PR locally.

The repository's normal release path is `.github/workflows/ci.yml`: PRs test;
a green `master` push deploys the GitHub `preview` environment with
`cargo xtask deploy --pinned-only --branch preview`; the `production` branch
uses the separate `production` environment without that branch flag. Deployment
requires green `check` and `e2e`. Production promotion is a later, separate
decision, after staging validation. No separate environment or manual deploy
is needed for this fix.

| Setting | Staging | Production | Reusable? |
| --- | --- | --- | --- |
| GitHub deployment environment / source branch | `preview` / `master` | `production` / `production` | Mechanism only; keep environment restrictions separate |
| Platform origin | Existing `preview` environment's `DEPLOY_HOST` and deployment config | Existing `production` environment's `DEPLOY_HOST` and deployment config | No; verify actual targets before merging/promoting |
| WorkOS environment and user issuer | Existing staging environment `environment_01KSB8XM8RSN80NM31YVYBAY20` | Existing production environment, verify its identifier | No cross-environment credentials or user IDs |
| Google project / OAuth client | `finitestaging228527660165` / existing `Finite Pipes Staging` client | Existing production project/client, verify; do not duplicate or rotate | Separate client IDs and secrets |
| Google authorized redirect URI | `https://<staging-platform>/api/connections/google/callback` | `https://<production-platform>/api/connections/google/callback` | Path is shared; origins are separate |
| OAuth endpoints and compatibility scopes | Reviewed endpoints/scopes below | Same protocol and reviewed scope set | Yes, subject to production consent-screen review |
| Cloudflare binding | `CONNECTION_SECRET_GOOGLE`, existing staging client's secret in its store | Same binding name, production client's secret in its store | Binding name only; not secret values |
| Deployment catalog | `connection_oauth` + store-name `key` in fragment-deploy's `preview.jsonc` | Equivalent row in fragment-deploy's `production.jsonc` | Shape only; client ID/store names/origin must be checked per environment |
| WorkOS provider | Existing staging Google integration, currently Disabled | Existing production integration, status to verify | Keep enablement separate and gated |
| Canonical privacy-policy / terms URLs | **Pending placeholders**; not a blocker for the controlled External/Testing staging work | **Pending placeholders**; do not submit unfinished policies for production verification | Reuse the eventual approved canonical URLs; none are invented or published here |

The existing WorkOS API/client bindings and Cloudflare deployment credentials
need no rotation for this feature. The new OAuth client-secret binding is an
additional consumer of the existing corresponding Google client, not a request
to generate new credentials. Parent coordinates exact account configuration;
none of these secret values were read or printed here.

1. Add the exact staging platform callback
   `https://<staging-platform>/api/connections/google/callback` to the **same**
   Google OAuth client configured in the staging WorkOS integration. The existing
   WorkOS callback alone does not serve this flow. Do not point staging at a
   production client or callback.
2. Bind that client's secret through Cloudflare Secrets Store as
   `CONNECTION_SECRET_GOOGLE`. In the deployment provider row, `key` is its
   **store name**, never the value. Add `connection_oauth` with `client_id`,
   `authorize: https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&include_granted_scopes=true&prompt=select_account%20consent`
   (`select_account`: Google's account chooser, even in a browser signed in to
   one Google account, so a person picks which account to connect; `consent`:
   the refresh token Pipes needs, every time; `include_granted_scopes`: what
   the person allowed before is kept, so a reconnect asks only for the rest:
   Partial grants, above),
   `exchange: https://oauth2.googleapis.com/token`, and the reviewed compatibility
   scopes. Endpoint/client/scope settings are public configuration; secrets are
   read only by `cell/src/keys.rs`. For Disconnect, add `revoke:
   https://oauth2.googleapis.com/revoke`, but only once a release that knows the
   field is deployed: a catalog refuses fields it does not know. Like any change
   to the row, it changes the configuration hash, so a Connect begun before that
   deploy comes back refused and is started again.
3. Review prior accounts and pending legacy authorization URLs in any environment
   that used the old flow. This patch cannot invalidate URLs already issued by
   WorkOS or undo existing provider grants. Resolve those before enabling use;
   deleting a Pipes account does **not** revoke the underlying Google grant.
4. Separately authorize provider enablement and a controlled staging grant.
   Start with the sole approved test account, isolated browser profiles and a
   dedicated Fragment test user. Verify a copied link cannot import or expose
   credentials, then the original profile can complete, replay cannot exchange,
   and the imported account belongs to the initiating WorkOS user. Inspect only
   status/attribution, never token values. Separately authorize a minimal API
   read and refresh test. A genuine two-Google-user test requires another
   explicitly approved test account; do not use someone else's account.
5. Revoke the test grant at Google and remove the test connected account after
   validation. Production requires Paul's separate rollout decision.

For deployment verification, require CI's `/healthz` `x-fragment-deploy` header
to match the merged commit, and verify that a connection with no OAuth config
refuses authorization without redirecting or importing. Then use only the
separately approved staging test above. Provider status remaining Disabled is
not evidence that the new callback/import works live.

Rollback: first disable new connection starts by removing both `connection_oauth`
and its provider-row `key` from that environment's catalog through the normal
deployment path. An orphan `key` is rejected by deployment validation. Preserve
Registry schema step 2 in any rollback build: deploying a pre-step worker onto
the upgraded Registry is refused by the schema guard and can break sign-in.
Use a reviewed forward fix or revert of feature code retaining the migration,
not an unexamined promotion of an older SHA. Credential revocation or deletion,
if needed, is a separately authorized account operation.

As of 2026-10-20, the user's staging OAuth client and compatibility scopes are
configured and the WorkOS Google provider remains **Disabled**. No live grant,
token import, refresh test, provider enablement or deployment was performed for
this change. Production, Places and billing remain outside this work.
