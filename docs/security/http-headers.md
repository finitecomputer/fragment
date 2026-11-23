# HTTP security headers

What the cell (the Cloudflare Worker in `cell/`) sends on its answers, and
why. Written for the CASA assessment (FIN-575, FIN-582). The header values
are in `crates/core/src/answer_headers.rs` and `crates/core/src/frames.rs`.
The cell sets them in `cell/src/lib.rs`, in `fetch`, `platform_origin` and
`fragment_origin`, and in the page handlers named below.

The cell answers three kinds of host:

- **The platform's host** (`finite.chat`): the shell's pages, sign-in, the
  API, and refusals.
- **A fragment's host** (`<name>.finite.place`, and a claimed address):
  each one runs a user's app on its own origin.
- **A computer's host**: a computer's ports, for its owner.

## On every host

| Header | Value | Why |
|---|---|---|
| `Strict-Transport-Security` | `max-age=31536000` | After one visit, the browser uses only HTTPS for that host for a year. It is set on every answer that arrived over HTTPS, errors and redirects included, replacing any value an app or a computer's image sent. Over plain HTTP the cell adds none and leaves the answer as it came, so an app's own header passes through; browsers ignore HSTS there, and the local dev stack (`http://127.0.0.1:8790`) must keep working. |

We leave out **`includeSubDomains`** on purpose. `finite.chat` has
subdomains that this Worker does not answer: `preview.finite.chat`,
`www.finite.chat`, and whatever mail or vendor names DNS may add later.
HSTS with `includeSubDomains` would bind all of those to HTTPS, and each
name should decide that for itself. It isn't needed for fragments either.
Each fragment's host is answered by this Worker, so each one sends its own
HSTS header.

We leave out **`preload`** because the preload list requires
`includeSubDomains`. Cloudflare's zone-level HSTS setting is another way
to get the same result. We set it in the Worker so that it is in code and
reviewed, and so that it applies the same way on a self-hosted deployment.

## The platform's host

| Header | Value | Why |
|---|---|---|
| `X-Content-Type-Options` | `nosniff` | On every answer. |
| `Referrer-Policy` | `same-origin` by default; the shell's pages send `strict-origin-when-cross-origin`; sign-in redirects send `no-referrer` | URLs may carry share links or sign-in state. |
| `Content-Security-Policy` | Shell pages: `default-src 'self'; script-src 'self'; style-src 'self'; …; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'` (`cell/src/shell.rs`). Sign-in, sharing and notice pages have their own strict policies (`auth.rs`, `share.rs`, `updating.rs`). | No inline script on the shell. Only the platform's own origin and fragment hosts may be framed (`frame-src`). |
| `frame-ancestors` (CSP) | `'self'` on any answer that doesn't name its own | Anti-clickjacking for API JSON, errors, redirects and every other answer. The shell frames two of the platform's own pages, `/share` and `/auth/frame`, so `'self'` is needed there; no other origin may frame anything. |
| `X-Frame-Options` | `DENY` on shell and sign-in pages; `SAMEORIGIN` elsewhere | The same rule for browsers without CSP. |
| `Cross-Origin-Opener-Policy` | `same-origin` on shell and sign-in pages | Another window can't keep a handle to the signed-in page. |

## A fragment's host

Fragments are user apps. The app decides which scripts and styles it
loads, so **the platform imposes no script or style CSP** on a fragment's
pages: one strict enough to satisfy a scanner would break apps that load a
CDN script or use inline handlers. An app can send its own CSP. The
platform's headers are merged with it, so the app can't weaken them.

| Header | Value | Why |
|---|---|---|
| `X-Content-Type-Options` | `nosniff` | Set over anything the app sends. |
| `Referrer-Policy` | `same-origin` | Set over anything the app sends. A page's URL may hold its share link (`?view=`). |
| `frame-ancestors` (CSP) | `'self' https://finite.chat` (the platform's origin) on every answer, an unclaimed address's or a failed registry lookup's included | Only the fragment's own pages and the shell may frame it, which the shell needs to show apps. A frame's navigation can get a narrower policy (`'self'` alone, for a page framing itself: `frames::ancestors`). The cell sends this as a policy of its own unless one of the answer's policies already enforces exactly it, judged by each policy's first `frame-ancestors`, the one a browser enforces (`answer_headers::has_directive`). So an app can narrow it with its own `frame-ancestors`, but can't widen it: a browser enforces every policy, and a duplicate later in a policy doesn't count. |
| `Content-Security-Policy: sandbox` | On scriptable media (SVG, XML) and on HTML content files (`__file`) | Opened directly, user-uploaded content runs no script and gets an opaque origin. |
| `X-Frame-Options` | not sent by the platform | It can't name a second origin (`ALLOW-FROM` is obsolete), and `SAMEORIGIN` would stop the shell from showing the app. An app's own `X-Frame-Options` passes through: it can only narrow, and CSP `frame-ancestors` takes precedence over it in every current browser. |

## A computer's host

A computer's pages come from its own image. The cell adds HSTS and
`frame-ancestors <platform>` (`cell/src/computer.rs`, `framed_by`), to
a refused signature's answer too, and leaves the rest as the image
answers.

## Not sent, and why

- **`Permissions-Policy`**: the shell delegates a few features to the
  frames it shows (`clipboard-write`, and `microphone` to one fragment's
  frame for voice input: `cell/shell/shell.js`). Each frame gets only what
  its `allow` attribute names, and a cross-origin frame gets no powerful
  feature by default. A header on the shell would have to list the same
  features again and would add no protection. We'll revisit this if the
  scan flags it.
- **`Cross-Origin-Embedder-Policy` / `Cross-Origin-Resource-Policy`**: the
  shell shows fragment origins in frames and loads fragment images.
  `require-corp` would make every app send CORP headers.

## How to check

`curl -sD - -o /dev/null https://finite.chat/` (a page),
`https://finite.chat/api/does-not-exist` (an error) and
`https://<name>.finite.place/` (a fragment) each show the headers above.
