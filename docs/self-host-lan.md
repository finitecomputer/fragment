# fragment on the home network, run as an intranet

Status: **re-ported 2026-10-22** onto master's stack (docs/self-host.md,
seam 9), and rehearsed end to end from this box on high ports, short of
sudo, the router and the devices (Evidence, below). The steps marked
**sudo** and the router's and the phone's are Paul's: this guide is them,
in order. docs/self-host.md is the design (seams 2, 4, 7, 9 and 12); this
is how the home network plays a company's.

## What a company's IT would do, and what stands in for it

| A company's IT | Here |
|---|---|
| delegates a subzone to the team's server | `fragment.home.arpa` (RFC 8375's home domain), answered by DNS on the box (192.168.50.7); every other name goes on to the router |
| issues certificates from its CA, and pushes the root to every device | a CA made on the box, **constrained to `fragment.home.arpa`** (it can vouch for nothing else, even if its key leaked); its root installed on the iPhone and the Mac by profile |
| puts one TLS edge in front of the apps | `fragment-lan`'s front door on :443: `https://fragment.home.arpa`, fragments at `https://<label>--<suffix>.fragment.home.arpa`, Dex at `https://dex.fragment.home.arpa`; WebSockets and event streams pass through |
| runs an identity provider | Dex v2.46.1 (pinned by digest), one static user per person, passwords in files; the cell signs people in through OpenID Connect |
| runs a secrets service | OpenBao 2.7.1 (pinned by checksum), on loopback: the cell's own secrets, read with a token that reads nothing else (docs/self-host.md, seam 12) |
| runs the model on its own network | Bonsai (NInfer on this box, behind Caddy: `http://bonsai.localhost/v1`) |
| runs the workloads on its own machines | sandcastle's engine on the box (microVMs, jailed) behind a `sandcastle-node` the stack runs; a person's own machine (the Mac) paired to them |
| opens the firewall to the office subnet only | ufw: 53, 80 and 443 from 192.168.50.0/24 |
| runs a mail server | **none yet**: the platform sends no mail (docs/technical-debt-ledger.md, "No mail without Email Sending"). Invite people by email as usual and **share the link yourself**: the invite is met when they sign in with that email |

## What runs on the box

`cargo xtask dev --lan` (xtask/src/lan.rs) runs the stack on celld and,
beside it, everything below, each its child: one terminal, one Ctrl-C.

| Process | Listens | What |
|---|---|---|
| `fragment-lan serve` (crates/lan) | 192.168.50.7: 53 (UDP and TCP), 80, 443 | DNS for the zone; the front door (TLS, HTTP/1.1, upgrades); the root's page over http |
| celld | 127.0.0.1:8790 | the cell, on loopback: devices reach it only through the door |
| Dex | 127.0.0.1:8806 | the issuer `https://dex.fragment.home.arpa`, behind the door |
| OpenBao | 127.0.0.1:8804 | the cell's own secrets (the host secret, the code store's key, Dex's client secret, the model key): unsealed from its key file at each start, seeded by the stack, read by the cell with its token |
| `sandcastle-node serve` | 127.0.0.1:8808 | the box's node, in front of the engine (`/var/lib/sandcastle`): computers are microVMs there, Hermes their image. Its platform is the front door, under the root |
| the code store, the model fake | 127.0.0.1:8792, :8796 | as `cargo xtask dev` (the model fake answers only image steps; Bonsai answers text) |
| the card renderer | 127.0.0.1, a free port | preview cards: the pinned chrome-headless-shell, which reaches fragments through the door and trusts the root |

State lives in `FRAGMENT_LAN_STATE`, by default `~/.local/state/fragment-lan`
(outside the checkout, so `cargo clean` never takes the root the devices
trust):

- `ca/`: the root (`ca.key`, 0600; `ca.pem`, `fragment-ca.crt`,
  `fragment-ca.mobileconfig`) and the zone's certificate (`zone.pem`,
  `zone.key`), issued fresh at each start for 397 days (iOS allows 825);
- `dex/`: `dex.yaml` (0600), the client's secret, and `passwords/<name>`
  (0600), one per person;
- `openbao/` (0700): its seal key (`seal.key`, 0600), its data, the
  stack's AppRole ids and the cell's token, each 0600. The stack seeds
  every secret into it at each start, so removing it loses nothing;
- `node/` (0700): the box's node's config and its secret (`node.secret`,
  0600, made once);
- `nodes.json`: the node list the cell is given, written at each start
  from the images `cargo xtask node-images` loaded last (and any
  `node-images-<arch>.json` here: the Mac's, step 7);
- `models.jsonc` and `model.key`: the deployment's models (step 2), yours;
- `serve.json`: the front door's config.

The platform's own state (people, agents, fragments) is the checkout's, as
`cargo xtask dev`'s is: `cell/.celld/dev` and `target/devstack`. `--clean`
starts it afresh and keeps the LAN's state.

Logs: `target/devstack/lan-door.log` (a device that does not trust the
root yet shows there as `TLS from <addr> failed: … UnknownCA` or
`BadCertificate`), `lan-dex.log`, `lan-node.log` beside it,
`$FRAGMENT_LAN_STATE/openbao/openbao.log`, and the cell's log the banner
names (`node log:`).

## 1. The box, once (sudo)

**1a. A fixed address.** The zone answers 192.168.50.7, so the box must
keep it. On the ASUS router (http://192.168.50.1): LAN, DHCP Server,
"Manually Assigned IP around the DHCP list": Enable, then add the box
(omarchy) with 192.168.50.7, and Apply.

**1b. The privileged ports.** Only `fragment-lan` binds 53, 80 and 443.
Give that one binary, at a path only root can write, the right to (in
step 2's checkout: make it first):

```sh
cargo build --release -p fragment-lan
sudo install -D -o root -g root -m 0755 target/release/fragment-lan /usr/local/lib/fragment-lan/fragment-lan
sudo setcap cap_net_bind_service=+ep /usr/local/lib/fragment-lan/fragment-lan
getcap /usr/local/lib/fragment-lan/fragment-lan   # cap_net_bind_service=ep
```

The other way: `echo 'net.ipv4.ip_unprivileged_port_start=53' | sudo tee
/etc/sysctl.d/50-fragment-lan.conf && sudo sysctl --system`.

Tradeoff: setcap lets only that root-owned binary take low ports, but a
new build whose config changed needs the install again (xtask refuses a
stale one and says so); the sysctl needs no reinstall, but lets any
program of any user on the box take 53 to 1023.

**1c. The firewall,** to the home subnet only:

```sh
sudo ufw allow from 192.168.50.0/24 to any port 53 comment 'fragment-lan DNS'
sudo ufw allow from 192.168.50.0/24 to any port 80 proto tcp comment 'fragment-lan root page'
sudo ufw allow from 192.168.50.0/24 to any port 443 proto tcp comment 'fragment-lan front door'
sudo ufw status numbered
```

**1d. The box's own resolver.** The cell fetches Dex at
`https://dex.fragment.home.arpa`, and the box's node calls the front door,
so the box asks its own DNS for the zone (and the router for everything
else, as now):

```sh
sudo mkdir -p /etc/systemd/resolved.conf.d
printf '[Resolve]\nDNS=192.168.50.7\nDomains=~fragment.home.arpa\n' | sudo tee /etc/systemd/resolved.conf.d/fragment-lan.conf
sudo systemctl restart systemd-resolved
```

Check it once the stack runs (step 3): `resolvectl query
dex.fragment.home.arpa` answers 192.168.50.7.

**1e. Optional: the box's browser and CLI.** Chromium on the box trusts
the root with no sudo, once step 3 has made it: `mkdir -p ~/.pki/nssdb &&
certutil -d sql:$HOME/.pki/nssdb -A -t C,, -n 'fragment LAN CA' -i
~/.local/state/fragment-lan/ca/ca.pem` (restart Chromium after). The
`fragment` CLI trusts the system's store: `sudo trust anchor --store
~/.local/state/fragment-lan/ca/ca.pem`.

## 2. Once, no sudo: the checkout, the model, the images

**The checkout.** The self-hosted lane is the branch `selfhost` (draft
#133, never merged), and it is force-moved when it is ported again, so run
it from a worktree of its own, never one you edit:

```sh
git -C ~/dev/finite/fragment fetch origin
git -C ~/dev/finite/fragment worktree add --detach ~/dev/finite/fragment-home origin/selfhost
cd ~/dev/finite/fragment-home
# later, to update: git fetch origin && git checkout --detach origin/selfhost
```

**The model**, Bonsai, for every tier (a deployment's `models`, as its
config writes them: docs/self-host.md, seam 3), at its name on this box,
`http://bonsai.localhost/v1` (Caddy on port 80, routing by name to
NInfer; a local fleet's model may be http to this machine by any local
name). Bonsai has no list price, so each slot names its ceiling, zero.
NInfer takes no key, but the slot names one, so the key's file holds any
word. Each slot's patience
is 900 s: a local model can stream nothing for longer than the agent's
20 s for cloud models (a long prompt, another caller ahead of it on the
GPU, a tool call sent whole at its end), and the agent would give up on
it (docs/self-host.md, found 42):

```sh
mkdir -p -m 700 ~/.local/state/fragment-lan
cat > ~/.local/state/fragment-lan/models.jsonc <<'JSON'
{
  "cheap":    { "provider": "openai", "base_url": "http://bonsai.localhost/v1", "key": "fragment-bonsai-key", "model": "bonsai-2-27b",
                "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } }, "stale_timeout_s": 900 },
  "medium":   { "provider": "openai", "base_url": "http://bonsai.localhost/v1", "key": "fragment-bonsai-key", "model": "bonsai-2-27b",
                "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } }, "stale_timeout_s": 900 },
  "fallback": { "provider": "openai", "base_url": "http://bonsai.localhost/v1", "key": "fragment-bonsai-key", "model": "bonsai-2-27b",
                "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } }, "stale_timeout_s": 900 }
}
JSON
(umask 077; echo none > ~/.local/state/fragment-lan/model.key)
curl -s http://bonsai.localhost/v1/models   # names bonsai-2-27b
```

**The images.** Computers run Hermes (our image) in microVMs on the box's
engine. Build the stub and Hermes here and load them into the engine
(Docker: your user in the docker group). The first build takes some
minutes, most of it Hermes'; again, with Docker's cache, under a minute.
Run it again after each update that changes `images/`: a computer takes
the new image at its next wake (an awake one is told an update is ready).

```sh
cargo xtask node-images
```

It prints each image pinned by the digest the engine gave it, and records
them (`target/devstack/node-images.json`): the stack's node list takes
them at its next start.

## 3. Start it

```sh
cd ~/dev/finite/fragment-home
export CELLD_BIN=$HOME/dev/finite/celld/target/release/celld
export SANDCASTLE_NODE_BIN=$HOME/.local/opt/sandcastle/bin/sandcastle-node
export FRAGMENT_LAN_BIN=/usr/local/lib/fragment-lan/fragment-lan   # step 1b's (omit with the sysctl)
export FRAGMENT_LAN_USERS=paul,mac-test   # the people; mac-test tries the Mac (step 7)
export FRAGMENT_BYOC=on                   # people pair machines of their own (step 7); a company leaves it off
cargo xtask dev --lan
```

The first start builds the cell (a few minutes in a fresh checkout),
fetches Dex, OpenBao and the card browser (about 250 MB, once), makes the
CA and initialises OpenBao; then it is ready in seconds. The banner says
where everything is: the front door, the DNS server and its upstream, the
root's page with its **SHA-256 fingerprint** (write it down: step 5
compares it), each person with their password's file, and the box's node
(`computers:`).

From the box, once 1d is done:

```sh
resolvectl query anything.fragment.home.arpa               # 192.168.50.7
curl --cacert ~/.local/state/fragment-lan/ca/ca.pem -s -o /dev/null -w '%{http_code}\n' https://fragment.home.arpa/   # 200
```

**The people** are `FRAGMENT_LAN_USERS` (default `paul`). Each signs in as
`<name>@fragment.home.arpa` with the password in
`~/.local/state/fragment-lan/dex/passwords/<name>`: made on first start
as `xxxx-xxxx-xxxx-xxxx-xxxx`, lower-case letters and digits, easy to type
on a phone. Read it on the box with `cat`. To choose your own instead,
write it there before the first start (12 characters or more):

```sh
mkdir -p -m 700 ~/.local/state/fragment-lan/dex/passwords
( umask 077; read -rs p && printf '%s' "$p" > ~/.local/state/fragment-lan/dex/passwords/paul )
```

**Stopping:** Ctrl-C. It takes under a second and frees every port; the
box's node stops last, once the cell has. Computers that were awake keep
running on the engine while the stack is down; the next start finds them
as they were, and they sleep when idle. To see them: `curl -s
--unix-socket /var/lib/sandcastle/engine.sock
http://engine/v1/containers`.

Every setting, with its default: `FRAGMENT_LAN_STATE`
(`~/.local/state/fragment-lan`), `FRAGMENT_LAN_ZONE` (fragment.home.arpa),
`FRAGMENT_LAN_ADDR` (the box's address toward its default gateway),
`FRAGMENT_LAN_BIND` (that address), `FRAGMENT_LAN_DNS_UPSTREAM` (the
gateway, :53), `FRAGMENT_LAN_HTTPS_PORT` (443), `FRAGMENT_LAN_HTTP_PORT`
(80, 0 for none), `FRAGMENT_LAN_DNS_PORT` (53, 0 for none),
`FRAGMENT_LAN_ENGINE` (`/var/lib/sandcastle`; `none` for no computers),
`SANDCASTLE_NODE_BIN` (`sandcastle-node` on the PATH), `DEX_BIN` (the
pinned Dex), `FRAGMENT_SECRETS_BACKEND` (`openbao`; `vars` puts the
secrets in the cell's `.dev.vars` instead), `FRAGMENT_OPENBAO_TARBALL`
(the pinned tarball, from a path), `FRAGMENT_MODELS_FILE` and
`FRAGMENT_MODEL_KEY_FILE` (the state's `models.jsonc` and `model.key`),
`FRAGMENT_NODES_FILE` (a node list of your own in place of the one the
stack writes, `nodes.json`: copy that, and keep its `box`; the box's node
runs either way: docs/self-host.md, seam 2), `FRAGMENT_COMPUTER_IMAGE`
(`hermes`),
and `FRAGMENT_DEV_PORT` (8790: the cell, and Dex, OpenBao and the box's
node above it). Preview cards reach fragments through the door (its first
address, on the https port) and trust the root, unless
`FRAGMENT_BROWSER_UPSTREAM` or `FRAGMENT_BROWSER_CA_FILE` say otherwise.

## 4. Point the iPhone at the box's DNS

**On the iPhone alone** (start here): Settings, Wi-Fi, the (i) beside the
home network, Configure DNS, **Manual**. Delete the servers listed, Add
Server, `192.168.50.7`, Save.

**Or for the whole house, on the router**: the ASUS's LAN, DHCP Server,
"DNS and WINS Server Setting": DNS Server 1 `192.168.50.7`, DNS Server 2
empty, "Advertise router's IP in addition to user-specified DNS" **No**,
Apply. Devices take it at their next lease (toggle Wi-Fi to hurry one).
Every device then depends on the box for all its DNS: while the box is
off, the house has none. Leave DNS Server 2 empty: a device asks either
server, and the router does not know the zone.

With Asuswrt-Merlin, the router can stay everyone's DNS and forward only
the zone to the box, as a company delegates a subzone. Administration,
System, "Enable JFFS custom scripts and configs": Yes; then over ssh:

```sh
printf 'server=/fragment.home.arpa/192.168.50.7\nrebind-domain-ok=/fragment.home.arpa/\n' >> /jffs/configs/dnsmasq.conf.add
service restart_dnsmasq
```

**iCloud Private Relay** sends Safari's lookups past the network's DNS. If
Safari cannot find `fragment.home.arpa`, turn it off for this network:
Settings, Wi-Fi, (i), "iCloud Private Relay" off.

## 5. Trust the root on the iPhone

1. In **Safari** (profiles download only there), open
   `http://192.168.50.7/ca`. Tap "iPhone, iPad or Mac: the profile", then
   Allow. "Profile Downloaded".
2. Settings, General, **VPN & Device Management**, "fragment on
   fragment.home.arpa" under Downloaded Profile. Tap More Details, then the
   certificate: its SHA-256 fingerprint must be the one the box's banner
   printed (the page over http could have been swapped on the way; the
   banner could not). Back, **Install**, your passcode, Install, Install
   (iOS warns of an unmanaged root), Done.
3. Settings, General, About, **Certificate Trust Settings** (at the bottom).
   Under "Enable full trust for root certificates", turn on "fragment LAN
   CA (omarchy)", Continue.

The root says, inside it, that it is valid for `fragment.home.arpa` and the
names under it only: the phone will not trust it for any other site.

## 6. Sign in from the iPhone, and talk to Hermes

1. Safari: `https://fragment.home.arpa` (type the `https://`; a bare name
   may go to a search). The padlock is the root's. "Welcome to Finite":
   **Sign In**.
2. Dex asks for an email and a password: `paul@fragment.home.arpa` and the
   password from step 3. Let iCloud Keychain keep it.
3. **Your first chat** opens at once: the shell makes your first agent
   with a name it picks ("Setting up your agent": made, in this chat,
   starting on its computer), and it greets you. Your computer is a Hermes
   microVM on the box; its model is Bonsai.
4. **Talk to it.** Each tool it uses is a step above its reply ("Worked
   through 1 step": tap it to see the command). A computer left idle
   sleeps; your next message wakes it.
   - **When it asks you something, answer in the chat**: your next message
     is the answer, while its turn waits. Stop works while it waits.
5. **An app**: the agent makes apps from a message. Open the desktop (the
   panel button at the top right): with no apps yet it offers three ways
   to ask (the die deals three more; later, **Add** in its dock). Tap one,
   make it yours in the composer, and send it. The agent builds it on its
   computer with the `fragment` CLI and deploys it, a minute or two on
   Bonsai; the app appears in the dock. It opens the app from its own
   computer to check it, with its browser or `curl` (a computer reaches
   no private address, but the platform's own hosts reach the platform
   through it: docs/computers.md, "The platform's hosts"), and gives you
   its link. Its preview card (the page as the platform shot it after the
   deploy) shows over its icon where there is a pointer, on the Mac; a
   phone shows the icon.
6. **Live**: tap the app in the dock (it opens in a window), and open its
   address in a second Safari tab (the window's ⋯, Copy link): each says
   "you and 1 other here". Add something in one; it shows in the other at
   once.
7. **Someone else**: the app window's share button (**Share…**), their
   email (`mac-test@fragment.home.arpa`), Editor or Viewer, **Invite**.
   No mail goes out, and the sheet says so, showing the app's address:
   send it to them yourself. When they sign in with that email, the
   invite is met and the app is in their list.

Add the shell to the home screen (Share, Add to Home Screen) for an app of
its own. Push notifications need Apple's push service and the internet;
offline, the shell's live channel tells an open tab instead
(docs/self-host.md, seam 8).

## 7. The Mac

**DNS, for the zone only** (the rest stays as it is; this is split DNS, as
a company's VPN does it):

```sh
sudo mkdir -p /etc/resolver
echo 'nameserver 192.168.50.7' | sudo tee /etc/resolver/fragment.home.arpa
dscacheutil -q host -a name dex.fragment.home.arpa   # ip_address: 192.168.50.7
```

(`dig` and `nslookup` skip `/etc/resolver`; `dscacheutil`, Safari and
everything else use it.)

**The root**, for Safari, Chrome, `curl` and the `fragment` CLI:

```sh
curl -sO http://192.168.50.7/ca/fragment-ca.crt
openssl x509 -inform der -in fragment-ca.crt -noout -fingerprint -sha256   # the banner's
sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain fragment-ca.crt
curl -so home-ca.pem http://192.168.50.7/ca/ca.pem    # the same root as PEM, for the Mac's Linux VM (below)
```

Or open the profile (`http://192.168.50.7/ca`) and install it in System
Settings, General, Device Management. Firefox keeps its own store: in
`about:config`, `security.enterprise_roots.enabled` true.

**The CLI** (it trusts the OS's roots beside the public ones), built from
the same checkout:

```sh
cargo build --release -p fragment-cli && cp target/release/fragment ~/.local/bin/
export FRAGMENT_HOST=https://fragment.home.arpa
fragment login        # it opens the link: sign in through Dex, check the code, approve
fragment whoami
```

**Its sandcastle node: pair it** (the Mac runner; experimental:
docs/self-host.md, seam 2, Bring your own computer). It dials the box, so
the Mac needs no open port, and it becomes a person's node, for their
computers alone. With sandcastle's engine running in the Mac's Linux VM
(sandcastle's docs/mac.md, steps 1 to 6, and its "The CA" with
`home-ca.pem` from above):

1. The images, in the VM (with Docker and Rust), from a checkout of the
   same branch: `cargo xtask node-images`. It builds them for arm64 there
   and loads them into the VM's engine, named by the digests that engine
   gives them, which name nothing on the box's. Give the box the VM's
   record, and restart the stack (Ctrl-C, then step 3's command): its
   list names each image by architecture, the VM's for arm64.

   ```sh
   # on the box: the VM's record, as its architecture's
   scp <the VM>:<its checkout>/target/devstack/node-images.json ~/.local/state/fragment-lan/node-images-aarch64.json
   ```

   Again after each `node-images` in the VM. (The arm64 Hermes image has
   not been built whole yet; its stages have.)
2. Pair, in the VM:

   ```sh
   sudo install -d -m 0700 -o "$(id -un)" /etc/sandcastle-node   # once: the node (you, not root) writes its config and secret here
   sandcastle-node pair https://fragment.home.arpa --config /etc/sandcastle-node/node.json --name mac \
     --ca-file /etc/sandcastle/home-ca.pem \
     --engine /var/lib/sandcastle/engine.sock --ports /var/lib/sandcastle/ports.sock \
     --egress /run/sandcastle-node/egress.sock
   ```

   It prints a link and a code.
3. **Who it is for.** A computer is placed once, at its first start, and
   stays (moving is not built), and the shell starts a person's first one
   when they first open it. Yours already runs on the box, so try the Mac
   with the second person, `mac-test` (step 3's `FRAGMENT_LAN_USERS`): on
   the Mac, in a private window, open the link, sign in as
   `mac-test@fragment.home.arpa` (its password is
   `~/.local/state/fragment-lan/dex/passwords/mac-test` on the box), check
   the page shows the same code, tick **Run my new computers on it**, and
   tap **Add this node**. The command ends having written
   `/etc/sandcastle-node/node.json` (the platform, its uplink as the id
   the box gave it, `ca_file`) and the secret (`node.secret`, 0600; never
   shown).
4. Start it with docs/mac.md's unit (its `RuntimeDirectory` makes
   `/run/sandcastle-node`), on `/etc/sandcastle-node/node.json`, and wait
   for its log's `uplink: connected`. **Before step 5**: a first computer
   whose chosen node is down goes to the box instead, for good.
5. "Node added" says "Your new computers run on it": **Open the shell**,
   and mac-test's first agent starts on the Mac. Settings, Computers
   (experimental) shows the box, and `mac` (theirs, Up, "Your computer
   runs here"). Revoke it there, and it is cut off at once.

You pair a node for yourself the same way; the tick only matters for a
person with no computer yet (yours stays on the box).

## 8. The WAN-unplugged test

It shows the whole loop needs nothing outside the house.

1. Everything working (steps 1 to 6). On the iPhone, turn **Cellular Data
   off**: otherwise iOS sends traffic over cellular once Wi-Fi has no
   internet, and the test proves nothing.
2. Unplug the router's WAN cable (or: WAN, Internet Connection, "Enable
   WAN" No).
3. On the iPhone: sign out and sign in again through Dex; open an app;
   change something and watch it update live in a second tab; ask an agent
   something and get Bonsai's answer.
4. On the box, nothing of the stack holds a connection past the house:

   ```sh
   ss -tnp state established '( not dst 192.168.50.0/24 and not dst 127.0.0.0/8 and not dst [::1] )' | grep -E 'celld|dex|fragment-lan|sandcastle|bao|ninfer'
   ```

   prints nothing. The door's log shows the DNS forwards that failed (other
   names, now unreachable): those are devices' lookups, not the stack's.
5. Expected gaps, each a known debt: the shell's fonts (Google Fonts) fall
   back to the system's (docs/self-host.md, seam 11); an agent's skills
   that fetch from npm or a CDN fail; push is off.
6. Plug the WAN back in (and Cellular Data back on).

## 9. Undo it all

```sh
# the box
sudo rm /etc/systemd/resolved.conf.d/fragment-lan.conf && sudo systemctl restart systemd-resolved
sudo ufw status numbered          # then, for each fragment-lan rule, highest number first:
sudo ufw delete <n>
sudo rm -r /usr/local/lib/fragment-lan            # 1b, setcap's way
sudo rm /etc/sysctl.d/50-fragment-lan.conf && sudo sysctl -w net.ipv4.ip_unprivileged_port_start=1024   # 1b, the sysctl's way
certutil -d sql:$HOME/.pki/nssdb -D -n 'fragment LAN CA'                  # 1e
sudo trust anchor --remove ~/.local/state/fragment-lan/ca/ca.pem          # 1e
rm -r ~/.local/state/fragment-lan                 # the CA, Dex's secrets, OpenBao, the node's: every device's trust in it ends here
git -C ~/dev/finite/fragment worktree remove --force ~/dev/finite/fragment-home
# the Mac
sudo rm /etc/resolver/fragment.home.arpa
sudo security delete-certificate -c 'fragment LAN CA (omarchy)' /Library/Keychains/System.keychain
```

- The iPhone: Settings, Wi-Fi, (i), Configure DNS, **Automatic**; Settings,
  General, VPN & Device Management, the profile, **Remove Profile** (that
  removes the root and its trust).
- The router: DNS Server 1 empty and "Advertise router's IP" Yes (or remove
  Merlin's two lines and `service restart_dnsmasq`); the box's manual DHCP
  assignment, if it is no longer wanted.

## When something fails

| What you see | Why, and what to do |
|---|---|
| Safari: "This Connection Is Not Private" | the root is installed but not trusted: step 5.3. The door's log says `UnknownCA` or `BadCertificate` for the phone's address |
| Safari: "Safari can't find the server" | DNS: step 4's server (`192.168.50.7` alone); Private Relay; the stack is not running |
| `fragment-lan serve` exited: permission denied on :53, :80 or :443 | step 1b; with setcap, `FRAGMENT_LAN_BIN` must name the installed copy |
| xtask: `… says "", not "fragment-lan 1"` | the installed `fragment-lan` is older than this checkout: step 1b's install and setcap again |
| `fragment-lan`: the address is in use | another server holds 53, 80 or 443 on 192.168.50.7 |
| xtask: `no target/devstack/node-images.json yet` | step 2's `cargo xtask node-images` has not run in this checkout |
| xtask: `the sandcastle engine at /var/lib/sandcastle …` | the engine's service is down (`systemctl status sandcastle-engine`), or this user cannot reach its socket; `FRAGMENT_LAN_ENGINE=none` runs with no computers |
| xtask: `start sandcastle-node (SANDCASTLE_NODE_BIN …)` | step 3's `SANDCASTLE_NODE_BIN` |
| a new agent's chat says its computer could not start, or `node_down` | the box's node is down (`lan-node.log`), or the box cannot resolve the zone (step 1d): its node reaches the platform through the front door |
| Sign-in: "the sign-in provider's metadata did not answer" | the box cannot resolve the zone (step 1d), or Dex is down (`lan-dex.log`) |
| Dex: "Invalid Email Address and password" | the email is `<name>@fragment.home.arpa`; the password is the file's |
| an app's icon in the dock, never its card | the card renderer could not reach the door: see the stack's output for `card browser:` lines |
| the CLI: "unreachable, or it dropped the connection" | its machine does not trust the root yet (step 7, or 1e on the box), or cannot resolve the zone |
| "the cell is not answering" (502) | the stack is starting or stopped; see the cell's log |
| xtask: `OpenBao stays sealed: its seal key … does not open …` | `openbao/seal.key` is not the one its data was sealed with: put it back, or remove `~/.local/state/fragment-lan/openbao` (the next start seeds every secret again) |
| a page or the CLI: `host_failed`, `SecretsError [down]: OpenBao at http://127.0.0.1:8804 is not answering` | OpenBao stopped (it stops with the stack): start the stack again; `openbao/openbao.log` |
| `SecretsError [refused]: … refused the cell's token` | the cell's token lapsed (30 days unrenewed) or was revoked: start the stack again, which mints another |

## Evidence, 2026-10-22: the guide rehearsed

From a fresh worktree of this branch's tip (`../fragment-home-rehearse`,
detached, as step 2 makes one), its own build of everything, with the
LAN's state in scratch rather than `~/.local/state/fragment-lan` (Paul's
stays untouched), on celld v0.6.3 (the fork's `selfhost` line), the real
engine (`/var/lib/sandcastle`, jailed microVMs; empty before and after),
and Bonsai-2-27B under NInfer. In place of what needs sudo, the router or
the devices:

- **1b, 1c**: high ports, `FRAGMENT_LAN_HTTPS_PORT=9543
  FRAGMENT_LAN_HTTP_PORT=9580 FRAGMENT_LAN_DNS_PORT=9553`, the door on
  192.168.50.7 alone (the default bind).
- **1d**: the stack, the CLI and the Mac's node ran in a mount namespace
  whose `/etc/hosts` maps the zone's two names to 192.168.50.7
  (`unshare -rm`, then an inner user namespace back to uid 1000).
- **The iPhone** (4, 5): headless Chromium 152 at 390×844 (3×), touch, an
  iPhone's user agent, a home of its own whose NSS database trusts the
  root (1e's `certutil`), the zone mapped by `--host-resolver-rules`.
- **The Mac** (7): a second such browser at 1280×900 (its "private
  window"), the CLI with `SSL_CERT_FILE` naming the root (as the Mac's
  keychain would), and sandcastle's Docker engine double with
  `sandcastle-node` from sandcastle's `node` (3748374).

Screenshots: `target/rehearse/shots/` in the `selfhost-port-lan`
worktree.

| Step | What happened |
|---|---|
| 2 | the models file and key as written; `node-images` 18 s with Docker's cache (both images' disks already built on the engine) |
| 3 | ready 128 s after the command in the fresh checkout (the cell's build 71 s; OpenBao, Dex and the card browser fetched once), then in 12 to 19 s; the banner as described. The zone's names answered by the box's DNS with `aa` over UDP and TCP, every other name the router's; the shell 200 under the root |
| 5 | the root's page by address over http, its fingerprint the banner's, the profile as `application/x-apple-aspen-config`; before the root was trusted, "Your connection is not private", and the door logged each refused handshake |
| 6.1–6.3 | Sign In, Dex's form, back signed in: 2.5 s. "Your first chat" at once; the first agent's computer placed on `box` and awake as the page landed; **its greeting 9 s later** (Bonsai's call 5.5 s, 15,945 tokens in) |
| 6.4 | "What is 17 times 23?": "17 × 23 = 391." in 2.8 s. "Run `uname -a` and `nproc`": one terminal step, "Linux 6.12.91, x86_64, 2 CPU cores" (the microVM's kernel), in 4.8 s |
| 6.5 | the desktop's "Make a shared grocery list my family can add to from their phones", sent as it came: Hermes made `grocery-list--h498` from the todo template and deployed it through the platform in 90 s; its card shot in 1.3 s (through the door, under the root) at each deploy. Then it tried to open the page from its computer, which reaches no private address, and its turn ended 36 steps in, mid-investigation, without the link (step 6.5 now says so) |
| 6.6 | the app in a window from the dock and in a second tab, each "you and 1 other here"; "Oat milk" added in one showed in the other **231 ms** later |
| 6.7 | Invite to an email no one signs in as: the sheet says no mail goes and shows the app's address (it said "we mailed them a link" before this branch's fix) |
| 7, CLI | `fragment login` printed its code; typed at `/cli` on the phone, the page showed the same key's tail, Add this key; the login ended by itself, `whoami` paul@fragment.home.arpa |
| 7, pairing | `sandcastle-node pair` through the door with the root; mac-test signed in through Dex from its link in the Mac's browser, saw the same code, ticked Run my new computers on it, Add this node: "mac is yours now, as paired-…"; the command wrote the config and its 0600 secret; the node's uplink connected through the door. Open the shell: mac-test's first agent placed on the Mac "as its owner chose"; `hostname` and `uname -r` there in 4 s (a Docker container, the host's kernel); Settings, Computers: `box` Up, the platform's; `mac` Up, theirs, "Your computer runs here", points not charged. Their invite to the grocery list met at that sign-in |
| 8, in part | while all of it ran, no process of the stack held a connection past the house |
| stopping | Ctrl-C: 0.2 s, every port freed (TCP, UDP and the sockets), nothing left running; the next start found Paul's computer awake as it was (its file kept). The Mac's node and double stopped on Ctrl-C, the double removing its container |

**The stand-in Mac shares the box's architecture**, and a node list names
one reference per architecture: the box's digest pins name nothing the
Docker double holds (it runs Docker's images by name). So step 7 ran on a
list of its own (`FRAGMENT_NODES_FILE`, a copy of `nodes.json` naming both
images by their `:local` tags, which both engines hold), restarting the
stack. A real Mac is arm64, with its own digests (step 7.1).

**What the rehearsal found, and what changed:**

- **Ctrl-C rolled awake computers back.** The stack's node was in the
  terminal's group, so it stopped at once while celld drained (15 to 25 s):
  each awake computer's `wait` was cut, the node did not answer the look
  after it, and the cell read that as the container's exit, started
  another and, at the next start, restored a save 12 minutes old. The node
  now has a group of its own and stops after the cell (dac508b3). The
  cell's reading of a node that does not answer as an exit is seam 2's,
  and stands for any node outage: Open questions.
- **The share sheet said it mailed an invite** it could not (no mailer):
  it now says no mail goes, and shows the address to send (9b929d89).
- **Fragments' names** in the banner and the guide were master's old
  `<label>--<username>`; they are `<label>--<suffix>`.
- **The guide**: the first agent is made at sign-in; apps are asked of the
  agent (the shell has no catalog); a phone shows no card; the agent cannot
  open its app from its computer on the LAN; the CLI's `login` and its
  `/cli` code; one terminal (the stack runs the box's node).

Not rehearsed: ports 53, 80 and 443 (sudo), setcap, ufw,
systemd-resolved, the router, the iPhone's Safari itself (its profile,
Private Relay, the home screen), the Mac's Linux VM and the arm64 images,
and the WAN unplugged.

## Design notes

- **One process holds the privileged ports**, and it parses only DNS, TLS
  and HTTP heads; it holds no secret but the zone's key.
- **The root is constrained to the zone.** An installed root is otherwise
  trusted for every site; with RFC 5280 name constraints, a leak of this
  box's key reaches nothing but `fragment.home.arpa`.
- **The zone's certificate is a wildcard** for its one level: every
  fragment, Dex and a computer share it, and nothing pins it, so it is
  issued fresh each start. A company that bans wildcards gives one
  certificate per name, or path mode (docs/self-host.md, seam 9).
- **The cell is the same cell**: its zone (`FRAGMENT_HOST_SUFFIX`), its
  origin (`FRAGMENT_PLATFORM_URL`, https), its issuer, and a root for its
  own fetches (`CELLD_EXTRA_CA_FILE`) are configuration. It takes the
  scheme from `x-forwarded-proto`; the door replaces that header and
  `x-forwarded-host` on every request, and celld listens on loopback only.
- **DNS forwards, it does not resolve.** The router's answers reach the
  device unchanged; a company's resolver would forward this zone to the box
  (Merlin's two lines are exactly that).
- **The box's node calls the platform at the front door**, under the root,
  as a node anywhere would: a computer's intercepts go to
  `<platform>/api/nodes/egress`, which the platform takes on its own host
  alone.
- **OpenBao listens on loopback alone.** Only the cell reads it, on the
  same box; the door never routes to it. A company's Vault is reached by
  name over TLS instead, its CA in `CELLD_EXTRA_CA_FILE` (docs/self-host.md,
  seam 12).
- **No mail** (Paul, 2026-10-22: mail is not built). celld has no mailer,
  and the stack runs none: each send logs `mail.unsent` and carries on.
- **The CLI trusts the OS's roots** beside the public ones, as every client
  on a company's network must. The e2e and the cell still trust the public
  roots alone.
