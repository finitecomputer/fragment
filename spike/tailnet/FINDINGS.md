# Tailnet clients spike: findings (2026-11-12)

Throwaway spike on `spike/tailnet-clients`. Evidence (logs, screenshots) lives
on the Linux box in `~/.cache/ts-spike/evidence/` (not pushed: it holds
tailnet addresses). Versions: tailscale 1.104.6 (box, static, sha256
82a85429…), libtailscale dacde75 (tailscale.com v1.104.1), Go 1.27.4,
Xcode 26.5, iOS Simulator 26.5, lego 5.6.0.

## Status of each Tailscale feature (from source and the issue tracker)

| Feature | Client code | Control plane |
|---|---|---|
| MagicDNS subdomains (`x.node.tailnet.ts.net`), nodecap `dns-subdomain-resolve` | merged #18258 (2026-01-30), in quad-100's resolver (`net/dns/resolver/tsdns.go`) | **not enabled**; #1196 reopened 11-02 "as we prepare to roll out"; support said no timeline (08-23) |
| Wildcard certs for subdomains | merged #18356 (2026-02-03) behind the same nodecap | **not enabled**; #7081 open |
| tsnet's own dialer (SOCKS5 `Loopback()`) | resolves exact node names only (`tsdial.resolveAddr` → `NodeByName`); everything else goes to the system resolver | n/a: **subdomains will not resolve through tsnet even after the rollout** |
| tsnet `Loopback()` HTTP CONNECT | not implemented (TODO in tsnet.go) | n/a |
| tailscale-rs | v0.7.1 (11-06): no iOS, no MagicDNS, no hostname lookup, no Tailnet Lock, no certs; pre-1.0 | not viable |

## Measured so far

### Option B in WKWebView (box CA pinned in the app), simulator iOS 26.5

The app sets `WKWebsiteDataStore.proxyConfigurations` to a SOCKS5 proxy
shaped like tsnet's (user `tsnet` + password), `allowFailover = false`, and
answers server-trust challenges with `SecTrustSetAnchorCertificates([boxCA])`
+ `AnchorCertificatesOnly` for the box's names.

- Every check passes (15/15) once ATS allows it: sign-in redirect setting
  cookies; same- and cross-origin scripts, CSS and images; same-origin and
  CORS fetch; the cross-origin chat frame with its own Lax/None/Partitioned/
  Strict cookies; WebSockets in the page (held 20 s, echo + server ticks) and
  in the frame; a 25 MB fetch upload (hash checked); 20 s streamed and idle
  long fetches.
- Every TLS connection, WebSockets included, raises a server-trust
  challenge at the navigation delegate (main frame, subresources, frame,
  fetch, WebSocket); 8 challenges for 5 hosts in one run.
- WKWebView hands the proxy **hostnames** (SOCKS ATYP domain), never a
  locally resolved IP. The test names exist only in the proxy's map, so
  nothing went around the proxy (121 proxy log lines, 0 refusals).
- **Where B breaks: ATS.** With no ATS keys both WKWebView (-1202, underlying
  -9808 "bad certificate format") and URLSession (-1200) refuse, even after
  the delegate accepts the pinned trust. Measured:
  - `NSAllowsArbitraryLoadsInWebContent`: WKWebView OK, URLSession fails;
  - `NSExceptionDomains{d: NSIncludesSubdomains}`: both fail;
  - `NSExceptionDomains{d: NSIncludesSubdomains + NSExceptionAllowsInsecureHTTPLoads}`: both OK;
  - `NSAllowsArbitraryLoads`: both OK.
  So B needs an ATS exception per domain compiled into the app: workable for
  `ts.net` (every MagicDNS name), impossible for an org domain unknown at
  build time without `NSAllowsArbitraryLoads` (App Review justification).
- Same on the sandboxed macOS app (needs `network.client`, and
  `network.server` for tsnet's loopback listener): -1200/-1202 without the
  exception, all network checks pass with it (12/15 reported by the page;
  the other 3 completed on the server, the page's timers were throttled
  because the Mac's screen was locked).
- Name constraints hold: a leaf the box CA signs outside its constraint is
  refused by Apple SecTrust (-67689), rustls and OpenSSL.

### The CLI with option B

- reqwest (rustls + rustls-native-certs 0.8.4) and tungstenite (native +
  webpki roots) both honour `SSL_CERT_FILE`: `SSL_CERT_FILE=box-ca.pem
  fragment …` passes TLS today with no code change (origin log shows the
  request; without it: "unknown certificate authority").
- `SSL_CERT_FILE` replaces the platform roots for reqwest, so a CLI talking
  to the box and a public host at once would need a real config option
  (`add_root_certificate` per host).
- The error without the CA is misleading: "unreachable, or it dropped the
  connection mid-body", not "certificate not trusted".

### Embedded tsnet: size and start (simulator)

- TailscaleKit (iOS arm64) is a 22.2 MB dynamic framework (`-ldflags -w`);
  `strip -x` gives 20.0 MB.
- Release device build: TSSpike 22,088 KB vs TSSpikeBare 356 KB installed;
  zipped IPA 7,901 KB vs 90 KB. **Delta: +21.7 MB installed, +7.8 MB
  download.** The Fragment app today: 14.6 MB installed, 6.3 MB zipped.
- Deployment floor: TailscaleKit needs iOS 18.1 / macOS 15 (Fragment's iOS
  app already needs 26.0).
- Build: Go 1.27.1+ toolchain, `make ios-fat` 49 s, `make macos` 20 s on an
  M-series Mac; no NetworkExtension entitlement.
- First start, no state: tsnet up and showing its login URL 1.0 s after the
  deep link (1.7 s after process start); footprint ~44 MB → ~60 MB.
- iOS socket reclamation (TN2277): tsnet's SOCKS server runs on one loopback
  listener; if it dies, "SOCKS5 server exited" and `Loopback()` keeps
  returning the dead address. Recovery = close and rebuild the node.

### Option A's DNS (public name → 100.64/10)

Cloudflare, Google, Quad9 and the home router (192.168.8.1) all answer a
public name pointing at 100.101.102.103 (no rebind filtering of 100.64/10).

## Pending (needs the tailnet)

- Box joined; `dns-subdomain-resolve` in its CapMap or not; quad-100 answers.
- B over the real tailnet; A's DNS over the tailnet via sslip.io; A's cert.
- tsnet in the app: cold start to first page, reconnect after background.
- CLI on the Mac over the system Tailscale.
- Phone run.
