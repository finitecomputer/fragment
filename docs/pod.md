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

Not yet on master: the owner's command line (`fragment pod …` in `cli/`),
the image, measurement and VM tooling (`cargo xtask pod …`), and a key
service. They are thin layers over the library.

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

The owner's state directory holds `owner.key` and `disk.key` (0600,
never overwritten), `state.json` and the owner's `policy.json`
(`owner::dir`, `owner::policy`):

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
attested owner is this key, or the pod is blank, for init only). After the
pod answers, a second fresh attestation must show the same boot key,
`open`, and this owner; then the release's seq is written to `state.json`
(`owner::flow`). `verify` adds `tls_root` (the served PEM hashes to the
attested root) and, with a TLS target, `tls_leaf` (a handshake trusting
only that root succeeds for the name).

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
