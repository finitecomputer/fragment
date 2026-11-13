# Private agents MVP: one attested pod per owner

Proposal, 2026-11-09, updated 2026-11-13 with what the proof of concept
built and found. It follows the research note "Rented Enclaves Can Make
Fragment Credibly Private" and its review. Nothing here is decided until
Paul says so; the decisions it needs are listed near the end.

What each run measured and showed is in
[`private-pod-findings.md`](private-pod-findings.md). Which clouds rent
such a VM is in
[`confidential-vm-provider-landscape.md`](confidential-vm-provider-landscape.md).

## The goal

An owner runs their agent on Finite, and no operator (Finite, Google or
Cloudflare) can read its chats, memory, files or prompts through
software. The owner can check that themselves, with a tool that does not
ask them to trust us.

"Honestly show" means three things ship together: the system, the
verifier the owner runs, and a published list of what is still trusted.
The MVP is done when an outsider can try to break the claim and fail.

## What the MVP claims

| We claim | We do not claim | Why not |
|---|---|---|
| Operators cannot read a pod's chats, memory, files, secrets or prompts through software | "Nobody can ever read it" | Physical tampering in Google's datacenter, or a new CPU bug, still breaks it |
| A leaked disk, snapshot, log or network capture reveals nothing | "Trustless" | You trust Intel, Google's custody, Tinfoil and the code we publish |
| No new release can touch your data until your own CLI unlocks it | That the code is free of bugs | Attestation proves which code runs, not that it is correct |
| Reaching one person's data takes a hardware attack, not a policy decision or a subpoena to a database | That the agent cannot leak | An agent that browses the web can be talked into sending data out |

## The design

```
owner's CLI ──(1) nonce ─────────────────────────► pod-init (locked)
            ◄─(2) TDX quote, naming one fresh boot key, the pod's TLS root and its instance
            ──(3) disk key, sealed to that key, signed by the owner ──►
                                                    opens /data, starts the stack

browser, CLI ── TLS that ends inside the pod ─► front door ─► celld (the cell)
                                                               ├ Durable Objects, R2, queues, workflows: local, on /data
                                                               ├ OpenBao: the platform's secrets, on /data
                                                               ├ identity door: sign-in
                                                               ├ git service: local
                                                               ├ computers: the open problem (below)
                                                               └ model sidecar ─ attested TLS ─► Finite Private on Tinfoil
```

### The pod

One Intel TDX VM on Google Cloud per owner, always on. Inside it runs
the whole of today's stack on one machine, as a self-hosted deployment
(below).

- **Boot disk:** a read-only image with a dm-verity root, booted as one
  unified kernel image (UKI) with systemd-stub 257 or later. Everything
  that sees plaintext is in it: the kernel, Docker, celld, the cell's
  release build, OpenBao, the front door, the computers' images,
  `pod-init` and the model sidecar. Nothing is fetched or compiled at
  boot. It has no SSH daemon, no account that can log in, no Google guest
  agent and no login on the serial console.
- **A debug sibling** of the same definition adds SSH with one key, for
  development and rescue. It measures differently, so a policy that
  accepts the locked image refuses it, and the other way round
  ([2026-11-12-measured-pod](private-pod-findings.md#2026-11-12-measured-pod)).
- **Data disk:** one persistent disk, LUKS2-encrypted, mounted at
  `/data`. It holds the runtime's state, OpenBao's, the git service's,
  Docker's volumes and the TLS keys. An overlay there holds what the
  platform writes over the image's read-only tree.
- **Nothing leaves the pod** except TLS to its clients, TLS to Tinfoil,
  certificate requests, and the agent's own web traffic. Logs stay on
  `/data`. The operator sees one unauthenticated status answer: locked
  or unlocked, the release, and uptime.

The image's definition is `pod-image/`; `cargo xtask pod image` builds
it, `cargo xtask pod measure` predicts its registers, and
`cargo xtask pod vm` boots it.

### Runtime: celld, as a self-hosted deployment (proposed)

**Proposed, open for Paul:** a pod is a self-hosted deployment, with the
cell on celld, plus three things of its own: the measured image, the
owner's unlock and attestation. The 2026-11-09 plan said "workerd now,
celld later". Since then self-hosting has started landing on master seam
by seam ([`docs/self-host.md`](../self-host.md#the-seams)), and the proof
of concept's demo ran on celld. A pod would take these seams as they
are:

- **1, the runtime:** celld runs master's cell, with no code of celld's
  in the cell.
- **3, models from a file:** every slot named in one file, which a pod
  fills with private slots alone.
- **12, secrets:** the platform's root secrets in OpenBao, on `/data`.
- **4, sign-in:** through the identity door of PR #1258, by
  configuration.
- **9, LAN mode:** the front door, bound to the pod's own address.

The trust machinery (the image, attestation, unlock) does not depend on
the runtime. What it buys is one lane, not two: a pod is what self-host
already builds, inside a TDX VM. The alternative is the original plan,
the cell on workerd through `wrangler dev`, which master's own e2e runs
with Docker computers. That route keeps computers working today, and
brings back the debt entry the earlier plan named: `wrangler dev` is a
development server, so the pod would expose only its front, keep the
runtime on loopback and refuse `/cdn-cgi/*`.

### Computers are the open problem

**A pod built from master on celld has no computers today.**

- Self-host's computers (PR #1264, seam 2) run on celld's krun engine,
  sandcastle's microVMs. They refuse celld's Docker engine, because it
  has no egress intercepts or snapshots, which a computer needs.
- krun needs KVM inside the VM, so nested virtualization. The proof of
  concept found no rentable confidential VM that offers it
  ([the survey's "Limits"](confidential-vm-provider-landscape.md#limits)).
- The demo works around it with sandcastle's Docker double, patched, on
  an old pinned fork of fragment (`2e123d40` on `selfhost-http-port`).
  That is a stand-in, not a path to ship.

The ways forward are a question for Paul (decision 2):

1. celld's Docker engine gains egress intercepts and snapshots;
2. a Docker engine is allowed for this case, with what that costs in
   isolation between the agent and the platform;
3. the original route: the cell on workerd through `wrangler dev`, with
   master's Docker computers.

### Keys: the owner holds the only one

There is no key service in the MVP.

- At `fragment pod init`, the CLI makes a random disk key, keeps it in
  its config and prints it once as a recovery code. It signs with the
  CLI's existing key.
- The pod holds the key only in memory. After any restart it comes up
  locked, with a new boot key, and waits.
- `fragment pod unlock` verifies the pod, then sends the key sealed to a
  key that exists only inside that boot of the enclave, in a request
  signed by the owner's key. A replay of that request on the same boot is
  refused.
- The pod keeps a hint of its owner's npub in the volume's LUKS2 header.
  The hint is untrusted, as instance metadata is: a second claimant with
  the right disk key, after the host rewrote the hint, was refused.
- Secrets the pod makes for itself (the host secret, agents' keys) are
  generated inside it and live on `/data`.

The cost is plain: after a restart the agent is down until its owner
unlocks it. Google restarts TDX VMs for host maintenance, since they do
not live-migrate. The MVP accepts that. Unattended restart needs the key
service the research describes, and it is deferred.

`pod-init`'s contract is `docs/pod.md`.

### What the CLI checks before it releases the key

1. The TDX quote verifies against Intel's certificates, with a fresh
   nonce in its report data. The report data also covers the pod's boot
   key, its state, its owner and its TLS root.
2. The measurement is one this CLI release accepts, and is not older
   than the last one it unlocked. RTMR1 and RTMR2 must equal the values
   computed from the image file, RTMR3 must be zero, and RTMR0 must be
   one observed for that machine type and its disks.
3. Debug is off and the platform's TCB status is up to date.
4. The quote names the Google Cloud instance: MR_OWNER is SHA-384 of the
   instance, project and zone, and the host is in Google's host registry
   for that zone. Without this, anyone with a TDX machine and a $1,000
   interposer, us included, can forge step 1. MR_OWNER is set by the
   host, so it means something only with the registry, and the registry's
   document is fetched over TLS and is not signed
   ([2026-11-10-quote-live](private-pod-findings.md#2026-11-10-quote-live)).
5. The pod names this CLI's key as its owner, or is blank.

Any failure refuses the unlock and says which check failed.
`fragment pod verify` runs the same checks against a running pod and
also confirms the TLS key the CLI is talking to is the attested one.
`fragment pod status` shows the pod's state and its checks.

### Identity

The pod has its own registry, on its own encrypted disk, so nobody
outside can mint a session.

- Sign-in is self-host's seam 4: the identity door (PR #1258) answers the
  WorkOS endpoints the cell calls, as an OpenID Connect relying party of
  the provider its config names. Which provider a pod names is open: one
  inside the pod (the demo ran Dex there), or the owner's key.
- The owner's key is the pod's one person. Their agents are theirs, as
  now.
- `fragment pod open`, a one-time link that signs a browser in from the
  CLI's key, is still a proposal and not built. The cell already does
  this for one fragment's pages (`POST /api/f/{name}/session`).

### Models

Every model slot in a pod is private, or the pod refuses to start. The
cell already refuses a private route with a non-private slot. A pod's
slots come from its models file (self-host's seam 3).

- The cell calls a sidecar on loopback. The sidecar verifies Tinfoil's
  attestation, pins the connection to the attested key, and forwards.
  Not built in the proof of concept.
- The sidecar accepts only the published measurement of the Finite
  Private deployment, which includes our own limiter.
- Voice notes, decisions, speech and embeddings either run on a private
  slot or answer "not available in a private pod".

### Network

- The pod's hostnames are DNS-only records. Cloudflare never proxies or
  terminates them.
- The front door gets certificates inside the pod, for the pod's own
  names only. In the proof of concept its certificate chained to the
  attested TLS root, and a browser reached the agent over a tailnet
  ([2026-11-11-lock-unlock](private-pod-findings.md#2026-11-11-lock-unlock)).
- The CLI pins the attested key. A browser cannot, so a browser trusts
  DNS and the certificate authorities. Mis-issuance would show in
  Certificate Transparency but would not be prevented.

### Releases and upgrades

- CI builds the image and the CLI, publishes both with GitHub build
  provenance, and writes each accepted measurement into the public repo.
- `cargo xtask pod measure` computes RTMR1 and RTMR2 from the image's
  bytes, so anyone can check the published values. RTMR0 is observed,
  not computed: it depends on the machine type and on the disks attached
  at boot.
- An upgrade is: the operator boots the new image, the pod comes up
  locked, and the owner's updated CLI unlocks it. An owner who does not
  update is never moved.

## How an owner uses it

1. We create the VM and send them its hostname.
2. `fragment pod init <host>`: verifies, makes the disk key, prints the
   recovery code, claims the pod.
3. They sign in through the browser, make an agent and chat with it.
4. `fragment pod verify`, whenever they like, prints the evidence.
5. After a restart or upgrade: `fragment pod unlock`.

## Scope

| In the MVP | Out, and why |
|---|---|
| One owner and their agents, per pod | Sharing with other people: needs member identity across pods |
| Chat, agent memory, files, the agent's screen and browser | Connections (Google and others): WorkOS Pipes holds the tokens |
| Hermes on Finite Private | Claude Code, an owner's own model, operator-key tools: the vendor sees plaintext |
| CLI as verifier; browser for daily use | iOS verifier, browser extension, WAICT |
| Owner unlocks after each restart | Key service, timelock, cross-cloud key shares |
| Operator-run disk snapshots as backup | Rollback protection, authenticated disk encryption |
| Pods made by hand with an xtask command | Self-serve creation, billing integration, metering export |
| One region, one cloud, Intel TDX | Azure, AMD SEV-SNP (on Google's N2D its report pins neither our image nor the instance), multi-node |
| Measured, published, provenance-signed image | Bit-for-bit reproducible builds |

## Milestones

Each is several small PRs, proven as AGENTS.md asks. On the proposed
runtime the cell does not change: a pod is configuration of self-host's
seams. Sizes are guesses.

**M0. The PoC: two halves, proven apart. Done, 2026-11-10 to
2026-11-11.** The go-or-no-go question was whether check 4 can be met
and the measurement recomputed from the image. Both were met, check 4
in the form described above.

- **Half A: it runs, and what TDX costs.** A warm agent turn with the
  real model cost nothing measurable on TDX: 1023.2 ms plain against
  1028.0 ms TDX on C3, p 0.97
  ([2026-11-10-tdx-vs-plain-real](private-pod-findings.md#2026-11-10-tdx-vs-plain-real)).
  On C4, three VMs a side, the VM's own share of a turn was 21.5 ms
  (1.08x) slower on TDX and a cold build 1.21x, but each arm was in its
  own zone, so part of that may be the zone's hosts
  ([2026-11-11-c4-tdx-set](private-pod-findings.md#2026-11-11-c4-tdx-set)).
  LUKS2 on the data disk cost a warm turn about 20 ms (2%) and a cold
  wake 55 ms (1.03x), with dm-crypt's workqueues bypassed
  ([2026-11-11-disk-encryption](private-pod-findings.md#2026-11-11-disk-encryption)).
- **Half B: a client can tell our image from another.** A laptop
  verified a live quote down to the instance and the host in Google's
  registry, on C3 and C4
  ([2026-11-10-quote-live](private-pod-findings.md#2026-11-10-quote-live)).
  RTMR1 and RTMR2 are computed from the image file and match the quotes;
  a one-file change moves RTMR1; a flipped bit on disk is refused by
  dm-verity before any quote; a bigger machine moves only RTMR0
  ([2026-11-10-measured-image](private-pod-findings.md#2026-11-10-measured-image)).
- **systemd-stub 257 or later is required.** With Ubuntu's stub 255 the
  host could swap the kernel command line and no register changed; stub
  257 measures it into RTMR2
  ([2026-11-10-cmdline-swap](private-pod-findings.md#2026-11-10-cmdline-swap)).
- **AMD SEV-SNP on Google's N2D cannot pin our image** or name the
  instance from the AMD-signed report: its measurement covers only
  Google's firmware and the vCPU count
  ([2026-11-11-n2d-sev-snp](private-pod-findings.md#2026-11-11-n2d-sev-snp)).

**M1. The measured image.** Shown in the proof of concept; being
rebuilt on master as `cargo xtask pod image|measure|vm` and `pod-image/`.
- Shown: the whole platform runs from the measured image with no login
  (image P), and its debug sibling measures differently; each is refused
  under the other's policy. The live quote matches RTMR1 and RTMR2
  computed from the image file
  ([2026-11-12-measured-pod](private-pod-findings.md#2026-11-12-measured-pod)).
  One image on two hosts gave the same registers
  ([2026-11-10-measured-image](private-pod-findings.md#2026-11-10-measured-image)).
- Still open: RTMR0 is observed, not computed. The image was rebuilt to
  the same bytes only on the same machine from the same inputs, never on
  another build box. The computers' images are rebuilt, not pinned by
  digest.

**M2. Lock, unlock and verify.** Shown in the proof of concept; being
rebuilt on master as `crates/pod` (the protocol and release checks),
`crates/pod-init` (`pod-init`, the one binary in the image) and
`fragment pod init|unlock|verify|status` in the CLI; the pod's contract
is `docs/pod.md`.
- Shown: on a stock Ubuntu TDX VM the pod booted locked; six attempted
  bypasses were refused (a changed RTMR1, another instance, the wrong
  disk key, a second claimant, a middleman rewriting the state, one
  swapping the boot key); a replay on the same boot got 409; the unlock
  took 3 s ([2026-11-11-lock-unlock](private-pod-findings.md#2026-11-11-lock-unlock)).
  `pod init` (now `fragment pod init`) ran live on a blank block device,
  and the locked image unlocked with a policy whose RTMR1 and RTMR2 come
  from the image file. With the demo's data, unlock took 9 s; from unlock to the front
  door answering, 37 to 38 s
  ([2026-11-12-measured-pod](private-pod-findings.md#2026-11-12-measured-pod)).
- Still open: a replay on a later boot, not tried live. A blank locked
  pod has no attested way to receive its first settings (the model key,
  the tailnet login): the proof of concept placed them through the debug
  image. The exit stands: each of the five checks has a test that was
  watched to fail, plus a replayed unlock and a second claimant.

**M3. The pod as a self-hosted deployment.** Under the proposed runtime
this replaces "pod mode in the cell".
- celld, models from a file, OpenBao, sign-in through the identity door,
  and the front door in LAN mode, each as self-host lands it.
- Every model slot private or refused; connections, provider keys,
  Stripe and mail off.
- The git service runs locally. Today's candidate is self-host's: the
  code.storage fake, which already persists its state (seam 5). That is
  a debt-ledger entry.
- Computers: blocked on decision 2.
- Exit: the e2e's pod section passes on a laptop and in the image.

**M4. Verified inference (about a week).** Not started.
- The model sidecar, and a read of the limiter confirming it keeps and
  sends no prompt content.
- Exit: a wrong measurement on Tinfoil's side refuses the call, watched
  to fail.

**M5. The proof (about a week, then a week of use).** Below.

## The proof

1. **Canary sweep.** Put unique strings in a chat message, the agent's
   memory and a file. Then search everything an operator can reach: a
   disk snapshot, serial output, Cloud Logging, a packet mirror on the
   VM's network, instance metadata, and Tinfoil's account dashboard. No
   canary may appear. Not run. The locked image's serial console showed
   boot lines only: no login prompt and nothing from the platform.
2. **Refusals.** The M2 and M4 negative tests, run against the real pod:
   a modified image, a debug VM, an older release, a wrong owner, and a
   quote that does not name the instance. Partly run: the six refusals
   on a stock Ubuntu VM, and the locked image and its debug sibling each
   refused under the other's policy. Not yet against the locked image
   with every case, and not an older release.
3. **Outbound audit.** On a test image, record every host the pod
   contacts during a canary turn. Expect Tinfoil, the certificate
   authority and the sites the agent browsed. Anything else is a bug.
   Not run.
4. **Latency.** First-token time and page load against the standard
   tier, and time from restart to unlocked. Measured against plain VMs,
   not yet against the standard tier: the figures are under M0 and M2.
5. **A week of real use** by one of us, including one upgrade and one
   forced restart. Not run.
6. **One outside review** of the image, the verifier and this document,
   one round. Fix what breaks the claim; file the rest. Not run.
7. **The trust page**, published with the code: the claims table above
   and the list below. Not written.

## What the owner still trusts

- Intel's TDX, and Google's physical custody and firmware.
- Google's host for RTMR0, which is accepted as observed per machine
  type and disks, not computed.
- The measured image, which is large: Linux, Docker, celld, OpenBao, the
  identity door, the cell, Hermes and Chromium.
- The CLI on their own machine, and GitHub's build provenance for it and
  the image.
- Tinfoil's enclaves and release process for prompts.
- DNS and certificate authorities, for browser sessions only.
- Their own agent, which can be prompt-injected into leaking.

What operators still see: that a pod exists, when it is up or locked,
its size and traffic volumes, and the addresses the agent connects to.
The operator can stop a pod, corrupt its disk or roll it back to an
older snapshot; nothing detects the rollback. It cannot read it.

## Deferred, and what brings each back

| Deferred | Comes back when |
|---|---|
| Key service with a published delay (unattended restart) | Owners unlock more than about once a month, or the first owner who is not a developer |
| iOS as a verifier | A pod owner uses the phone app daily |
| Sharing and multiplayer in a pod | A paying owner asks to share a private fragment |
| Rollback detection, authenticated disk | Before the claim covers integrity, not only privacy. LUKS2 with integrity halved sequential writes and cost 20% of commits in the proof of concept |
| Reproducible builds | The outside review, or a customer, asks who checks the build |
| Connections inside the pod | OAuth tokens can be held in the pod instead of WorkOS |
| A pod across more than one machine | A pod outgrows one VM |

## Decisions needed from Paul

1. **Runtime.** Proposed: a pod is a self-hosted deployment, the cell on
   celld with self-host's seams 1, 3, 12, 4 and 9, plus the measured
   image, the owner's unlock and attestation. The alternative is the
   original plan, the cell on workerd through `wrangler dev`.
2. **Computers.** On celld a pod has none today: self-host's computers
   need krun, so KVM, and no rentable confidential VM offers nested
   virtualization. Which way forward: celld's Docker engine gaining
   egress intercepts and snapshots, a Docker engine allowed for this
   case, or the `wrangler dev` route of decision 1's alternative?
3. **A decision entry** allowing Google Cloud as an outside service for
   confidential compute, which Cloudflare cannot do (decision 1).
4. **Owner unlock after every restart** as the MVP's cost.
5. **Public source.** The claim can only be checked if the pod's inputs
   are public. Is the repo, or at least the image's inputs, public?
6. **Spend.** One TDX VM for M1 to M5 and a Finite Private key.
7. **The first owner** and the region.

## Not yet verified

- That Google keeps RTMR0 stable for a machine type and its disks across
  hosts and over time. The proof of concept saw it change with the disks
  attached at boot.
- Whether check 4, an instance named by a host-set register and an
  unsigned registry document, is strong enough for the claim.
- That Tinfoil's Go verifier covers our own container deployment as it
  does their shared models.
- How often Google restarts a TDX VM. The week of use measures it.
- The VM's monthly cost. My guess is $100 to $150 for 4 vCPU and 16 GiB;
  the survey found no base price for C3 or C4.
- Card rendering end to end in the pod, and a restart of the locked pod
  with the demo's data.
- A computer awake when the VM goes down loses what it did since its
  last save.
- celld's own defects on self-host (`docs/self-host.md`), which a pod on
  celld would carry.

Settled by the proof of concept: Google's firmware measures our kernel
and command line so an outsider can recompute RTMR1 and RTMR2 (with
stub 257 or later); Google's instance evidence is MR_OWNER and the host
registry, bound to the quote; and Phala's pure-Rust DCAP verifier,
`dcap-qvl`, verified the quotes on the laptop.
