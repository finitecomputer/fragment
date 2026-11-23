# Sessions: cookies, lifetime, sign-out and invalidation

For the CASA assessment (OWASP ASVS V3, session management). It describes
the code on `master` as of 2026-11-23 (FIN-581). File references are to
this repository. The wire contract is docs/api.md ("Sign-in"); Google's
connect flow, which builds on these sessions, is
docs/security/google-oauth.md.

## Summary

- People sign in with WorkOS AuthKit. Finite keeps no passwords: after
  WorkOS's code exchange it issues its own session.
- A session token is 32 random bytes. The browser holds it in a
  `__Host-` cookie that is `HttpOnly`, `Secure` and `SameSite=Lax`. The
  server keeps only its SHA-256.
- Every request whose answer depends on who is asking looks the session
  up live, so a revoked or expired session stops working at once.
- A session lasts 30 days from sign-in. There is no idle timeout and no
  sliding renewal.
- Sign-out revokes the session on the server, every fragment session
  made from it, and the WorkOS session.

## Kinds of session

| Session | Cookie (over HTTPS) | Lifetime | Where it is kept |
| --- | --- | --- | --- |
| Platform (finite.chat) | `__Host-fragment_session` | 30 days from sign-in (`SESSION_TTL_MS`, `cell/src/registry/signin.rs:71`) | registry `sessions` table, by SHA-256 |
| A fragment's origin, top level | `__Host-fragment_site` | until its platform session expires | registry `sessions`, `parent` = the platform session |
| A fragment's origin, in a frame of the platform's page | `__Host-fragment_frame`, `SameSite=None; Partitioned` | until its platform session expires | registry `sessions`, with its embedder |
| A computer's origin (its desktop and ports) | `__Host-fragment_computer`, or `__Host-fragment_computer_frame` in a frame | 12 hours (`cell/src/computer.rs:130`) | that computer's Durable Object, by SHA-256 |
| A round trip in progress (sign-in, switch, own-key sign-in, connection) | `__Host-fragment_login`, `_switch`, `_own_key`, `_connection` | 10 minutes, one use | registry, by SHA-256 |

CLI and app keys are not cookies: each request is signed with the
person's key (NIP-98, `crates/nip98`), within a short time window
(`limits::AUTH_WINDOW_S`). A key is revoked in Settings, and that also
ends any fragment session made from it.

## Cookie flags (ASVS 3.4)

`cell/src/auth.rs:179-201`.

- `set_cookie`: `<name>=<value>; Path=/; Max-Age=<s>; HttpOnly;
  SameSite=Lax; Secure` (`Secure` whenever the request is HTTPS, which
  every hosted deployment is).
- `frame_cookie`: the same with `SameSite=None; Partitioned` and always
  `Secure`, so a frame session lives only in the partition of the page
  that framed it (CHIPS).
- **`__Host-` prefix** whenever the cookie is HTTPS and `Path=/`
  (`cookie_name`, `auth.rs:179`). The browser then accepts it only
  host-only and `Secure`. This matters because every fragment shares one
  domain with the others: no fragment's page can set or replace another
  origin's session cookie. Cookies are read under that name only.
- A cookie's value is accepted only as exactly 64 hex characters
  (`cookie_of`, `auth.rs:203`); anything else is ignored before the
  registry is asked.

## Creation (ASVS 3.2)

- **New token at every sign-in.** WorkOS's callback
  (`auth.rs:372`, `callback`) asks the registry to exchange the code; the
  registry makes a fresh token (`fresh_token`, `registry/signin.rs:90`:
  32 bytes from the Workers CSPRNG) and stores its SHA-256
  (`new_session`, `registry/signin.rs:429`). A token is never taken from
  the request, so session fixation is not possible.
- **The sign-in is bound to the browser that began it.** `/auth/login`
  stores a one-use `state` (its SHA-256) and sets it in
  `__Host-fragment_login`. The callback must bring the same `state` in
  its URL and its cookie (`trip_bound`, `auth.rs:154`; constant-time
  compare, `crates/core/src/roundtrip.rs:65`), within ten minutes. The
  row is deleted as it is read (`registry/signin.rs:533`, `finish`).
- WorkOS must have verified the email, or no session is made (403).
- Switching accounts (`POST /auth/switch`) revokes this browser's session
  before the new sign-in begins.

## Use (ASVS 3.5, and forgery)

- **Live lookup.** `live_session` (`registry/signin.rs:401`) accepts a
  token only when its row exists, is not revoked, has not expired, is of
  the expected kind (platform, this fragment's, frame or top level), and
  its parent platform session (or parent key) is still live.
- **Cross-site request forgery.** `SameSite=Lax` keeps the cookie off
  cross-site POSTs. On top of that, the API counts the platform session
  only with the shell's own header, `Sec-Fetch-Site: same-origin`, and,
  for anything but GET or HEAD, the platform's exact `Origin`
  (`shell_session`, `cell/src/lib.rs:242`). Platform forms refuse another
  `Origin` (`same_origin`, `auth.rs:309`), and sharing's forms also carry
  a form token bound to the session.
- **Framing.** Platform pages answer `frame-ancestors 'none'` and
  `X-Frame-Options: DENY`, except the share sheet and sign-in notes,
  which only the platform's own page may frame.

## Lifetime and timeout (ASVS 3.3)

- **Absolute:** 30 days from sign-in for a platform session and every
  fragment session made from it (a fragment session expires with its
  parent, `registry/signin.rs`, `redeem`). The cookie's `Max-Age`
  matches.
- **Idle:** none. A session used every day still ends at 30 days; one
  never used also lasts 30 days.
- **Renewal:** none. When it expires the person signs in again through
  WorkOS. WorkOS's own session may sign them in without a password.
- Expired rows are deleted by the registry's alarm
  (`registry/signin.rs:268`, `sweep_signin`).

## Sign-out and invalidation

| Event | What ends | Code |
| --- | --- | --- |
| Sign out (`POST /auth/logout`) | the platform session and every fragment session made from it (revoked on the server), the cookie (cleared), and the WorkOS session (the browser goes to WorkOS's logout with its `sid`) | `auth.rs:418`, `registry/signin.rs:692` |
| Sign out on a fragment (`POST __signout`) | that fragment's sessions in this browser, and the person's consent to it | `registry/signin.rs:684` |
| Use another account (`POST /auth/switch`) | this browser's platform session | `auth.rs:434` |
| A key revoked | every fragment session made from that key | `registry/signin.rs` (`live_session`'s key check) |
| Account deleted (operator wipe) | every session and key of the person and their agents, at the wipe's first call; sign-in refused until the wipe is done | `registry/wipe.rs:248`, `end_access` |
| 30 days pass | the session and its fragment sessions | `live_session` |

## Findings

| # | Severity | Finding | ASVS | Status |
| --- | --- | --- | --- | --- |
| 1 | Medium | Signing out does not end a computer-origin session. It lives in the computer's own store for 12 hours and is not tied to the platform session, so a browser that signed out still opens the person's computer desktop and ports until it expires. (A wipe destroys the computer, so deletion is not affected.) | 3.3.1 | Filed, FIN-642 |
| 2 | Medium | A person cannot see or end their other sessions ("sign out everywhere"). Signing out ends only this browser's. | 3.3.4 | Filed, FIN-643 |
| 3 | Low | No idle timeout: a session stays valid for 30 days whether used or not. | 3.3.2 (L2) | Filed, FIN-644 (a product decision) |
| 4 | Low | A change at WorkOS (password reset, user blocked or deleted there) does not end Finite sessions already issued; they last to their 30 days. | 3.3.3 | Filed, FIN-643 |
| 5 | Info | `POST /auth/logout` without an `Origin` header is accepted. Browsers always send one, and the worst case is a forced sign-out. | — | By design (`same_origin`) |

Checked and found sound: token entropy (256 bits) and hashing at rest;
`__Host-`, `HttpOnly`, `Secure`, `SameSite`; a new token at each sign-in;
browser-bound, one-use sign-in state; live lookup on every request;
server-side revocation at sign-out, including fragment sessions and the
WorkOS session; deletion at wipe.
