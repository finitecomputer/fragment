# Tailnet clients spike (throwaway, never merged)

Branch `spike/tailnet-clients`. Answers: can staff reach a self-hosted box with
no public IP over a tailnet, from iOS, macOS and the `fragment` CLI, and how
(option A: org domain + public wildcard cert; option B: MagicDNS + box CA)?

- `origin/` a Fragment-shaped test origin (platform host P, fragments `<name>.P`,
  chat frame on `chat.P`, cookies, WebSockets, long fetches, uploads), logging
  each request's real client address (PROXY v1/v2 from `tailscale serve`).
- `socksprobe/` a SOCKS5 proxy shaped like tsnet's `Loopback()` that logs what
  WKWebView sends through `WKWebsiteDataStore.proxyConfigurations`.
- `ca/mkca.sh` a box CA constrained to one name (option B) plus a rogue leaf.
- `app/` one SwiftUI app for iOS and macOS (xcodegen): direct, SOCKS probe, or
  an embedded tsnet node (libtailscale's TailscaleKit); `TSSpikeBare` is the
  same app without TailscaleKit (size delta).

Findings: `FINDINGS.md`.
