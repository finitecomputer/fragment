# Tailnet clients spike: findings (2026-11-12)

Throwaway spike on `spike/tailnet-clients`; never merged. Evidence (logs,
screenshots) is on the Linux box in `~/.cache/ts-spike/evidence/`, not
pushed, since it holds tailnet addresses.

Versions:
- tailscale 1.104.6 (static, tgz sha256 82a85429…);
- libtailscale dacde75 (tailscale.com v1.104.1);
- Headscale 0.29.7; Go 1.27.4; lego 5.6.0;
- Xcode 26.5; iOS Simulator 26.5 (iPhone Air 6C481C2B); macOS 26.6.2.

The tailnet runs used a local, rootless **Headscale**, because the box's
login to the real tailnet was still pending. Headscale used Tailscale's
public DERP map, and the clients ran the real tailscaled and tsnet code. Its
policy granted `dns-subdomain-resolve` to every node through `nodeAttrs`,
which makes it a preview of Tailscale's rollout.

## Feature status (source + issue tracker + live runs)

| Feature | Client code | Tailscale control plane |
|---|---|---|
| MagicDNS subdomains (`x.node.tailnet.ts.net`), nodecap `dns-subdomain-resolve` | merged #18258 (01-30) into quad-100 (`net/dns/resolver/tsdns.go`). **Live:** with the cap, quad-100 answers `chat.box…` and `a.b.box…` | **not enabled**: #1196 reopened 11-02 "as we prepare to roll out"; support said no timeline (08-23) |
| Wildcard certs for subdomains | merged #18356 (02-03), behind the same nodecap | **not enabled**; #7081 open |
| tsnet's own dialer (`Loopback()` SOCKS5, userspace tailscaled's SOCKS/HTTP proxy) | `tsdial.resolveAddr` resolves exact node names, plus extra records whose `Type` is empty. Anything else goes to the **system resolver**. **Live, with the cap granted:** `lookup chat.box.hs.tsspike.test: i/o timeout`. So subdomains will **not** resolve through tsnet even after the rollout | n/a |
| Exact extra DNS records | resolve through tsnet when `Type` is empty (live: 200 via SOCKS). Headscale's `extra_records_path` (a watched file) can publish one per fragment | Tailscale's admin has no custom records |
| tsnet `Loopback()` HTTP CONNECT | not implemented (TODO in tsnet.go); SOCKS5 only | n/a |
| tailscale-rs | v0.7.1 (11-06): no iOS, MagicDNS, hostname lookup, Tailnet Lock or certs; pre-1.0 | not viable |
| Public Suffix List | `ts.net` is listed: each `<tailnet>.ts.net` is one site | n/a |

## Hosted Tailscale, live (the box on the real tailnet, 11-12 ~20:45Z)

- The box's CapMap has **no** `dns-subdomain-resolve`. Quad-100 answers
  `<box>.<tailnet>.ts.net` → 100.x but `chat.<box>.<tailnet>.ts.net` →
  **NXDOMAIN**. Through the box's SOCKS, the platform host gives 200 and
  `chat.<box>` gives rc=97.
- `tailscale cert <box>.<tailnet>.ts.net`: a Let's Encrypt (YE1) cert in
  36 s, 90 days. `tailscale cert '*.<box>…'`: "wildcard certificates are not
  enabled for this node". `tailscale cert chat.<box>…`: "must be one of
  [<box>…]".
- So hosted Tailscale today offers B one publicly trusted name, the
  platform host's, and nothing for fragments.
- A-shaped names (`<x>.100-a-b-c-d.sslip.io` → the box) work through the
  real tailnet; `serve --tcp 443 --proxy-protocol 2` was the front door.

## 1. Names

- MagicDNS subdomains: not live on Tailscale today (the box joining the real
  tailnet is pending; its CapMap gets checked then). Under a control plane
  that grants the cap (Headscale), quad-100 resolves them. That serves
  devices running the Tailscale app.
- In-app tsnet never uses quad-100 for its dials. So B's per-fragment names
  through an embedded node need one of:
  - (a) exact records per fragment (Headscale only);
  - (b) the app mapping `*.<box>` to the box's tailnet IP itself (a small
    SOCKS shim in front of tsnet, or a one-line upstream patch to
    `resolveMagicDNS` that walks parents when the cap is set);
  - (c) split DNS to a resolver on the box. This helps the Tailscale app but
    **not** tsnet, which ignores split DNS for SOCKS dials.
- Option A's names (public DNS → 100.x) resolve everywhere with no help.
  Measured with sslip.io names pointing at the box's 100.64.0.1: the box's
  SOCKS gave 200, and the simulator's tsnet passed 15/15.
- Public resolvers answer names pointing into 100.64/10: Cloudflare, Google,
  Quad9 and the home router all returned 100.101.102.103, so no rebind
  filtering. Enterprise resolvers with rebind protection are not tested.

## 2/3. TLS

### Option B in WKWebView (box CA pinned in the app)

The app sets `WKWebsiteDataStore.proxyConfigurations` = SOCKS5 (`tsnet` user
plus password, `allowFailover = false`). It answers server-trust challenges
with `SecTrustSetAnchorCertificates([boxCA])` + `AnchorCertificatesOnly`, for
the box's names only.

- **All 15 checks pass:**
  - sign-in redirect setting cookies;
  - same- and cross-origin script, CSS and images;
  - same-origin and CORS fetch;
  - the cross-origin chat frame with its own Lax/None/Partitioned/Strict
    cookies;
  - WebSockets in the page (held 65 s) and in the frame;
  - a 25 MB upload (hash checked);
  - 65 s streamed and idle fetches.

  Measured through a logging SOCKS probe, and through real tsnet on Headscale.
- Every TLS connection raises a server-trust challenge at the navigation
  delegate: main frame, subresources, frame, fetch, WebSocket.
- WKWebView sends the proxy **hostnames** (SOCKS ATYP domain) and never
  resolves them itself. All app traffic arrived from the app's tailnet
  address (28/28 requests from 100.64.0.2 on 4 hosts, plus 2 WebSockets).
  The probe refused nothing (0 refusals in 121 lines).
- **Where B breaks: ATS.** With no ATS keys, both WKWebView (-1202, underlying
  -9808 "bad certificate format") and URLSession (-1200) refuse, even after
  the delegate accepts the pinned trust. Measured on iOS:

  | ATS keys | URLSession | WKWebView |
  |---|---|---|
  | none | ✗ | ✗ |
  | `NSAllowsArbitraryLoadsInWebContent` | ✗ | ✓ |
  | `NSExceptionDomains{d: NSIncludesSubdomains}` | ✗ | ✗ |
  | `NSExceptionDomains{d: NSIncludesSubdomains + NSExceptionAllowsInsecureHTTPLoads}` | ✓ | ✓ |
  | `NSAllowsArbitraryLoads` | ✓ | ✓ |

  So B needs a per-domain ATS exception compiled into the app:
  - workable for `ts.net`, which covers every MagicDNS name;
  - impossible for an org domain unknown at build time, short of
    `NSAllowsArbitraryLoads` (needs an App Review justification).
- macOS (sandboxed app): the same -1200/-1202 without the exception, and
  passes with it. The sandbox needs `network.client` and `network.server`
  (tsnet's loopback listener).
- Name constraints hold. A leaf the box CA signs outside its constraint is
  refused by Apple SecTrust (-67689), rustls and OpenSSL.

### Option A (public wildcard cert by DNS-01)

lego 5.6.0 is ready. The real certificate waits on a zone and a token from
the owner. Its trust on iOS, macOS and the CLI is the platforms' default
behaviour; to be shown. Everything else about A (DNS → 100.x → tailnet,
through tsnet and through the box's dialer) is measured above.

### The CLI

- reqwest (rustls + rustls-native-certs 0.8.4) and tungstenite (native plus
  webpki roots) both honour `SSL_CERT_FILE`. So `SSL_CERT_FILE=box-ca.pem
  fragment …` works today with no code change. Without it: "unknown
  certificate authority".
- `SSL_CERT_FILE` replaces reqwest's platform roots: a real `ca` setting in
  the CLI config (`add_root_certificate`) would be cleaner.
- The CLI over a **userspace** tailnet node works through
  `HTTPS_PROXY=http://127.0.0.1:<tailscaled http proxy>`, because reqwest
  honours env proxies. The live socket (tungstenite) ignores proxies.
- The error with an untrusted CA is misleading: "unreachable, or it dropped
  the connection mid-body".

## 4. Embedded tsnet (TailscaleKit) vs the Tailscale app

- **Size:** TailscaleKit iOS arm64 is a 22.2 MB dynamic framework (20.0 MB
  with `strip -x`). Release device builds:

  | Build | Installed | Zipped IPA |
  |---|---|---|
  | TSSpike (with TailscaleKit) | 22,088 KB | 7,901 KB |
  | TSSpikeBare | 356 KB | 90 KB |
  | **Delta** | **+21.7 MB** | **+7.8 MB** |
  | The Fragment app today | 14.6 MB | 6.3 MB |

- **Build:** Go 1.27.1+; `make ios-fat` 49 s, `make macos` 20 s. The floor is
  iOS 18.1 / macOS 15. No NetworkExtension entitlement; the macOS sandbox
  needs `network.server`.
- **Cold start, saved node (simulator, local control):**
  - tsnet up at 1.46–1.89 s;
  - first page committed at 1.89–2.32 s;
  - signed-in page done at 2.01–2.42 s from process start (3 runs).

  First run, before login: the login URL appears 0.9–1.7 s after process
  start.
- **Memory:** footprint ~35–40 MB in direct/probe mode, ~60–67 MB with tsnet
  up: **+25–30 MB**.
- **CPU:** foreground, tsnet up, page idle: 0.39 s in 120 s (simulator, M
  Mac). Suspended in background: 0.
- **Reconnect after background:**
  - 30 s away: the first fetch succeeded 21 ms after foreground.
  - 150 s away: the control long-poll reconnected 0.2 s after foreground (log
    shows "node connected"), and a fresh load finished in about 100 ms. My
    in-page probe of the old page hung; that page was mid-test.

  The simulator does not reclaim sockets: see the phone gap.
- **iOS socket reclamation (TN2277):** tsnet's SOCKS server lives on one
  loopback listener. If it dies, "SOCKS5 server exited", and `Loopback()`
  keeps returning the dead address. Recovery = close and rebuild the node.
- **Identity:** WhoIs on the box names the app's node and its owner
  (`tsspike-sim-hs`, user `spike`, OS iOS, no tags). An app logged in
  interactively is a person's node. One logged in with a tagged auth key is
  not (and tsidp refuses tagged devices).

## 5/6. Fragment's behaviours over this path

All passed through tsnet and through the probe:
- cookies on the platform host and fragments;
- the chat frame's own cookies, all four SameSite flavours, Partitioned
  included;
- credentialed fetches both ways between the platform and the chat host;
- WebSockets (65 s);
- long fetches (65 s streamed and idle);
- 25 MB uploads (86.8 Mbps on a direct path).

Self-host is **same-site** (the platform and `<name>.<P>` share a registrable
domain; `ts.net` is on the PSL). So Safari's cross-site frame-cookie problem
from prod (finite.chat vs finite.place) does not arise. The cell already
assumes one site: `__Host-` cookies, Origin checks on POST, Sec-Fetch-Site.

## Front door and identity door: requirements from this spike

- **TLS ends on the box** (A's wildcard, or B's box CA). `__Host-` cookies and
  `Secure` need https.
- **Client address:** rootless tailscaled (userspace) hands inbound
  connections over from 127.0.0.1. `tailscale serve --tcp 443
  --proxy-protocol 2 tcp://127.0.0.1:<door>` restores the tailnet source
  (measured). The door must:
  - parse PROXY v2;
  - set `CF-Connecting-IP` from it, overwriting any the client sent;
  - set `X-Forwarded-Proto: https` (the cell's `arrived_url`);
  - keep `Host`;
  - pass WebSocket upgrades and stream bodies;
  - impose no idle timeout under the cell's own.

  With root (TUN), the socket peer is the 100.x address directly.
- **Identity door:** one `WORKOS_API_URL` serves both the browser's authorize
  and logout redirects and the cell's server-side code exchange. So it needs
  one name that resolves:
  - for clients: on the tailnet, through the same proxy the WebView uses. A
    name under the platform's wildcard works for A, and for B when it has a
    record/mapping like any fragment;
  - on the box: to loopback, since a rootless box cannot reach its own 100.x.

  The door's WhoIs takes the PROXY-v2 address.
- **Cookie domains:** keep everything host-only (no `Domain=`). Fragments
  and every other service on the tailnet or org domain are same-site, so
  SameSite is no boundary there. Origin checks stay mandatory.
- **B on iOS:** the app must ship an ATS exception for `ts.net`, or B is
  dead. Per-fragment names need the app to map `*.<box>` itself (Tailscale)
  or exact records (Headscale) until the nodecap and a tsnet fix land.

## Gaps

See the report: the real tailnet (box login), a Let's Encrypt cert, the
system Tailscale on the Mac, the phone, battery, and the dev stack pass.
