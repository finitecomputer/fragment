# A pod

A pod is one owner's self-hosted stack (`docs/self-host.md`) inside a
confidential VM (Intel TDX on a Google Cloud confidential VM), plus three
things: a measured image, the owner's unlock, and attestation. The pod
keeps its data on a LUKS volume whose key only the owner holds. After any
boot it is locked; the owner's machine verifies the pod by attestation,
then sends the disk key sealed to a key that exists only inside that boot,
in a request signed by the owner's key.

On master:

- `crates/pod` (`fragment-pod`): the library both sides link. Without
  features, `unlock` (the protocol below) and `attest` (the quote layout,
  nonces, the GCE instance binding, the CCEL event log and its replay, and
  the RTMR1 and RTMR2 an image's boot must produce, computed from the
  image's partition table and UKI). Feature `verify`: `verify` (the
  verifier) and `checks` (the checks before a key leaves its holder, behind
  an `Attestor`, so the owner's CLI and a key service decide them with the
  same code). Feature `owner`: `owner` (pod-init's client, the owner's state
  directory, policy.json, the TLS checks and the key request in its order).

- `crates/pod-init` (`fragment-pod-init`, binary `pod-init`): the service
  in the TD (below).

- `crates/pod-keyd` (`fragment-pod-keyd`, binary `pod-keyd`): a key
  service, in a TD of its own, that gives a pod its disk key after a
  restart on the owner's terms (below, "A key service"). The protocol is
  `fragment-pod`'s `keyd`, without a feature, since pod-init opens what it
  delivers.

- `fragment pod status|verify|init|unlock <pod>` and `fragment pod keyd
  status|deposit|approve` (`cli/src/pod`, the CLI's default `pod` feature,
  which the computer images' CLI is built without): the owner's command
  line over `owner`, with the CLI's own key as the owner (cli/GUIDE.md,
  "Your pod").
- `pod-image/`: the measured image's definition (below, "The image"),
  and `cargo xtask pod` (`xtask/src/pod/`): its build, measurement and
  VMs.


## Lock and unlock

States: `blank` (no volume, or no LUKS header), `locked` (a volume, not
open), `open` (mapped and mounted). The pod reads the state from the
system for every request and remembers nothing but its boot key, an
X25519 pair made in memory at start and never stored.

Each quote's REPORTDATA is SHA-512 of `"pod-binding-v1\0"` ‖ nonce (32)
‖ boot_pubkey (32) ‖ state (1: 0, 1, 2) ‖ owner (32, x-only secp256k1,
zeros if none) ‖ tls_root (32, SHA-256 of the root certificate's DER,
zeros if none) (`unlock::binding`). The client recomputes it from
`/status` and its own nonce; until it equals the verified quote's
REPORTDATA, nothing in `/status` is trusted.

A key request seals the 64-byte disk key with HPKE base mode
(DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305), info
`pod-unlock-v1`, aad `kind ‖ owner ‖ boot_pubkey` (kind is `init` or
`unlock`) (`unlock::seal`), and signs SHA-256(`"pod-request-v1\0"` ‖ kind
‖ boot_pubkey ‖ enc ‖ ct) with BIP-340 (k256 0.13.4, as crates/nip98)
(`unlock::sign`). The pod checks, in order: shape and size (4 KiB), the
signature under `owner`, the seal under this boot's secret, the state
(init needs blank, unlock locked), the volume operation, for unlock the
owner recorded inside the volume (another owner holding the right key is
refused and the volume closed again), then the open hook. A request made
for an earlier boot fails at the signature.

The JSON (`unlock::wire`): binary fields are lowercase hex of a fixed
length, every length is checked before decoding, and unknown fields are
refused. `/status` is `{"version":1,"state","boot_pubkey","owner"|null,
"tls_root"|null,"quote"}`; a key request is `{"owner","enc","ct","sig"}`
and answers `{"state":"open"}`; a refusal is `{"error":"<code>",
"detail":"…"}`.

While locked, `/status`'s owner is the hint beside the volume, which the
host can rewrite; once open it is the file inside the volume. A forged
hint can make the owner's side send a key, but only to a TD running an
attested release, which opens nothing with it unless the volume's own
record names that owner.

A release (`unlock::release`) is a sequence number, a name, and the
registers it pins (MRTD, RTMR0 to RTMR3); registers it does not name are
not compared, and it must name at least one. One shape everywhere: a
release in policy.json, a release file, an approval to a key service.

## The owner's side

A pod's directory on the owner's machine holds `disk.key` (0600, never
overwritten), `state.json` and the owner's `policy.json` (`owner::dir`,
`owner::policy`). The owner's signing key is the caller's: the fragment
CLI's own (its config's `secret_key`), and the directory is
`<config>/pods/<host>/` beside that config (cli/GUIDE.md, "Your pod"):

```json
{"releases": [{"seq": 1, "name": "pod-a", "mrtd": "<96 hex>", "rtmr0": "…", "rtmr1": "…", "rtmr2": "…"}],
 "allow_tcb_status": [],
 "gce_instance": "projects/<number>/zones/<zone>/instances/<id>"}
```

Before a key leaves, five checks must pass on a fresh quote
(`owner::checks`, all but `owner` are `checks`'): `quote` (the chain to
Intel's root with collateral fetched now, and the binding), `release` (the
registers match a policy.json release whose `seq` is not below
`state.json`'s `last_unlocked_seq`; omitted registers are named as not
compared), `platform` (debug off, TCB UpToDate or allowed),
`google_instance` (MR_OWNER and Google's host registry), `owner` (the
attested owner is this key, or the pod is blank, for init only). Once they
pass, and before the key is made or sent, the release's seq is saved as
`state.json`'s `last_unlocked_seq`, the floor: the maximum of what is
saved and this seq, under a lock on `state.json.lock`, through a synced
temporary file and a rename; if that fails, nothing is sent. So the floor
is the newest release this owner checked and sent a key to, whether or
not the pod's answer came back, and a pod rebooted into an older release
gets nothing. An owner who must go back to an older release edits
`state.json` on purpose. After the pod answers, a second fresh
attestation must show the same boot key, `open`, and this owner
(`owner::flow`). `verify` adds `tls_root` (the served PEM hashes to the
attested root) and, with a TLS target, `tls_leaf` (a handshake trusting
only that root succeeds for the name).

## pod-init

`crates/pod-init` (`fragment-pod-init`, binary `pod-init`) is the service
in the TD. It links fragment-pod without features (`unlock` and `attest`
alone) and no HTTP or async crate, so it builds static for
x86_64-unknown-linux-musl with no C toolchain. It answers one request at a
time, so two key requests never race on the volume.

| Route | Answer |
|---|---|
| `GET /status?nonce=<64 hex>` | the state, boot key, owner and TLS root, and a quote over their binding |
| `GET /eventlog` | the CCEL (`/sys/firmware/acpi/tables/data/CCEL`) |
| `GET /tls-root` | the root's PEM; 404 if none yet |
| `GET /healthz` | `ok` |
| `POST /init`, `POST /unlock` | a key request; `200 {"state":"open"}` |

Refusal codes: `bad_request`, `bad_field`, `too_large` (413), `bad_nonce`,
`bad_signature` (401), `bad_seal`, `wrong_key`, `owner_mismatch` (403),
`wrong_state` (409), `volume_failed`, `open_hook_failed` (the volume stays
open), `quote_failed` (500), `not_found`, `method_not_allowed`.

```sh
sudo pod-init --config /etc/pod-init.json [--bind 0.0.0.0] [--port 7000] [--configfs <dir>] [--ccel <file>]
```

```json
{"volume_image": "/var/lib/pod/volume.img", "volume_size_gb": 20,
 "mapper_name": "pod", "mount": "/srv/pod", "owner_file": "pod/owner",
 "owner_hint": "/var/lib/pod/owner.hint", "tls_root": "/srv/pod/tls/root.pem",
 "open_hook": "/usr/local/bin/pod-opened"}
```

With `"key_service": "<host:port>"`, pod-init asks that key service for
the disk key while it is locked (below, "A key service"); the owner's
`/init` and `/unlock` work throughout.

Without `owner_hint`, the hint is kept in the volume's LUKS2 header
(token 7), for an image whose root is read-only and whose /var is memory.
It needs root, `cryptsetup`, `mkfs.ext4`, `mount` and `umount` on PATH.
The key goes to cryptsetup on stdin only. Init makes a sparse image file
of `volume_size_gb` if `volume_image` does not exist (never under /dev),
then `luksFormat --type luks2 --cipher aes-xts-plain64 --key-size 512
--sector-size 4096 --pbkdf pbkdf2 --pbkdf-force-iterations 1000` (the key
is 64 random bytes; a slow KDF adds nothing), opens it with
`--perf-no_read_workqueue --perf-no_write_workqueue --allow-discards`,
makes ext4 and mounts with `-o discard`. `open_hook` runs with no
arguments once the volume is mounted, for at most 300 s.

Quotes come through configfs-tsm (`tsm`: a report directory, REPORTDATA
to `inblob`, `outblob` read while `generation` holds still), behind a
`Quoter` trait so the service is tested without a TDX guest.

`cargo test -p fragment-pod-init` covers the service with a fake volume
and quoter through the real HTTP reader, the owner's client and key
request against it, and the configfs-tsm sequence against a fake kernel.
The real volume (`tests/luks.rs`) needs root and loop devices and is
ignored; on a Mac, build it for Linux and run it in a privileged container,
with the VM's own `/dev`, where the loop and device-mapper nodes made during
the run appear (a container's `/dev` holds only those there when it started):

```sh
docker run --rm --privileged -v /dev:/dev -v "$PWD:/src:ro" -w /src rust:1.99-alpine sh -c \
  'apk add -q musl-dev cryptsetup e2fsprogs util-linux \
   && CARGO_TARGET_DIR=/tmp/t cargo test --locked -p fragment-pod-init --test luks -- --ignored'
```

The measured image's binary: `cargo build --release -p fragment-pod-init
--bin pod-init --target x86_64-unknown-linux-musl`, with
`RUSTFLAGS="-C linker=rust-lld"` when the host is not x86_64 Linux.

## A key service

After a restart a pod is locked until its owner unlocks it. A key service
(`crates/pod-keyd`, binary `pod-keyd`) does it for the owner, on the
owner's terms: it holds the pod's disk key in memory, and gives it to a
boot of that pod only when the boot's quote passes the checks the owner's
CLI runs (`checks`, the same code), against releases the owner approved.
It acts only on statements signed by the owner's key; nothing Finite or
the operator says changes what it gives out.

It runs in a TD of its own, attests itself as a pod does, and holds
nothing at start but the names of the owners it serves. Its operator
gives it those (`--owner`, each a public key), and it takes a deposit
from no one else: a signature shows only that a depositor holds a key,
and anyone can make keys. The list decides who may use the service and
nothing about what a key is released to. It is not attested, and adding
an owner is a restart (debt: "A pod's key service serves only the owners
named when it starts"). A restarted key service holds no record, and its
pods wait for their owners to deposit again or unlock them by hand (debt:
"A pod's key service holds its keys in memory only").

| Route | Answer |
|---|---|
| `GET /status?nonce=<64 hex>` | `{"version":1,"boot_pubkey","quote"}`: a quote over SHA-512(`"pod-keyd-binding-v1\0"` ‖ nonce ‖ boot key) |
| `GET /eventlog`, `GET /healthz` | as pod-init's |
| `POST /deposit` | a deposit from an owner the operator named; the record's public view |
| `POST /approve` | an owner's approval of one more release; the record's view |
| `GET /nonce` | `{"nonce"}`: for one `/release`, within a minute. Nothing is kept for it |
| `POST /release` | a pod's status bound to that nonce; a delivery |
| `GET /pods/<owner hex>` | the record's public view: its id, instance, TCB statuses, `min_seq`, approvals and last release; never a key |

The statements (`keyd`). Each is signed with BIP-340 by the owner over a
fixed binary encoding (`keyd::encode`), never over JSON, and names the
key service's boot key from a quote the owner checked, so none applies to
another boot of it:

- **Deposit** (`keyd::deposit`): the disk key sealed with HPKE base mode
  (info `pod-keyd-deposit-v1`, aad owner ‖ key service boot key) to that
  boot key; the pod's Google instance, the TCB statuses it may have, the
  approvals and `min_seq`; the id of the record it replaces (none if none:
  a deposit is taken only over the record it names, so an older one
  replayed cannot put back what a newer one changed); the **grant**,
  SHA-256(`"pod-keyd-grant-v1\0"` ‖ owner ‖ boot key) signed; and the
  signature over SHA-256(`"pod-keyd-deposit-v1\0"` ‖ boot key ‖ owner ‖
  all of that), which is the record's id. The record a deposit makes
  takes the `min_seq` of the one it replaces if that is higher: within a
  boot an owner's `min_seq` never goes down. A record keeps its id while
  each release it serves raises its `min_seq`, so a replacement signed
  before a release and arriving after it would otherwise bring the older
  one back.
- **Approval** (`keyd::approve`): one release (`unlock::release`'s shape)
  with an optional `not_before` and `expires` (unix seconds), signed over
  SHA-256(`"pod-keyd-approve-v1\0"` ‖ boot key ‖ owner ‖ record id ‖ the
  approval). Refused below the record's `min_seq`, with a seq the record
  already has (so a replay adds nothing), past 32 approvals, for a record
  that is gone, or signed by anyone but the record's owner.
- **Delivery** (`keyd::delivery`): the disk key sealed in HPKE **auth**
  mode from the key service's boot key (info `pod-keyd-release-v1`, aad
  owner ‖ pod boot key ‖ key service boot key) to the pod's boot key, with
  the grant. A pod uses it only if the owner it names signed the grant
  for the boot key that sealed it, which exists only inside a key service
  boot the owner attested.

`/release`, in this order: the nonce (one of this boot's, made in the
last minute, and not one a release was already made for), the record
of the owner the pod's status names, then the checks, refusing at the
first that fails: `quote` (the chain, and REPORTDATA is the pod's binding
of its status and our nonce), `release` (an approval with the quote's
registers, open now: `not_before <= now < expires`, and not below
`min_seq`), `platform` (debug off, TCB UpToDate or one the deposit
allowed), `google_instance` (the record's), and the pod's state is
`locked`. Then the nonce is spent and `min_seq` is raised to the release
matched, before the key is sealed, as the owner's CLI saves its floor
before it sends a key: an older release is refused from then on, even if
the delivery is lost.

A nonce is the time it was made, eight random bytes and 16 bytes of
HMAC-SHA256 over both (domain `pod-keyd-nonce-v1`) under a key made when
the service starts. That time is the latest the service has read from
its clock, so it never goes back: a nonce whose minute is over stays over
when the host moves the clock back. `/nonce` keeps nothing, so asking for
nonces takes room from no one. A refused `/release` keeps nothing either.
What is kept is the nonces releases were made for, by owner, for their
minute: 16 an owner, added to only by a boot of that owner's pod that
passed every check. Past that the owner's pod is told `too_many_releases`
and asks again; no other owner is affected.

Refusal codes: `bad_request`, `bad_field`, `bad_nonce`, `no_owner` (400);
`bad_signature`, `bad_grant` (401); `bad_seal`, `unknown_nonce`,
`not_owner`, `not_admitted`, and the checks `quote`, `release`, `not_yet_valid`,
`expired`, `rollback`, `platform`, `google_instance` (403);
`unknown_owner`, `unknown_record`, `not_found` (404);
`method_not_allowed` (405); `stale_record`, `duplicate_seq`,
`below_min_seq`, `too_many_approvals`, `wrong_state` (409); `too_large`
(413); `too_many_releases`, `too_many_records` (503); `quote_failed`,
`internal` (500). Limits: 1,024 owners, a record each, 32 approvals a
record, 16 releases an owner in a minute, bodies of 64 KiB (a deposit),
4 KiB (an approval) and 132 KiB (a release, its quote in hex).

```sh
pod-keyd --owner <64 hex> [--owner <64 hex>]... [--bind 0.0.0.0] [--port 7100] [--configfs <dir>] [--ccel <file>]
```

**pod-init's side** (`ask`): with `key_service` set, while locked and a
hint names an owner, a thread asks (a nonce, its status for it,
`/release`) with backoff from 2 s to 5 min, and opens the volume with the
answer through the same path `/unlock` takes, under one lock, owner check
inside the volume included. Whichever opens first wins; the other gets
`wrong_state`. The address is untrusted (debt: "A pod learns its key
service's address from its config"): a wrong or hostile key service
learns only what `/status` gives anyone and can only fail the unlock.

**The owner's side** (`owner::keyd`; `fragment pod keyd`, cli/GUIDE.md):
the key service is held to `keyd-policy.json` in the pod's folder,
policy.json's shape naming its own releases and instance, and four checks
on a fresh quote (`quote`, `release`, `platform`, `google_instance`) must
pass before anything is signed for it. A deposit carries policy.json's
instance, TCB statuses and releases at or above the floor as approvals,
and the floor (state.json's) as `min_seq`. The floor signed is the
owner's machine's alone. A record's view is plain HTTP that no one
vouches for: the id of the record to replace is taken from it, and a
wrong one only gets the deposit refused; its `min_seq` is not, since
anyone on the path could raise it and a floor signed that high would
hold. The key service keeps its own higher `min_seq` itself (above).

The key service decides windows and nonces by the TD's clock, which the
host sets (debt: "A pod's key service trusts the host's clock").

`pod-keyd` links `fragment-pod` with `verify` (for the checks, and through
them dcap-qvl and the blocking rustls client that fetches Intel's
collateral and Google's registry), so it does not build static for musl
without a C toolchain (ring), unlike pod-init. It takes pod-init's library
for its quoter, HTTP server and event log. It serves one request at a
time: a release fetches collateral and the registry (cached while good),
so a slow one delays the next.

## The image

`pod-image/` defines it (its README says what it is and where each part
lives): one mkosi definition for the locked image and its debug sibling,
with the tools pinned in `pod-image/pins.jsonc` and the platform it carries
pinned in `pod-image/pinned-platform/` (debt: the image carries an old
fork of the platform, not the tree it is built from;
`docs/technical-debt-ledger.md`). `cargo xtask pod` builds, measures and
boots it:

```sh
cargo xtask pod image platform all --config <file>       # once per build host: the pinned platform, staged
cargo xtask pod image build --config <file> [--variant locked|debug|both]
cargo xtask pod image compare target/pod-image/locked <other build> [--same]
cargo xtask pod measure --dir target/pod-image/locked --name pod-p --seq 1 [--quote q.bin [--log ccel.bin]] [--mrtd <hex>] [--rtmr0 <hex>] [--out release.json]
cargo xtask pod image publish --config <file> --variant locked --confirm-spend
cargo xtask pod vm create|start --config <file> --confirm-spend
cargo xtask pod vm stop --config <file> --confirm-stop
cargo xtask pod vm describe --config <file> [--save <dir>]
cargo xtask pod vm delete --config <file> [--delete-data-disk <name>] --confirm-delete
```

Every command that reaches the build host or Compute Engine takes
`--dry-run`, which prints each command it would run, in order, and runs
none. Those that spend money or delete things run only with their own
confirming flag; without it they print the plan and refuse. A delete says
what it deletes and what it keeps first; the data disk, the owner's
volume, goes only when named again with `--delete-data-disk`.

- **build** ships the committed tree (`git archive HEAD`; a run refuses a
  dirty tree) to the build host, builds `pod-init` from it static for
  musl, assembles mkosi's folder from `pod-image/mkosi` (and
  `pod-image/debug/mkosi`), systemd-stub 257, the staged platform and the
  debug image's one key (`build.debug_key`), runs mkosi as root, and
  fetches the small outputs into `target/pod-image/<variant>/`: `pod.efi`,
  `gpt.bin`, `pod.roothash`, `partitions.json`, `sha256sums`, `sizes`,
  `pod-init.sha256` and `build.json` (the commit and the pins). The disk
  stays on the host. mkosi needs Linux and root: the host is this machine
  when it is Linux (`"host": "local"`), or a Compute Engine VM reached with
  `gcloud compute ssh` (`"gce"`).
- **platform** builds the pinned platform on the host and stages it
  (`sources`, `build`, `trial`, `images`, `debs`, `stage`, or `all`); it
  reads the patches from the tree the last build shipped.
- **measure** computes RTMR1 and RTMR2 from the image's own bytes
  (`attest::expect`, `verify::measure`) and writes the release a
  policy.json holds, RTMR3 pinned to zero. MRTD and RTMR0 cannot be
  computed from the image (RTMR0 depends on the machine type and the
  disks attached at boot): they come from a boot's `--quote` or as given,
  and are left out otherwise. With `--quote` it refuses a boot whose RTMR1
  to RTMR3 differ.
- **publish** writes the disk onto a scratch disk attached to the build VM,
  checks it against the hash fetched here, makes an image with
  `UEFI_COMPATIBLE,TDX_CAPABLE,GVNIC`, and deletes the scratch disk
  (`<image>-scratch`). An image that exists ends it. The scratch disk
  carries labels only this tool sets (`pod-tool=xtask-pod-publish`,
  `pod-scratch-for=<image>`): one left by a stopped run for this image is
  reused, and any other disk of that name stops the publish before
  anything is attached. The config refuses a scratch name that is the
  data disk's, the VM's or the build VM's.
- **vm create** makes the data disk if it is not there (kept as it is if
  it is), then the TDX VM, attaching the data disk as device name
  `pod-data`; a VM of that name already there is refused. Port 7000 must
  reach the VM: the network's firewall is the deployment's own.

The config is a file outside the repo (no deployment setting lives in the
repo); its sections are read by the commands that need them, and an
unknown or missing key is refused:

```jsonc
{
  "project": "my-pod-project",
  // on everything made, beside the tool's own pod-role and pod-variant
  "labels": { "owner": "me" },
  "build": {
    // "local" (this machine, Linux) or "gce" with instance and zone
    "host": "gce", "instance": "pod-build", "zone": "us-central1-f",
    // the build user's home on the host (its cargo, worker-build's cache);
    // "dir" (default <home>/podimg) holds the sources, tools and outputs
    "home": "/home/builder",
    // the one SSH key the debug image admits, a file here
    "debug_key": "/home/me/.ssh/pod-debug.pub"
  },
  // Compute Engine image names, 55 characters at most
  "images": { "locked": "pod-img-p", "debug": "pod-img-p-debug" },
  "vm": {
    "name": "pod-1", "zone": "us-central1-f", "machine_type": "c3-standard-8",
    "variant": "locked", "boot_disk_gb": 10,
    "data_disk": { "name": "pod-1-data", "size_gb": 20, "type": "pd-balanced" },
    "network": "pod-net", "subnet": "pod-subnet", "tags": ["pod-7000"],
    // optional: the VM deletes itself after this (its data disk stays)
    "max_run_duration": "3h"
  }
}
```

## What each check proves

| Check | Proves | Does not prove |
|---|---|---|
| chain (dcap-qvl) | A TDX module on a genuine Intel CPU signed this report. The checks cover the quote signature, the attestation key bound through the QE report, the PCK chain to Intel's pinned root, both CRLs, the TCB info and QE identity signatures and dates, and that the TCB is not Revoked. | Where the CPU is. Anyone with any TDX machine passes this check. A machine whose attestation key was extracted (TEE.fail, DDRop) also passes. |
| collateral dates | TCB info and QE identity are within their issueDate and nextUpdate (dcap-qvl checks the CRLs and certificates too). | That collateral saved from an earlier fetch reflects revocations made since then. |
| td_attributes | DEBUG and the other "TD under debug" bits are off, so the host cannot read the TD's memory through the debug interface. | Anything about physical attacks. |
| nonce / binding | The quote was made after we chose the nonce, and (the binding) the TD itself said the boot key, state, owner and TLS root. | That no unannounced flaw lets a host forge a TD. |
| tcb_status | Intel's status is UpToDate for this CPU and TDX module, or one you named. | That no unannounced flaw exists. |
| expect_* / release | The registers equal the values given. | That the values are the right ones. That comes from computing them from the image (`attest::expect`, `verify::measure`). |
| replay | The event log is the boot that produced RTMR0 to RTMR2 (`attest::replay`). Each event whose digest is of its own data, text or value shows exactly what was hashed (`attest::describe::coverage`). | What an event that only names a file, image or ACPI table contained. You must hash that yourself. |
| firmware | Google signed this MRTD as one of its firmware builds (signer certificate chains to the pinned GCE-cc-tcb-root). | That the firmware is benign, or that the binary can be rebuilt from source. |
| gce_instance | MR_OWNER is SHA-384 of the named project number, zone and instance id (the PZID). | That the host is Google's. The host sets MR_OWNER, so this means something only together with host_registry. |
| host_registry | Google's public registry lists this quote's PPID, which comes from Intel's PCK certificate, as a Google host in that region. | A signature: the document is fetched over TLS and is not signed. It also does not prove that a listed host is still in Google's data centre. |

Network: collateral fetches send only the FMSPC and CA type to Intel's PCS
(`api.trustedservices.intel.com`); the firmware endorsement fetch sends the
MRTD, and the host registry fetch the PPID, to storage.googleapis.com.

## SEV-SNP

`verify::snp` verifies an AMD SEV-SNP report (N2D, Milan) with virtee's
`sev` crate, pure Rust: the ARK must be AMD's Milan root pinned in that
crate, ARK → ASK → VCEK/VLEK → report signature, the VCEK's hwID equal to
CHIP_ID, the nonce in REPORT_DATA, debug off. Every other field (policy,
TCBs, CHIP_ID, HOST_DATA, ID and author key digests, REPORT_ID) is
returned for the caller to judge; none names a cloud instance.
`verify::firmware::verify_snp` checks the measurement against Google's
SEV-SNP endorsement. A pod runs on TDX; this was added to compare Google's
N2D (SEV-SNP) machines, and is kept with its tests.

## Tests

`cargo test -p fragment-pod` runs offline from `crates/pod/testdata/`: a
real quote, nonce and CCEL from a c3-standard-8 TDX VM, Intel's collateral
for that quote (fetched once on 2026-11-10; the tests run at a fixed time
after that fetch), Google's firmware endorsement for its MRTD, a real
SEV-SNP report with its certificates and endorsement, test TLS
certificates, and `pod-image-a` and `pod-image-d`: the log, quote and
first 34 sectors of the measured image's boots. Four tests also need that
image's UKI and are ignored unless asked for, with `POD_IMAGE_A_UKI` or
`POD_IMAGE_D_UKI` naming it (checked against testdata's `sha256sums`). The
recorded quotes carry a plain nonce, so no offline test covers the chain
and the binding together.

`cargo test -p fragment-pod-keyd` runs the key service over a fake
attestor (playing Intel and Google for a pod's TD and its own) and a
clock it moves, beside real pod-init services with their fake volume:
each statement valid, invalid, replayed and after a restart, a refusal at
each check, an owner the operator did not name, a replacement deposit
held back past a release, nonces asked for and left unused, and pod-init
asking it over loopback and opening.
`crates/pod-init/tests/ask.rs` covers a key service absent, broken or
hostile; `cli/src/pod/keyd_tests.rs` the owner's commands against both.
