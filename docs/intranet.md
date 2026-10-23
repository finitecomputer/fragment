# fragment as an intranet: install on the box, open a link on each device

Status: **proposal, 2026-10-23**: research and a design, no code. For
Paul, after his first run of docs/self-host-lan.md: "way too many steps,
way too brittle, and sounds really scary to run". It would replace that
guide's names, certificates and sign-in (docs/self-host.md, seams 4 and
9). Everything behind the front door stays as it is: celld, computers on
nodes, models, secrets, cards.

## Why

Today's LAN mode is a company's intranet played by hand. The box stands in
for IT's DNS, IT's CA and IT's identity provider, and each device is
enrolled in them as a managed laptop would be, except that nothing manages
it:

| | Today (docs/self-host-lan.md) | Proposed, at home |
|---|---|---|
| The box | a fixed address on the router; `setcap` on a binary (or a sysctl); three ufw rules; a systemd-resolved drop-in; the root in the box's own stores; a checkout, its env, `cargo xtask dev --lan` | one install command, the password once |
| Each phone | its DNS pointed at the box (or the router's, for the house); the root's profile; three settings screens to trust it; perhaps Private Relay off | open the link (scan the QR) |
| Each Mac | an `/etc/resolver` file; the root in the System keychain; Firefox apart | open the link |
| Sign-in | Dex, a password file per person, read on the box with `cat` | a passkey, made at that link |

Each step is one more place to go wrong, and each fails in a way a person
cannot read: Paul's first sign-in failed on a box step that was missed.
Installing a root is also the scary part: an unmanaged root is what iOS
warns about, name-constrained or not.

## What fragment needs from the network

- **A wildcard, one level deep.** Each app is its own origin,
  `https://<name>.<zone>` (a name is `<label>--<suffix>`), the platform is
  the zone itself, and a computer's origin is `<24 hex>--computer.<zone>`.
  One certificate for `<zone>` and `*.<zone>`, and one wildcard record,
  cover every origin there will ever be.
- **Real HTTPS.** The session cookies are `__Host-` (docs/api.md), and
  service workers, push, passkeys and the clipboard need a secure context.
  `*.localhost` is the one secure context without TLS, and it is one
  machine's.
- **Not path mode.** Master had one: without `FRAGMENT_HOST_SUFFIX`,
  fragments were served at `/f/<name>/` on the platform's origin, cookies
  scoped by path. A page of one fragment could call another's `__op` and
  `__file` with the visitor's cookies (the debt ledger's words), and every
  fragment shared the platform's own origin. It was cut on 2026-10-06
  (#213, 728c6c3b: "fragments are always on their own hosts"); the
  `pathmode` section went with it. Coder says the same of its path-based
  apps ("can make requests to the Coder API and pose a security risk when
  the workspace serves malicious JavaScript"), and GitLab serves Pages from
  a hostname of its own "to prevent XSS attacks". Apps here are written by
  agents and by strangers: a wildcard, or nothing.
- **Offline**, for Paul's milestone (docs/self-host-lan.md, step 8): the
  whole loop with the WAN unplugged.

## 1. The recommendation

**At home, the box gets a name and a certificate from Finite**, as Plex,
Sandstorm, Tailscale and Home Assistant's remote UI get theirs. The box
makes its own keys and asks Let's Encrypt for `<box>.<boxes domain>` and
`*.<box>.<boxes domain>` by DNS-01. Finite's DNS publishes only the
challenge and the box's LAN address. Every device already trusts that
certificate and every resolver can find the name, so a device opens a link
and nothing else. Sign-in is a passkey made at that link, and Dex goes.
Working offline is one optional setting on the router.

**In a company, fragment asks IT for what Coder and GitLab Pages ask
for**: a wildcard DNS record, a certificate for it (from their CA, which
their devices already trust, or by ACME), and an OpenID Connect client in
their IdP. Devices need nothing. Finite runs nothing.

Both run **one front door** (crates/lan's, kept), which gets its
certificate from one of four places (below). The cell is unchanged: its
zone, its origin and its issuer are configuration already.

**Before building any of it, one day's test on Paul's network** settles
what the research could not: the same design with a domain Paul already
holds (`*.home.<a domain of his>` answering 192.168.50.7, a Let's Encrypt
wildcard by DNS-01 at its DNS provider, the door given the files). On his
iPhone with Private Relay on, his Mac, the ASUS, and a passkey used in
airplane mode. A subzone of finite.place would do for the test, not for
keeps: it shares a site with the previews.

### At home: a name and a certificate from Finite

Examples use `boxes.example` for the domain Finite would register for this
(its name is an open question) and `k7q2m9xd` for a box.

#### What the person does

**On the box, once:**

1. Run the installer: `curl -fsSL https://<finite>/install.sh | sh`, or
   the package. It asks for the password once, to install a system
   service and, where ufw or firewalld is on, to open 80 and 443 to the
   home's own subnet (it says which rules it added). Then, with no more
   questions, it:
   - makes the box's key and registers it with Finite's name service,
     which names it `k7q2m9xd.boxes.example` and publishes the box's LAN
     address for that and `*.k7q2m9xd.boxes.example`;
   - gets the certificate from Let's Encrypt (about a minute);
   - starts the stack.
2. It prints a QR code and a link,
   `https://k7q2m9xd.boxes.example/setup#<code>`: the code is single-use,
   good for an hour, and only makes the owner. `fragment-box setup-link`
   prints a new one and voids the last.
3. If anything is wrong, `fragment-box doctor` says what and how to fix
   it: the ports, the certificate and when it renews, whether the name
   answers the box's address, the issuer, the node.

**On each device:**

- **The owner's phone:** scan the QR. The page asks a name and an email (a
  handle: no mail is sent), then Face ID makes a passkey. They land in the
  shell, signed in. Add to Home Screen if they like.
- **Everyone else:** the owner taps **Invite to this box** (Settings,
  People) and sends the link by Messages or AirDrop. It opens the same
  page with the email filled in; Face ID; the shell.
- **A Mac's terminal:** `fragment login https://k7q2m9xd.boxes.example`
  opens the browser to approve the key, as today. No resolver file, no
  root.
- **A Mac as a node:** `sandcastle-node pair https://k7q2m9xd.boxes.example
  …`, without `--ca-file`.

**To keep working offline (optional, once, on the router):** send the
box's zone to the box. Asuswrt-Merlin, OpenWrt and Pi-hole take dnsmasq's
`server=/k7q2m9xd.boxes.example/<box>` (with `rebind-domain-ok` for it
where the filter is on); pfSense and OPNsense a domain override. A router
that cannot forward one domain (stock Asuswrt, Google Wifi) uses the box
as the house's DNS server instead: the box forwards every other name on,
as Pi-hole does, and while the box is off the house has no DNS. That is what Nextcloud AIO's LAN-only guide asks. The box answers
its zone with the same address the public DNS gives, under the same
certificate, so nothing on any device changes.

**When the link does not open:** some routers drop public names that
answer private addresses ("DNS rebinding protection"; on by default in
OpenWrt, pfSense, OPNsense and Google and Nest Wifi, always on in a
FRITZ!Box, off by default on ASUS and in Pi-hole). The installer also
prints the box's address, `http://192.168.50.7/`: that page (plain http, by
address, no DNS) tests whether this device finds the name, opens it if it
does, and otherwise names the router's fix: an exception for the zone where
the router has one, or the offline setting above, which also ends the
filtering. Plex and Unraid document the same exceptions.

#### What fragment builds

1. **`fragment-box`**, crates/lan grown up, still the one process that
   holds the low ports:
   - the door as it is (TLS, routing by host, its own forwarded headers,
     streams, WebSockets, its limits);
   - **certificates by ACME**: instant-acme (DNS-01, ARI, Let's Encrypt's
     profiles, EAB for another CA), the challenge's TXT through the name
     service, renewal in ARI's window, the chain served whole (Plex
     servers broke in September 2026 serving Let's Encrypt's new
     intermediate without the cross-signed root that trust stores do not
     hold yet), and `doctor` warning with a third of its life left;
   - **the name service's client**: register (the box's key: a Nostr key,
     as the CLI's are, each request signed as a NIP-98 event, crates/nip98),
     keep its LAN address current, set and clear the challenge, set CAA;
   - **the issuer** at `https://id.<zone>` (item 2);
   - **DNS for the zone** (dns.rs as it is, for the new zone): on
     loopback always, for the box itself (its resolver sends the zone
     there, so the cell reaches its issuer, the node its platform and the
     card browser its pages with the WAN down: today's step 1d, done by the
     installer); on the LAN only once the household turns offline on;
   - the page by address over http (the root's page, re-purposed);
   - `doctor` and `setup-link`.
2. **The issuer**, in `fragment-box`: an OpenID Connect provider for the
   household, so the cell keeps seam 4 and gains no mode flag:
   - **passkeys only**, as Pocket ID. The relying party's ID is
     `id.<zone>`, never `<zone>`: any app's page may claim `<zone>` as its
     RP ID, so a passkey there is one every app could ask the person to
     use;
   - **the owner** by the installer's setup code; **everyone else** by an
     admin's invite link (single-use, 24 hours, as Kanidm's reset tokens);
     **a lost phone** by a new link from an admin, adding a passkey to the
     same person (the same `sub`); **a lost owner** by `fragment-box
     setup-link --owner` on the box: whoever holds the box holds the house,
     as with a router's reset button;
   - the email is what the owner typed in the invite, so the issuer
     vouches for it (`FRAGMENT_OIDC_CLAIMS`'s `vouched`, seam 4: a
     directory whose addresses are its own to give).
3. **The installer and a release**: one bundle (celld, the cell's build,
   `fragment-box`, OpenBao, `sandcastle-node`, the card browser), a
   `fragment` system user, state in `/var/lib/fragment`, and one systemd
   unit whose door holds 80 and 443 (and 53 with offline on) by
   `AmbientCapabilities=CAP_NET_BIND_SERVICE`, as Coder's and Caddy's
   packaged units do: root once, at install, never again. Updates in
   place. The sandcastle engine (root, microVMs) and the model are their
   own installs today; the box's installer should bring the engine too.
4. **The cell: nothing.** `FRAGMENT_HOST_SUFFIX` is the box's zone,
   `FRAGMENT_PLATFORM_URL` its apex, `FRAGMENT_OIDC_ISSUER`
   `https://id.<zone>`; no `CELLD_EXTRA_CA_FILE`.
5. **The e2e**: a fake of the name service (crates/fakes: a vendor's
   boundary) and Pebble, Let's Encrypt's test CA, for the certificate's
   path; the issuer's valid, invalid, replay and restart cases (a setup
   code used twice, an invite replayed, a restart mid-ceremony).

#### What Finite runs: the name service

- **A domain of its own**, never a product's, so no cookie and no
  blocklist is shared with finite.chat (plex.direct beside plex.tv;
  GitLab's advice for Pages). On the Public Suffix List once it serves
  thousands (the PSL's bar), so each box is a site of its own. The PSL
  refuses entries "whose sole purpose is to circumvent rate limits";
  fragment's reason is the list's own, that strangers' boxes must not share
  cookies (sandcats.io, ts.net, ui.nabu.casa, libp2p.direct and
  direct.quickconnect.to are on it).
- **Records, per box:** `<box>` and `*.<box>`, A (and AAAA) to its LAN
  address, private ranges only (RFC 1918, RFC 6598, ULAs), so no public
  site can live under the domain; `_acme-challenge.<box>` TXT while a
  challenge runs; CAA `0 issue "letsencrypt.org; accounturi=<the box's
  account>; validationmethods=dns-01"` (RFC 8657; every CA must honour it
  from 2027-03-15, ballot SC-098v2).
- **An API** on Finite's own stack (a Worker, a Durable Object per box):
  register (a key in, a random name out, no account), address, challenge
  (set, clear), each signed by the box's key and rate-limited by source; a
  box touches its own records alone.
- **Authoritative DNS:** a managed zone with an API to start (Cloudflare's
  Pro and Business plans hold 3,500 records a zone: about a thousand boxes
  at three each), then two small servers of its own (hickory, as dns.rs;
  two providers) answering from the API's table, as sandcats.io runs its
  own.
- **Let's Encrypt's limits:** 50 new certificates a week per registered
  domain; renewals of the same names are exempt, and renewals at ARI's
  suggested time are exempt from every limit. So about 50 new boxes a week,
  until the domain is on the PSL or Let's Encrypt grants an override (its
  form, which takes weeks, is for "large hosting providers or organizations
  working on a Let's Encrypt integration"). ZeroSSL and Google Trust
  Services also issue wildcards by DNS-01 (an EAB key per account): the
  fallback CA.
- **What it costs:** a domain (tens of dollars a year), the DNS (Cloudflare
  Pro, or two small hosts), the API (a Worker). The real cost is being on
  the path: of every renewal, since Let's Encrypt's authorization reuse
  falls to 7 hours in 2028 and each renewal then needs a fresh challenge
  (DNS-PERSIST-01, a record set once at registration, would end that; its
  ballot passed in 2025, but Let's Encrypt has not shipped it, and Google
  Trust Services ships an older draft); and of every lookup in a household
  without the offline setting. Sandstorm ran sandcats.io for about 500
  servers, and it still answers in 2026.
- **Its risks:** abuse (private addresses only; rate limits), the PSL's
  slow and hard-to-undo changes, and one CA.

#### What it gives up

- **Offline takes the router setting.** Names come from public DNS, so with
  the WAN down a device finds nothing beyond its caches (dnsmasq and
  Unbound serve nothing stale by default), unless the router sends the zone
  to the box. Certificates hold offline until they lapse: renewed at two
  thirds of their life, about 30 days past the renewal point today (90-day
  certificates), 21 from 2027-02-10 (64-day) and 15 from 2028-02-16
  (45-day), on Let's Encrypt's schedule. A box offline longer shows
  certificate errors until it is back. A house offline for good takes a
  company's path (its own CA), or today's CA, kept (open questions).
- **Privacy.** Finite learns that each box exists, its LAN and public
  addresses, and when it renews. The names devices look up (each app's
  label) reach Finite's DNS through the household's resolver. The
  Certificate Transparency logs show `<box>` and `*.<box>`, never an app,
  and the renewals' rhythm. With the offline setting, lookups stay in the
  house.
- **Trust in Finite.** Whoever controls Finite's DNS can get a certificate
  for a box's names; it helps only someone also on that house's path (its
  LAN, or its resolver). CAA's `accounturi` makes that a visible change,
  and the box watches the CT logs for certificates on its names it did not
  ask for (`doctor` says so). The keys never leave the box, as with
  Tailscale and Sandstorm (Unraid's vendor makes its boxes' keys, and by
  all appearances Plex's does).
- **Filtering routers** need one setting, once (above).
- **iCloud Private Relay: unknown until tested.** Safari with Private Relay
  resolves through Apple, past the router (its filter and its forwarding
  alike), and connects through the relay by name. Apple says a name that
  "is not a public internet name" is sent direct; whether a public name
  answering a private address counts is not documented, and Plex users
  reported failures in 2022. If it fails, the fix is per device (Settings,
  Wi-Fi, (i), Limit IP Address Tracking off), as today's guide already says:
  the test above decides.
- **Dependence.** If Finite stops the service, boxes keep working until
  their certificates lapse, then move to a domain of their own: the same
  door, DNS-01 at their DNS provider.

#### Offline against no setup

Three facts fix the tradeoff, whatever fragment builds:

- **No setup on a device means public DNS**, the one resolver every device
  already asks, and public DNS needs the WAN.
- **Names offline mean the house's DNS knows the zone**: one setting on
  the router (or, as today, on every device).
- **Certificates with no root on a device mean a public CA**, which needs
  the WAN every few weeks (fewer each year: 47 days at most by 2029).
  Offline for good means a root on every device, by hand or by MDM.

So the home gets no setup online, one router setting for days offline,
and today's CA (or a company's) only for a house that is never online.
Paul's WAN-unplugged demo is the second: the ASUS's DNS server set to the
box (today's step 4, for the house), the rest unchanged.

### In a company: their name, their certificate, their sign-in

#### What IT does

What Coder, GitLab Pages and Ona's runners ask of IT, and nothing else:

1. **A host and the package.** A server or VM (or a container, or Helm),
   its unit running as a service user, the door holding 443 by
   `CAP_NET_BIND_SERVICE`, or behind their load balancer.
2. **DNS:** `fragment.corp.example` and `*.fragment.corp.example` to the
   host, in their internal zone (split horizon if the name is also
   public). A zone on a domain of its own is better (GitLab's advice for
   Pages): under the company's domain, cookies the company sets for
   `.corp.example` reach every app. Fragment's own isolation holds either
   way (seam 9: host-only `__Host-` cookies, Origin checks).
3. **One certificate for both names**, one of:
   - **from their CA**, which managed devices already trust (Group Policy,
     an Intune trusted certificate profile, an MDM profile's
     `com.apple.security.root`), as two files the door reloads when they
     change;
   - **by ACME from the door**: DNS-01 at their DNS (Route 53, Azure DNS,
     Cloud DNS, Cloudflare, or RFC 2136 for BIND and Windows DNS; a CNAME
     delegating `_acme-challenge` to a narrow zone keeps the door's token
     narrow), or against their internal ACME CA (step-ca, Vault PKI since
     1.14; AD CS has no ACME of its own);
   - **at their load balancer or ingress**, fragment behind it on plain
     HTTP, trusting its forwarded headers (Backstage's way).
4. **An OIDC client** in their IdP (Okta, Entra ID, Google, Keycloak,
   ADFS): the redirect URI `https://fragment.corp.example/auth/callback`,
   and the email rule (seam 4: `vouched` for their own directory).
5. **Secrets** in their Vault (seam 12) or the bundled OpenBao; 443 open
   from their networks.

**On each device: nothing.** Managed devices trust their CA and ask their
DNS. Remote access is their VPN or ZTNA (Tailscale, Cloudflare Access,
Zscaler), as for any intranet app. **Air-gapped**, the same with every part
inside: their CA, their DNS, an IdP on site (seam 4's offline providers:
Keycloak, Authentik, Dex, ADFS).

#### What fragment builds

The door's certificate from files (reloaded on change) and from ACME at a
DNS provider or an ACME directory; a setting for "behind your proxy"; a
package and a Helm chart; one page of what fragment asks of IT, in Coder's
shape. The cell: nothing; seams 4 and 12 are built.

#### What it gives up

Nothing of isolation, and Finite runs nothing. A company that forbids
wildcard certificates needs one per name: the door could ask an internal
ACME CA for each at a fragment's first request (Caddy's on-demand TLS); a
public CA would log every app's name (open questions). A small office with
no IT is a home, and takes the home's path.

### One door, four places its certificate comes from

| Source | For | Names | How |
|---|---|---|---|
| `finite` | a home | `<box>.<boxes domain>` | ACME DNS-01, the challenge through Finite's name service |
| `acme` | a power user; a company with an API on its DNS or an internal ACME CA | their zone | ACME DNS-01 at their DNS provider (or RFC 2136), or their CA's directory |
| `files` | a company's PKI | their zone | two PEM files, reloaded when they change |
| `none` | behind a company's proxy | their zone | plain HTTP from the proxy, its forwarded headers trusted |

The door's DNS answers the zone under any of them, when asked to; the cell
never knows which.

## 2. The options compared

| Option | Each device | The box | Offline | Trusted HTTPS | Wildcards | Risks | Finite operates | Company fit |
|---|---|---|---|---|---|---|---|---|
| **Today**: the box's CA and DNS, Dex | DNS to the box; the root (iOS: a profile, three screens); a Mac's resolver file and keychain; Firefox apart | a fixed address, setcap, ufw, resolved, password files | yes | where the root is installed | yes | a root on every device (name-constrained); the house's DNS on the box, if the router points there | nothing | a company's shape, with none of its parts |
| **Box names from Finite** (recommended, home) | open a link | the installer (root once) | names no, unless the router setting; certificates until they lapse | yes, public | yes, `*.<box>` | Finite could get a box's certificate; filtering routers; app names in lookups; Private Relay untested | a domain, DNS, an API; the PSL; Let's Encrypt | poor: not their domain |
| **… with the router sending the zone to the box** | open a link | the installer; one router setting | yes, until certificates lapse (30 days past renewal now, 15 from 2028) | yes | yes | as above; lookups stay in the house | as above | it is split horizon |
| **A domain of your own**, DNS-01 at its provider | open a link | the installer; a domain and its DNS's API token | as the two above | yes | yes | the token can change the domain (narrow it by a CNAME delegation) | nothing | good: what companies with public DNS do |
| **The company's DNS and certificate** (recommended, company) | nothing (managed) | IT: a wildcard record, a certificate, an OIDC client | yes, with their CA and IdP inside | yes, their CA's or public | yes | their PKI's | nothing | the norm (Coder, GitLab Pages, Ona) |
| **Wildcard-to-IP names** (nip.io, sslip.io, traefik.me) | open a link | none | no | no: no certificate of your own; shared keys get revoked | names only | anyone's names; published keys | nothing | no |
| **mDNS `.local`** with a private CA | the root on each device; Linux needs nss-mdns | one name published per app; the CA | yes | only with the root | no | as Today; any device on the link can claim a name | nothing | no |
| **Tailscale** (MagicDNS, `tailscale cert`, Services) | the app and a sign-in on every device | tailscale | connected peers only; no new devices or restarts | yes | no: a name a machine; 10 Services | Tailscale's coordination server | nothing | good for remote access, not for names |
| **Cloudflare Tunnel** and Access | nothing | cloudflared, a domain on Cloudflare | no: every request leaves the house | yes, at Cloudflare's edge | first level, free | Cloudflare sees every request in clear | a tunnel per box, or the user's own account | as a ZTNA; hosting, not an intranet |
| **One name, path mode** | as the certificate chosen | as chosen | as chosen | as chosen | not needed | every app on the platform's origin: an app's page acts as its visitor there | as chosen | cut (#213) |
| **Plain HTTP on the LAN** (Home Assistant, Jellyfin, CasaOS) | nothing | nothing | yes | no | n/a | no secure context: no `__Host-` cookies, service workers or passkeys | nothing | no |

## 3. Prior art

Read 2026-10-23. Where a vendor's page could not be read, it says so.

### Home servers: who gets trusted HTTPS on the LAN

Those that do, with nothing on each device, all do it one way: a name
under a vendor's domain that answers the LAN address, a wildcard
certificate under it, and an exception on routers that filter. Those that
keep the key on the box run ACME on it and let the vendor publish only the
challenge.

- **Plex**: `<ip-dashes>.<hash>.plex.direct`, the address written in the
  name, under a wildcard `*.<hash>.plex.direct` per server; DigiCert from
  2015, Let's Encrypt now; not on the PSL. The server downloads it from
  Plex as a `.p12`, by all appearances key and all. Its support page names
  rebinding filters as the common failure, and the fixes:
  `rebind-domain-ok=/plex.direct/` (dnsmasq), `private-domain:
  "plex.direct"` (Unbound, pfSense), or another resolver. "Secure
  connections: Preferred", the default, falls back to plain HTTP. Sign-in
  is a plex.tv account; offline needs the LAN listed as "allowed without
  auth". [How to use secure server connections](https://support.plex.tv/articles/206225077-how-to-use-secure-server-connections/);
  [Filippo Valsorda, 2015](https://words.filippo.io/how-plex-is-doing-https-for-all-its-users/);
  [no internet](https://forums.plex.tv/t/howto-use-plex-with-no-internet/383325);
  [the chain, 2026-09](https://forums.plex.tv/t/https-certs-are-missing-trust-roots/943238).
- **Sandstorm and sandcats.io**, the closest analogue: a self-hosted
  platform that gives every session a random origin of its own, so it
  needed a wildcard. `<name>.sandcats.io` (on the PSL) points at the
  server's public address, not the LAN's. GlobalSign's wildcards until
  2020; since then "your self-hosted Sandstorm server [talks] directly to
  Let's Encrypt", and "the Sandcats.io central server only needs to set
  some DNS records to pass an ACME DNS-01 challenge". The server proves
  itself to sandcats by a client certificate made at install. About 500
  servers used it. The installer prints a setup-token link; its opener
  becomes the admin. [Let's Encrypt for sandcats, 2020](https://sandstorm.io/news/2020-06-13-lets-encrypt);
  [wildcard host](https://docs.sandstorm.io/en/latest/administering/wildcard/);
  [sandcats](https://docs.sandstorm.io/en/latest/administering/sandcats/);
  [install](https://docs.sandstorm.io/en/latest/install/).
- **Unraid**: `<ip-dashes>.<hash>.myunraid.net`, a Let's Encrypt wildcard
  the vendor makes and sends, key and all (the box posts its licence file
  to `provisionwildcard`); not on the PSL; "Strict" mode has no fallback.
  [Securing your connection](https://docs.unraid.net/unraid-os/system-administration/secure-your-server/securing-your-connection/);
  [ProvisionCert.php](https://github.com/unraid/webgui/blob/master/emhttp/plugins/dynamix/include/ProvisionCert.php).
- **Synology QuickConnect**: `<ip-dashes>.<id>.direct.quickconnect.to`
  (on the PSL), a Let's Encrypt wildcard per device in the CT logs
  ([crt.sh](https://crt.sh/?q=kvk2026.direct.quickconnect.to)). Who makes
  the key, not found. **TrueNAS Connect** (paid):
  `<ip-dashes>.<sys>.<acct>.truenas.direct`, ZeroSSL wildcards.
- **Home Assistant**: plain HTTP on the LAN (`homeassistant.local:8123`).
  Its remote UI, `<random>.ui.nabu.casa` (on the PSL), has a Let's Encrypt
  certificate whose key the instance makes and whose DNS-01 record Nabu
  Casa publishes, behind an SNI passthrough proxy. The first visitor to
  onboarding becomes the owner, whose credentials "cannot be recovered".
  [Onboarding](https://www.home-assistant.io/getting-started/onboarding/);
  [authentication](https://www.home-assistant.io/docs/authentication/);
  [hass-nabucasa's acme.py](https://github.com/NabuCasa/hass-nabucasa/blob/main/hass_nabucasa/acme.py).
- **libp2p.direct** (IPFS AutoTLS): `*.<PeerID>.libp2p.direct` (on the
  PSL); the node proves itself with its own key and the broker sets its
  DNS-01 record. [p2p-forge](https://github.com/ipshipyard/p2p-forge).
- **Nextcloud AIO**: a real domain or nothing. Its LAN-only route
  registers a free `dedyn.io` name at deSEC, gets Let's Encrypt through
  Caddy, and asks you to "point your router's DHCP DNS server to the AIO
  host so that all LAN devices resolve the domain locally": this
  proposal's offline setting. [local-instance.md](https://github.com/nextcloud/all-in-one/blob/main/local-instance.md).
- **A root on every device**: StartOS (`start.local`; its guide walks each
  OS through trusting the root, iOS's in five steps) and umbrelOS's local
  HTTPS of 2026 (a CA per box, optional, plain HTTP kept as the way back).
  [StartOS: trust the CA](https://docs.start9.com/start-os/0.4.0.x/trust-ca.html);
  [Umbrel's commit](https://github.com/getumbrel/umbrel/commit/077248d1ff3237d8cd8c5cb8f8a5403b3b8e0bd1).
- **Plain HTTP or a self-signed warning**: Home Assistant, Jellyfin
  ("HTTPS is disabled because it requires an SSL certificate"), CasaOS,
  umbrelOS until 2026; UniFi, QNAP, DSM and TrueNAS by default.
  [Jellyfin networking](https://jellyfin.org/docs/general/post-install/networking/).
  QNAP's myQNAPcloud and DSM's synology.me point at the public address.

### Public names for private addresses

- **nip.io and sslip.io** (now one service) read the address out of the
  name. They "do not support wildcard certificates"; sslip.io once
  published its key, and the certificate was revoked. **traefik.me** served
  its wildcard's key; on 2025-03-24 Let's Encrypt revoked it and blocked
  the key (Baseline Requirements: revocation within 24 hours of a known
  compromise). So each box holds a key of its own, for its own name.
  [nip.io](https://nip.io/);
  [traefik.me #14](https://github.com/pyrou/traefik.me/issues/14);
  [revoking](https://letsencrypt.org/docs/revoking/).
- **Rebinding filters**, by default:

  | Resolver | Private answers to public names | Exception |
  |---|---|---|
  | dnsmasq | kept (`--stop-dns-rebind` is opt-in) | `--rebind-domain-ok=/<domain>/` |
  | OpenWrt | dropped (`rebind_protection` 1) | `list rebind_domain` |
  | Unbound | kept (`private-address` empty) | `private-domain` |
  | pfSense, OPNsense | dropped | a private domain in the resolver |
  | FRITZ!Box | dropped, always | host names, one a line (subdomains: not found) |
  | Google and Nest Wifi | dropped | none: custom DNS servers turn it off |
  | ASUS (Asuswrt, Merlin) | kept (`dns_norebind` 0) | `rebind-domain-ok` (Merlin) |
  | Pi-hole v6 | kept | dnsmasq lines |
  | Google, Cloudflare, Quad9 | kept | |

  [dnsmasq](https://thekelleys.org.uk/dnsmasq/docs/dnsmasq-man.html);
  [OpenWrt](https://openwrt.org/docs/guide-user/base-system/dhcp);
  [Unbound](https://unbound.docs.nlnetlabs.nl/en/latest/manpages/unbound.conf.html);
  [pfSense](https://docs.netgate.com/pfsense/en/latest/services/dns/rebinding.html);
  [OPNsense](https://docs.opnsense.org/manual/unbound.html);
  [FRITZ!Box](https://uk.fritz.com/service/knowledge-base/dok/FRITZ-Box-4020/663_No-DNS-resolution-of-private-IP-addresses/)
  (the page renders only with JavaScript; read from search excerpts);
  [Google Wifi](https://support.google.com/googlenest/answer/9144137?hl=en);
  [Merlin's defaults](https://github.com/RMerl/asuswrt-merlin.ng/blob/master/release/src/router/shared/defaults.c);
  [Pi-hole](https://docs.pi-hole.net/ftldns/configfile/);
  [Tailscale's list](https://tailscale.com/docs/reference/faq/dns-rebinding.md).
  UniFi and eero: not found.
- **iCloud Private Relay**: "An unencrypted DNS server provided by a local
  network … will not be used for iCloud Private Relay traffic"; Safari
  connects "through the proxy using names instead of IP addresses"; "If a
  proxy or ODoH server detects that a specific server name is not a public
  internet name, it instructs the device to try to access the server
  directly over the local network." A network can refuse Private Relay by
  answering `mask.icloud.com` with nothing, and each device is then asked
  to turn it off for that network.
  [Apple's overview](https://www.apple.com/privacy/docs/iCloud_Private_Relay_Overview_Dec2021.PDF);
  [Plex users, 2022](https://forums.plex.tv/t/bug-report-plex-web-app-doesnt-work-with-icloud-relay/806174).
- **Chrome's Local Network Access**: a page's address space is that of the
  address it came from, and local to local needs no permission; so pages
  from the box framing or fetching the box's other names are never asked
  about. A public page reaching the box is.
  [Chrome](https://developer.chrome.com/blog/local-network-access);
  [the spec](https://wicg.github.io/local-network-access/).
- **Encrypted DNS on the device** skips the router: Android's Private DNS
  in strict mode, a custom DoH provider in Chrome. Firefox's DoH treats a
  private answer as invalid (`network.trr.allow-rfc1918` false) and falls
  back to the system's resolver. So the public answer must be right on its
  own: it is the one path every device shares.
  [Firefox TRR](https://wiki.mozilla.org/Trusted_Recursive_Resolver).

### Certificates: DNS-01, limits, lifetimes

- **Wildcards need DNS-01** ("Wildcard issuance must use the DNS-01
  challenge"), and the challenge may be delegated by CNAME or NS to another
  zone; acme-dns is a small server for exactly that.
  [FAQ](https://letsencrypt.org/docs/faq/);
  [challenge types](https://letsencrypt.org/docs/challenge-types/);
  [acme-dns](https://github.com/acme-dns/acme-dns).
- **DNS-PERSIST-01**: one standing record at `_validation-persist.<name>`
  naming the CA and the account, `policy=wildcard` for the wildcard; ballot
  SC-088v3 passed in October 2025. Let's Encrypt aimed for production in
  2026's second quarter; its last word (2026-06-25) is that it will not
  deploy until an open issue in the draft is settled. Google Trust Services
  has shipped it (September 2026) on draft 01, whose record draft 02
  changed. [Let's Encrypt, 2026-02](https://letsencrypt.org/2026/02/18/dns-persist-01);
  [its status thread](https://community.letsencrypt.org/t/dns-persist-01-deployment-status-and-timeline/246468);
  [Google's](https://community.letsencrypt.org/t/google-supporting-dns-persist-01-in-production/251849);
  [the draft](https://datatracker.ietf.org/doc/draft-ietf-acme-dns-persist/).
- **CAA** `accounturi` and `validationmethods` (RFC 8657) tie issuance to
  one ACME account; every CA must honour them from 2027-03-15.
  [Ballot SC-098v2](https://cabforum.org/2026/05/13/ballot-sc098v2-process-rfc-8657-caa-parameters/);
  [Let's Encrypt's CAA](https://letsencrypt.org/docs/caa/).
  **ARI** is RFC 9773.
- **Let's Encrypt's limits**: 50 new certificates per registered domain a
  week (one back every 202 minutes), the registered domain read from the
  PSL; 300 new orders per account per 3 hours; 5 for one exact set of
  names a week; renewals of the same names exempt from the first two, and
  ARI-timed renewals "exempt from all rate limits"; overrides by form, in
  weeks. Unchanged for 45-day certificates.
  [Rate limits](https://letsencrypt.org/docs/rate-limits/).
- **The PSL**: "We do not accept entries whose sole purpose is to
  circumvent rate limits of third parties (such as Let's Encrypt rate
  limits - use their form instead)"; small projects "will likely be
  declined"; a `_psl` TXT record for good; two years of registration left;
  no deadlines. Listing makes each `<box>.<domain>` its own site (cookies,
  SameSite, Chromium's site isolation, passkeys' RP IDs).
  [Guidelines](https://github.com/publicsuffix/list/wiki/Guidelines);
  [the list](https://publicsuffix.org/list/public_suffix_list.dat)
  (checked 2026-10-23 for the domains named here).
- **Lifetimes**: ballot SC-081v3 (2025-04-11) caps a certificate at 200
  days from 2026-03-15, 100 from 2027-03-15 and 47 from 2029-03-15, and
  the reuse of a domain's validation at 200, 100, then 10 days. Let's
  Encrypt's default profile gives 90 days (30 days' reuse) today, 64 days
  (10 days' reuse) from 2027-02-10 and 45 days (7 hours' reuse) from
  2028-02-16; its `tlsserver` profile gives 45 days already, and
  `shortlived` 160 hours. OCSP ended in 2025.
  [SC-081v3](https://cabforum.org/2025/04/11/ballot-sc081v3-introduce-schedule-of-reducing-validity-and-data-reuse-periods/);
  [From 90 to 45](https://letsencrypt.org/2025/12/02/from-90-to-45);
  [profiles](https://letsencrypt.org/docs/profiles/).
- **Other free ACME CAs** for wildcards: ZeroSSL and Google Trust Services
  (each needs an EAB key per account); Buypass stopped issuing in 2025.
  [ZeroSSL](https://zerossl.com/documentation/acme/);
  [Google](https://docs.cloud.google.com/certificate-manager/docs/public-ca-tutorial).
- **Certificate Transparency**: Tailscale's warning for its own per-machine
  certificates, "machine names are still published in the public ledger";
  a wildcard hides the apps. Tailscale also says of its DNS-01 for
  `ts.net`: the keys "are generated and stored locally on your machine and
  Tailscale never sees them". [Enabling HTTPS](https://tailscale.com/kb/1153/enabling-https).
- **In Rust**: instant-acme (DNS-01, ARI, profiles, EAB; maintained) is the
  one to embed; rustls-acme does TLS-ALPN-01 only.
  [instant-acme](https://github.com/djc/instant-acme).

### mDNS and `.local`

No wildcards: a responder answers the names it has claimed one by one
(Avahi publishes one name per process), and Linux's nss-mdns refuses
multi-label names by default. No public certificate (the CA/Browser
Forum's ban on internal names, 2015), and `http://x.local` is not a secure
context. iOS and macOS resolve it; Android since 12 (not over a VPN);
Windows 10 and 11 in their resolver. `home.arpa` (RFC 8375) resolves only
where a home's DNS answers it, and is itself on the PSL.
[RFC 6762](https://www.rfc-editor.org/rfc/rfc6762.html);
[internal names](https://cabforum.org/working-groups/server/internal-names/);
[secure contexts](https://www.w3.org/TR/secure-contexts/);
[Android's resolver](https://source.android.com/docs/core/ota/modular-system/dns-resolver).

### Overlays and tunnels

- **Tailscale**: MagicDNS names a machine `<machine>.<tailnet>.ts.net`,
  and `tailscale cert` gets it a Let's Encrypt certificate by DNS-01 that
  Tailscale publishes, one name per machine. Wildcards are asked for and
  not shipped ([#7081](https://github.com/tailscale/tailscale/issues/7081),
  open since 2023). Services (generally available 2026-02-19) give a
  service its own name and certificate, but "Every Tailscale user,
  including our Personal users, can create up to 10 services". Every device
  needs the app; with the coordination server unreachable, connected peers
  go on, new devices and restarts do not. tsidp signs in only people
  already on the tailnet.
  [MagicDNS](https://tailscale.com/kb/1081/magicdns);
  [Services](https://tailscale.com/blog/services-ga);
  [coordination server down](https://tailscale.com/kb/1091/what-happens-if-the-coordination-server-is-down);
  [tsidp](https://tailscale.com/docs/features/tsidp).
  **Headscale** merged `tailscale cert` in October 2026 (unreleased; its
  base domain must be a public one you control); **NetBird** and
  **ZeroTier** give names but no certificates for them.
- **Cloudflare Tunnel**: a `*.example.com` ingress rule works; Universal
  SSL covers the first level only (deeper needs Advanced Certificate
  Manager); every request goes through Cloudflare's edge, which ends TLS
  and sees it in clear, and nothing works with the WAN down. Access signs
  people in "entirely in the browser" (free to 50 users); private
  hostnames need its client on each device.
  [Configuration](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/configuration-file/);
  [Universal SSL's limits](https://developers.cloudflare.com/ssl/edge-certificates/universal-ssl/limitations/);
  [Access](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/choose-application-type/).

These are the answer for reaching a box from outside the house, later; none
gives a wildcard, trusted HTTPS and no app on the device inside it.

### Companies' self-hosted platforms

- **Coder**: `CODER_WILDCARD_ACCESS_URL` (`*.coder.example.com`) for apps
  on subdomains, which "requir[es] DNS records and TLS certificates";
  without it apps are on paths, which "can make requests to the Coder API
  and pose a security risk"; with no access URL at all it opens a tunnel of
  its own (wgtunnel) for trials. Built-in TLS from PEM files, no ACME;
  OIDC by issuer, client and secret. Its package's unit: `User=coder`,
  `AmbientCapabilities=CAP_IPC_LOCK CAP_NET_BIND_SERVICE`; `install.sh`
  prints the `systemctl enable` and touches no firewall.
  [Setup](https://coder.com/docs/admin/setup);
  [server flags](https://coder.com/docs/reference/cli/server);
  [OIDC](https://coder.com/docs/admin/users/oidc-auth);
  [coder.service](https://raw.githubusercontent.com/coder/coder/main/scripts/linux-pkg/coder.service).
- **Gitpod, now Ona**: self-hosted ended in 2022; its runners in a
  customer's AWS need a certificate "with SANs for `yourdomain.com` and
  `*.yourdomain.com`". [Ona runners](https://ona.com/docs/ona/runners/aws/setup.md).
- **GitLab Pages**: "a wildcard DNS A record pointing to the host", a
  wildcard certificate, a hostname apart from GitLab's "to prevent XSS
  attacks", and the Pages domain on the PSL; "unique domains" per project
  by default, so projects do not share cookies. Omnibus's own Let's Encrypt
  is HTTP-01: no wildcards. [Pages administration](https://docs.gitlab.com/administration/pages/);
  [Omnibus SSL](https://docs.gitlab.com/omnibus/settings/ssl/).
- **Mattermost, Sourcegraph, Backstage**: one name each; Let's Encrypt
  built in (Mattermost; Sourcegraph's Caddy), files, or TLS ended at the
  company's ingress (Backstage's advice); OIDC and SAML.
  [Mattermost TLS](https://docs.mattermost.com/deployment-guide/server/setup-tls);
  [Backstage](https://backstage.io/docs/golden-path/deployment/deploying).
- **Kubernetes**: cert-manager's DNS-01 (Route 53, Azure DNS, Cloud DNS,
  Cloudflare, RFC 2136, acme-dns), its CA and Vault issuers for an internal
  PKI; ExternalDNS run twice for split horizon.
  [DNS-01](https://cert-manager.io/docs/configuration/acme/dns01/);
  [ExternalDNS](https://kubernetes-sigs.github.io/external-dns/latest/docs/faq/).
- **The norms**: a root pushed by Group Policy, an Intune trusted
  certificate profile, or an Apple profile's `com.apple.security.root` (a
  root installed by MDM is trusted at once; by hand, iOS asks for full
  trust in Certificate Trust Settings); split-brain DNS or a conditional
  forwarder; internal ACME from step-ca or Vault PKI (AD CS has none).
  [GPO](https://learn.microsoft.com/en-us/windows-server/identity/ad-fs/deployment/distribute-certificates-to-client-computers-by-using-group-policy);
  [Intune](https://learn.microsoft.com/en-us/intune/device-configuration/certificates/trusted-root-profiles);
  [Apple](https://support.apple.com/102390);
  [split-brain](https://learn.microsoft.com/en-us/windows-server/networking/dns/deploy/split-brain-dns-deployment);
  [step-ca](https://smallstep.com/docs/step-ca/acme-basics/).

### Installers and low ports

A non-root daemon holds 80 and 443 by systemd's
`AmbientCapabilities=CAP_NET_BIND_SERVICE` under `User=` (Coder's and
Caddy's packaged units), by socket activation, by `setcap` on the binary
(Mattermost's docs, AdGuard Home's), or by
`net.ipv4.ip_unprivileged_port_start` (rootless Docker and Podman, which
opens the ports to every user). Root is asked once, by the package or the
installer, never at each start. The installers (Sandstorm's, Coder's,
Tailscale's, k3s's, CasaOS's) install a unit and print the next step; none
changes ufw or firewalld, and k3s's docs say to turn them off or open its
ports. Home Assistant OS and umbrelOS are whole images instead.
[systemd.exec](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html);
[caddy.service](https://raw.githubusercontent.com/caddyserver/dist/master/init/caddy.service);
[rootless Docker](https://docs.docker.com/engine/security/rootless/tips/);
[k3s](https://docs.k3s.io/installation/requirements).

### Sign-in for a household

- **Claiming the box**: the first visitor wins in Home Assistant, Immich,
  Umbrel, Jellyfin and Pocket ID's `/setup`, and that window has been
  exploited (nginx-ui's CVE-2026-42222: a takeover within its setup window).
  A code only the console shows closes it: Sandstorm's setup-token link,
  Jenkins' `initialAdminPassword`, Plex's claim token.
  [nginx-ui advisory](https://advisories.gitlab.com/golang/github.com/0xjacky/nginx-ui/CVE-2026-42222/);
  [Jenkins](https://www.jenkins.io/doc/book/installing/war-file/).
- **Adding a device**: Jellyfin's Quick Connect and Plex's link codes (a
  code on the new device, approved on a signed-in one: RFC 8628's shape,
  fragment's own `/cli`) are phishable: RFC 8628 says so, and device-code
  phishing has been used at scale. An invite link sent by the owner, used
  once, is not.
  [Quick Connect](https://jellyfin.org/docs/general/server/quick-connect/);
  [RFC 8628, 5](https://www.rfc-editor.org/rfc/rfc8628#section-5);
  [Storm-2372](https://www.microsoft.com/en-us/security/blog/2025/02/13/storm-2372-conducts-device-code-phishing-campaign/).
- **Passkeys**: a secure context; the RP ID "a registrable domain suffix
  of or equal to" the origin's domain, never a public suffix (so never
  `home.arpa`), and fixed for good (changing it loses every passkey).
  Chrome refuses WebAuthn on a certificate error. Signing in on the device
  that holds the passkey is local, but no vendor says it works without the
  internet, so it is part of the test. Signing in from another device (QR
  and Bluetooth) needs the internet. NIST's SP 800-63B-4 accepts synced
  passkeys at AAL2.
  [WebAuthn RP ID](https://www.w3.org/TR/webauthn-3/#rp-id);
  [Chromium's origins](https://chromium.googlesource.com/chromium/src/+/main/content/browser/webauth/origins.md);
  [NIST, syncable](https://pages.nist.gov/800-63-4/sp800-63b/syncable/);
  [OWASP](https://cheatsheetseries.owasp.org/cheatsheets/Passkey_Security_Cheat_Sheet.html).
- **Small providers**: Pocket ID (passkeys only, one Go binary with
  SQLite, one-time login codes by QR or link); Kanidm (Rust, one binary,
  passkeys first, single-use enrolment links good for an hour); Authelia;
  Dex (static bcrypt passwords, no passkeys, no self-service).
  Authentik, Zitadel and Keycloak each need a database server.
  [Pocket ID](https://pocket-id.org/docs/setup/user-management);
  [Kanidm](https://kanidm.github.io/kanidm/stable/accounts/authentication_and_credentials.html);
  [Dex local](https://dexidp.io/docs/connectors/local/).

## 4. Today's LAN mode: what stays, what goes

**Stays:**

- **The door** (crates/lan `door.rs`): TLS, routing by host, the
  forwarded headers it alone sets, streams, WebSocket splicing, its limits
  and its tests. It takes its certificate from a source (above) in place
  of the CA.
- **DNS for the zone** (`dns.rs`): the household's offline setting and the
  box's own resolver, for the box's public zone.
- **The cell's settings** (`FRAGMENT_HOST_SUFFIX`, `FRAGMENT_PLATFORM_URL`,
  seam 4's issuer), OpenBao, the box's node, the card browser through the
  door, found 40's intercept of `*.<zone>`: unchanged.
  `CELLD_EXTRA_CA_FILE` stays for companies' private CAs.
- **`cargo xtask dev --lan`**, as the developer's way to run it: on a name
  from the name service's staging (or its fake, with Pebble), on high
  ports, with no sudo at all.

**Goes:**

- **The CA** (`ca.rs`): the root, its name constraint, the iOS profile,
  the root's page and fingerprint, the card browser's root, `--ca-file` at
  pairing, and `CELLD_EXTRA_CA_FILE` on the LAN. Once the name service
  works, unless a house offline for good keeps it (open questions).
- **Dex** (`dex.rs`: the pinned image, its config, bcrypt, the password
  files, `FRAGMENT_LAN_USERS`), for the household's issuer. Companies
  bring their own IdP; the e2e keeps its OpenID Connect fake.
- **`fragment.home.arpa`.** No public CA issues under it, and `home.arpa`
  is itself on the PSL.
- **The box's sudo steps** (docs/self-host-lan.md, 1a to 1e): the name
  follows the box's address (the name service is told), and the
  installer's unit holds the ports, opens the firewall and points the
  box's resolver at its zone, once. The box's browser and CLI need no
  root.
- **The devices' steps** (4, 5, and 7's resolver and root), and the
  troubleshooting rows about roots and resolvers.
- **docs/self-host-lan.md** shrinks to: install, open the link, the
  router's setting for offline, the WAN-unplugged test, `doctor`.

## 5. Open questions for Paul

- **Does Finite run the name service?** It holds the trust and sees the
  lookups of every home that uses it. Without it, a home needs a domain of
  its own and its DNS's API token (the `acme` source): fine for power
  users, not for a family. And the domain's name.
- **Offline:** is one router setting acceptable for offline, and is the
  private CA kept for a house offline for good, or cut?
- **The household's sign-in:** an issuer of our own in `fragment-box`
  (passkeys, a setup code, invite links), or Pocket ID pinned as Dex is
  now (less code, one more process and its database)? Emails as handles
  the owner types, with no mail sent?
- **Box names:** random (`k7q2m9xd`) or chosen (`paul`, as sandcats lets
  you)? Registered anonymously by a key, or tied to a Finite account?
- **The day's test first** (Private Relay on the iPhone, passkeys in
  airplane mode, the ASUS), on a domain Paul holds?
- **Companies that forbid wildcard certificates:** one certificate per app
  on demand from their ACME CA, or not supported?
