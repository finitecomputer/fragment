# Private pod proof of concept: findings

What the proof of concept measured and showed from 10 to 12 November 2026:
running Fragment's stack inside a confidential VM (Intel TDX, and AMD SEV-SNP
for comparison) on Google Cloud, and checking from a laptop what runs inside.
The plan it works from is [`private-agents-mvp.md`](private-agents-mvp.md);
which clouds offer such VMs is in
[`confidential-vm-provider-landscape.md`](confidential-vm-provider-landscape.md).

**The archives.** Three archives on the private pod PoC's Linear issue,
FIN-494, hold what is not in the repo:

| Archive | SHA-256 | Size | Holds |
|---|---|---|---|
| `private-pod-poc-tooling-2026-11-13.tar.gz` | `77f77952a323a740a52c9f0e9cb1e4a117cf70aa624fe64d2a033d339d5c398c` | 399,170 bytes, 251 files | the proof of concept's own tree, `poc/pod/` at commit `a9c5f341b` |
| `private-pod-poc-results-2026-11-11.tar.gz` | `02968faec5630e8a0bab5cf0fc228af2f3ad4f6c4907cfeb12f1ff72b7a6f29f` | 2,700,696 bytes, 1,805 files | every run's raw output up to lock and unlock |
| `private-pod-poc-results-2026-11-12.tar.gz` | `cac5c1aabcd328dbc33ce3052763476e637514bd53c6c23c7d2add070ad8eadc` | 3,113,004 bytes, 138 files | `results/2026-11-12-measured-pod/` |

The results archives hold results, logs, quotes, event logs and each run's
own scripts. They unpack to `results/<run>/...`; paths below such as
`results/2026-11-11-c4-tdx-set/by-vm-warm.txt` are paths inside them.
Unpacked in the root of the tooling archive's tree, they restore its
git-ignored `results/` folder. The tooling archive's `archive-results.py`
made them (the uncompressed tars' SHA-256 are
`b57d9dbfc1a80724e0441d96dc66f2198da55e1e89a08d2fbde2a3f1efca2c74` and
`88f570f96a4ab64047827184c27c57566e3418b2869090749f14c225fad755fe`).

**The tools, then and now.** The runs used the proof of concept's own tools.
Every path below that names a tool, a script or a README (`attest/`,
`image/`, `measure/`, `vm/`, `demo/`) is in the tooling archive's tree, as
the runs used it. The code that stays now lives in the repo:

| Then (tooling archive) | Now |
|---|---|
| `attest/crates/{unlock,attest,verify}` (`pod-verify`; its `expect` is below), and the shared release checks | `crates/pod`, its test data in `crates/pod/testdata/` |
| `attest/crates/{init,quote}`: `pod-init` and `pod-quote` | one binary, `pod-init`, in `crates/pod-init` |
| `attest/crates/pod`: the owner's `pod status`, `verify`, `init`, `unlock` | `fragment pod status`, `verify`, `init`, `unlock` |
| `image/`: the image's definition | `pod-image/` |
| `image/*.sh` (build, publish, boot) and `pod-verify expect` | `cargo xtask pod image`, `measure` and `vm` |

The rest (`measure/` and its `pod-measure`, the twin-VM scripts in `vm/`,
`report/`, `archive-results.py`, `image/swap-test/`, the per-run scripts and
`demo/`) is only in the tooling archive.

Each run is a section named by its folder; file names in a section are in
that run's folder. Where a run's `notes.txt` gives a figure its data files
contradict, the section gives the data's figure and says the notes differ.
Four runs wrote no `notes.txt` (`2026-11-10-tdx-vs-plain`, `-quote-live`,
`-measured-image` and `-cmdline-swap`); their sections come from their
summary files and the READMEs in the tooling archive.

## Summary

- **A warm agent turn costs nothing measurable on TDX:** 1023.2 ms plain
  against 1028.0 ms TDX with the real model on C3 (p 0.97); 0.99x on C4 and
  0.99x on SEV-SNP with echo turns. ([C3](#2026-11-10-tdx-vs-plain-real),
  [C4](#2026-11-11-c4-tdx-set), [N2D](#2026-11-11-n2d-sev-snp))
- **The VM's own work in a turn, a wake and a cold build are slower on TDX
  C4:** +21.5 ms (1.08x) for the VM's share of a turn, +31 ms warm and +38 ms
  cold for a wake, 1.21x for a build, every TDX VM slower than every plain
  one (the warm wake by less than the spread within an arm). Pure CPU was
  6-9% slower, and each arm was in its own zone, so part of this may be the
  zone's hosts and not TDX. On SNP, two VMs a side, warm wakes were +109 ms
  (1.10x) and builds 1.07x, not separable from the spread between VMs.
- **The C3 TDX VM's slow disk was that VM, not TDX:** 2.3x as long per read
  as its plain twin, while a second TDX VM was faster than ordinary VMs. So
  the C3 pair's slower wakes, restarts and builds cannot be put down to TDX.
  ([zone probe](#2026-11-11-disk-encryption))
- **Encrypted storage (LUKS2) costs a warm turn about 20 ms (2%) and a cold
  wake 55 ms (1.03x).** On a fast disk inside a TD (a TDX guest), LUKS's
  defaults make a commit about 20% slower; with two flags the cost is within
  the raw device's spread. ([2026-11-11-disk-encryption](#2026-11-11-disk-encryption))
- **A laptop can check a TDX quote from Google Cloud,** down to the instance
  in MR_OWNER and the host in Google's registry, on C3 and C4.
  ([2026-11-10-quote-live](#2026-11-10-quote-live))
- **Our own image is told apart by registers computed from the image file
  alone,** and a flipped bit on disk is refused before any quote. With
  systemd-stub 255 the host can swap the kernel command line without any
  register changing; stub 257 records it in RTMR2. ([measured image](#2026-11-10-measured-image),
  [command line](#2026-11-10-cmdline-swap))
- **SEV-SNP on Google's N2D cannot pin our image or name the instance from
  the AMD-signed report:** its measurement covers only Google's firmware and
  the vCPU count. Only Google's host-run vTPM tells image A from image B.
- **The owner's key opens the pod's volume only after attestation:** on a
  stock Ubuntu TDX VM it booted locked, six attempted bypasses were refused,
  and the unlock took 3 s. This shows the mechanism, not privacy from the
  operator. ([2026-11-11-lock-unlock](#2026-11-11-lock-unlock))

- **The whole platform runs from a measured image with no login, and the
  demo's data now lives under it:** the image's registers are computed
  from the image file, its debug sibling (with SSH) measures differently
  and each is refused under the other's policy, and the owner's laptop
  unlocks the pod in 9 s. ([2026-11-12-measured-pod](#2026-11-12-measured-pod))

## How the speed runs were measured

- **From the laptop, as a client.** `pod-measure` (`measure/`) ran on
  the laptop, through an SSH tunnel to one VM at a time, against the repo's
  unchanged dev stack. Runs alternated between the VMs so that drift hit
  both alike. The server's parts of a turn were timed on the VM's own clock
  from its node log, so no clock is compared across machines.
- **A run's median is one observation.** A side's figure is the median of
  its run medians, compared by an exact two-sided Mann-Whitney test on the
  run medians; p below 0.05 is read as real.
- **The VM as the unit** (C4 and N2D). With three VMs a side the smallest
  possible p is 0.10, with two 0.33. What they can show is separation (every
  confidential VM beyond every plain one) with a gap between the arms larger
  than the spread within either arm. Pooled p values over runs are
  optimistic, since runs on one VM are not independent.
- **Real model or echo.** Only `2026-11-10-tdx-vs-plain-real` used a real
  key. The later sets used the word `placeholder` as the key: each turn's
  call was refused by Finite Private (401) and fell back to dev's echo fake.
  Their "model" part includes one refused call over the internet.
- **The tool's own noise.** Two runs a few seconds apart on the laptop
  (`results/compare-local-2026-11-10.txt`) differed by up to 1.28x on HTTP
  medians of 2.4 to 4.9 ms, hence several alternating runs a side.

## 2026-11-10-tdx-vs-plain

*From `compare-runs-laptop.txt`, `compare-runs-loopback.txt`,
`cold-build.txt` and `README.md`.*

**What ran.** Two c3-standard-8 VMs (8 vCPU, 32 GB, Ubuntu 24.04): TDX in
us-central1-b, plain in us-central1-f (zone b had no plain C3). Every model
on the fake, computers on the stub image. Six alternating runs a side from
the laptop, then four a side of the tool on each VM against itself
(loopback).

**Results.** From the laptop everything was within noise except
`http.shell.ttfb` (108.8 against 110.6 ms, p 0.026) and the wake (893.8
against 937.9 ms, 1.05x, p 0.002); a turn took 352.5 ms plain, 352.3 TDX. On
loopback TDX was if anything faster (`live.post`, `live.round_trip` and
`http.shell.ttfb` at 0.95-0.97x, p 0.029). A cold build, one per VM, took
342 s plain and 367 s TDX (1.07x), its steps 1.04x to 1.15x. The kernel
showed the TDX guest (`Memory Encryption Features active: Intel TDX`,
`/dev/tdx_guest`), absent on the plain VM.

**Not shown, or uncertain.** One VM each, in two zones. The zone probe later
found this TDX VM's disk slow for reasons that are not TDX, so its slower
wake and build cannot be put down to TDX. One build sample each.

## 2026-11-10-tdx-vs-plain-real

**What ran.** The same two VMs with the real model (Finite Private's
`glm-5-3-flash`), 2026-11-11 02:22-03:20 UTC, computers on the Hermes image.
Ten alternating runs a side, each with 20 turns and 3 wakes and restarts.

**The turns were real:** 469 `model.settled` lines in the runs' windows, all
on `glm-5-3-flash` with one call and no fallback (`model-counts.txt`), and
all 400 kept replies answer their sum, none an echo (`replies-check.txt`).

**A turn's parts** (`split.txt`; medians of run medians, ms):

| part | plain | TDX | difference | p |
|---|---|---|---|---|
| on the VM: message received until reply posted | 862.0 | 870.5 | +8.5 | 0.99 |
| of which the model call | 586.0 | 585.2 | −0.8 | 0.59 |
| of which the VM's own work | 257.5 | 268.0 | +10.5 | 0.085 |
| on the laptop: message sent until the turn ends | 1023.2 | 1028.0 | +4.8 | 0.97 |

The model call is about 57% of a turn and the VM's own work about 26%.

**Real differences** (`compare-runs-laptop.txt`): a wake 805.3 against
863.3 ms (1.07x) and a restart 3728.7 against 4040.4 ms (1.08x). A cold
Hermes image build took 434 s plain, 470 s TDX (1.08x; `cold-build.txt`).

**Not shown, or uncertain.** One VM each, two zones, and this TDX VM's disk
was slow for its own reasons, so the wake and restart gaps may not be TDX's.
Each turn was one short model call; longer turns with tools were not run.

## 2026-11-10-quote-live

*From `verify.txt`, `verify-instance.txt`, `replay.txt`, `firmware.txt` and
`host-registry.txt`, and `attest/README.md`.*

**What ran.** `pod-quote` on the stock Ubuntu TDX VM (c3-standard-8,
us-central1-b) made a quote (`q.bin`: the CPU's signed statement of the
guest's registers) on a fresh 64-byte nonce (`q.nonce`), and `pod-verify`
checked it on the laptop, 2026-11-10 at about 23:19 UTC.

**Results.** Every check passed; the run recorded no refusals. (That a wrong
instance, zone or project is refused is shown by tests on another quote
recorded from the same VM and boot, `attest/crates/verify/tests/real_quote.rs`
(its quotes now in `crates/pod/testdata/`), and a wrong instance was refused live in
[2026-11-11-lock-unlock](#2026-11-11-lock-unlock).)
`verify-instance.txt`: the chain to Intel's root; debug off; the nonce in
REPORTDATA; the CPU's firmware level (TCB) UpToDate, no advisories (FMSPC,
the CPU family's code, `00806F050000`); MR_OWNER, a register the host sets,
equal to SHA-384 of instance 541049797945993118, project 228527660165 and
zone us-central1-b; and the host (PPID `6c5c6bf95ed6711de2c5e5c85bf06c54`)
in Google's host registry in us-central1-b. `firmware.txt`: Google endorses
the firmware's measurement, MRTD (`f21e6b48...1731e5d3`, build cl
957008435). `replay.txt`: the boot event log reproduces the four registers
the boot extends, RTMR0 to RTMR3.

**Not shown, or uncertain** (attest README). The chain proves a TDX module on
a genuine Intel CPU signed the report, not where the CPU is; a machine whose
attestation key was extracted also passes. MR_OWNER is set by the host, so
the instance check means something only with the registry, whose document
is fetched over TLS and not signed. This VM ran stock Ubuntu, not our image.

## 2026-11-10-measured-image

*From this folder's summary files and `image/README.md`.*

**What ran.** Our image (`image/`): one unified kernel image (UKI)
started by the firmware, a
read-only root under dm-verity whose root hash is on the UKI's command line,
no SSH or login, `pod-quote` on port 7000. Variants: A; B, one file changed;
C, one bit flipped on disk; D, systemd-stub 257 in place of Ubuntu's 255.
Booted as TDX VMs in us-central1-b: a1 and a2 (A on two hosts), b1, c1, d1
(c3-standard-4) and a8 (A on c3-standard-8).

**Results.** Every booted variant passed every quote check
(`*/verify-instance.txt`). The same image on two hosts gave the same
registers (`diff-a1-a2.txt`). A rebuild of A is byte for byte A; A and B
differ in one file of 12,269, and only RTMR1 moves (`build-a-vs-b.txt`,
`diff-a1-b1.txt`). `pod-verify expect` computes RTMR1 and RTMR2 from the
image file; they match the quotes of a1, a2, a8, b1 and d1, and B's do not
match A's quote (`expect-from-image.txt`). C never served a quote: dm-verity
logged `data block 26064 is corrupted` (30 lines in `c1/serial.txt`). A
bigger machine moves only RTMR0 (`diff-a1-c3s4-vs-a8-c3s8.txt`).

**Not shown, or uncertain** (image README). That the image is free of bugs,
or that Ubuntu's or Google's binaries match their sources. RTMR0's ACPI
tables beyond "the same as last time on this machine type". Reproducibility
across machines or days: only a rebuild from the same inputs on the same
machine was identical.

## 2026-11-10-cmdline-swap

*From `kernel-cmdlines.txt`, `diff-*-before-after.txt`,
`difflogs-*-before-after.txt`, `expect-exit-codes.txt` and
`image/README.md`.*

**What ran.** With Secure Boot off, systemd-stub accepts a command line from
the boot entry's options. Test images SWAP-A (stub 255) and SWAP-D (stub
257) carry a deliberately hostile unit playing the host: it adds a boot
entry for the same UKI that appends `pod.swapped=1`, and reboots into it
once. c3-standard-4, us-central1-b.

**Results.** Both swapped boots ran with `pod.swapped=1`. With stub 255
every register and event stayed the same, and `pod-verify expect --quote`
passed the swapped boot (exit 0). With stub 257 only RTMR2 changed, by one
event holding the command line, and `expect --quote` refused it (exit 1). So
a client must pin RTMR1 and RTMR2 to their predicted values, RTMR3 to zero
and RTMR0 per machine type, and the image needs stub 257 or later.

**Not shown, or uncertain.** Only boot-entry options were tried; stub 257
measuring credentials and add-ons too was read from its source, not tested.

## 2026-11-11-disk-encryption

**What ran.** What LUKS2 (dm-crypt), dm-verity and LUKS2 with integrity cost
inside the TD, 2026-11-11 03:51-05:32 UTC, on the C3 TDX VM (us-central1-b),
with its plain twin (us-central1-f) for the block test only. An empty 80 GB
pd-balanced test disk on each VM, one partition per mode; LUKS2 was
AES-256-XTS with 512-byte sectors (`disk-setup.sh`). "No workqueues" means
LUKS opened with `--perf-no_read_workqueue --perf-no_write_workqueue`, which
bypass dm-crypt's kernel workqueues.

**The block device** (`fio-table.txt`, `plain/fio-table.txt`; median of 3):

| | TDX raw | TDX LUKS | TDX LUKS, no workqueues | TDX verity | TDX integrity | plain raw | plain LUKS |
|---|---|---|---|---|---|---|---|
| 4K read, depth 1, p50 µs | 1122 | 1139 | 1122 | 1171 | 1188 | 481 | 489 |
| 4K write and fsync, per s | 915 | 925 | 916 | - | 729 | 2389 | 2156 |
| sequential write, MB/s | 199.6 | 199.6 | 199.4 | - | 93.3 | 199.6 | 199.6 |

The disk's own limits bind (about 200 MB/s, and 4075-4078 IOPS at depth 32,
in every mode). Under them LUKS costs nothing measurable on the TDX VM,
dm-verity about 3% on a single read, and integrity halves sequential writes
and costs 20% of commits. On the plain VM's faster disk LUKS costs 10% of
commits. The TDX VM's raw disk took 2.3x as long per read as the plain VM's.

On a RAM disk, with LUKS's defaults a 4K read took 33 µs on TDX against
12 µs plain, and with the workqueues bypassed 3 µs on both (`ram-table.txt`,
`plain/ram-table.txt`): TDX raises dm-crypt's cost only with the defaults.

**The real stack on encrypted storage** (TDX VM, echo turns; Docker's,
wrangler's and the fakes' state on raw or on LUKS opened with the workqueues
bypassed, three visits each; `compare-runs-warm.txt`, `cold.txt`):

| measure | raw | encrypted | ratio | p |
|---|---|---|---|---|
| warm turn, ms (9 runs a side) | 876.4 | 897.1 | 1.02x | 0.014 |
| warm wake, ms | 974.7 | 987.3 | 1.01x | 0.050, noise |
| cold wake, ms (12 samples a side) | 1778.0 | 1833.1 | 1.03x | 0.0145 |
| cold restart, ms | 5793.9 | 5481.1 | 0.95x | 0.29, noise |
| cold stack start, s (3 a side) | 50.1 | 49.3 | 0.99x | 1.0, noise |

The turn's cost is on the VM: message received until reply posted took
652.0 against 672.5 ms (p 0.004, `split.txt`). HTTP requests were 3-5%
slower at p 0.040-0.094, perhaps partly the tunnel, reopened every visit.

**Not shown, or uncertain.** One VM a side, in two zones. Only the faster
LUKS setting ran in the stack. Cold samples dropped the whole page cache,
colder than a real miss, and cold restarts are bimodal (about 5.2 or 6.3 s),
so a small cost would hide in them. Echo turns write little. Integrity and
dm-verity were not in the stack.

### Zone probe, added after the run

05:45-06:25 UTC, by the coordinating session: the same fio commands
(`zone-probe/probe-bench.sh`) on balanced persistent disks, median of 3
(`zone-probe/*.txt`).

| machine | zone | read p50 | reads/s | commits/s |
|---|---|---|---|---|
| n2-standard-4, ordinary | b | 436 µs | 2203 | 2315 |
| n2-standard-4, ordinary | f | 408 µs | 2346 | 2408 |
| n2-standard-4, ordinary | c | 416 µs | 2258 | 2428 |
| c3-standard-8, plain (main run, not probed) | f | 481 µs | 1962 | 2389 |
| c3-standard-8, TDX, the main run's VM | b | 1057 µs | 1220 | 975 |
| c3-standard-8, TDX, a second VM | c | 342 µs | 2723 | 2735 |

The plain C3 was not probed again: its row is the main run's
(`plain/fio-table.txt`, repeats 473-485 µs; the run's notes list it as
"~485 µs"). The first TDX VM measured 1122 µs and 915 commits/s in the main
run. The zones are alike for an ordinary VM and a second TDX VM was faster
than them, so TDX does not by itself slow a disk operation; the first TDX VM
has a slow disk path of its own (its host or the disk's placement, not
separated). No plain C3 could be made in zone b or c. So the slower wakes,
restarts and builds on that VM
([2026-11-10-tdx-vs-plain](#2026-11-10-tdx-vs-plain),
[2026-11-10-tdx-vs-plain-real](#2026-11-10-tdx-vs-plain-real)) cannot be put
down to TDX. (The notes call them 5-8% and 4-10%; the data give 1.05x to
1.08x for wakes and restarts, and 1.04x to 1.15x for build steps.)

On the second TDX VM (`zone-probe/luks-on-fast-tdx.txt`), LUKS's defaults
cut 4K commits from 3220 to 2555 a second (0.79x) and depth-1 reads to 0.94x;
with the workqueues bypassed both were within the raw device's spread (0.95x
and 1.04x).

## 2026-11-11-c4-tdx-set

**What ran.** Three TDX VMs against three plain VMs, 2026-11-11 06:50-08:56
UTC, all c4-standard-4 (4 vCPU, Granite Rapids), fresh Ubuntu 24.04, 60 GB
Hyperdisk Balanced. TDX in us-central1-f, plain in us-central1-a: no zone
had both, so the arms are in neighbouring zones, a known confound. The VM is
the unit. Echo turns (25 settled calls and 25 fallbacks in each warm run).
The six disks are alike: 4K reads at depth 1 ran 1959-2046 per second on
TDX and 1729-2054 plain (`disk-table.txt`).

**Cold builds** (`build-table.txt`, from `build/*-times.txt` and
`build/*-dev.log`; all six started in the same second; seconds; "unit start
to ready" runs from the dev service's start to the stack's ready line):

| step | tdx-1 | tdx-2 | tdx-3 | plain-1 | plain-2 | plain-3 | ratio |
|---|---|---|---|---|---|---|---|
| bootstrap (overlaps) | 135 | 135 | 128 | 130 | 120 | 120 | 1.13 |
| Hermes image | 521 | 532 | 463 | 443 | 426 | 435 | 1.20 |
| unit start to ready | 933 | 949 | 833 | 777 | 762 | 773 | 1.21 |

Every build step after bootstrap (the stub and Hermes images, the xtask and
cell build, node boot and the whole start) separates. From unit start to
ready the arm medians are 933 and 773 s, a gap of 160 s. The spread within
the TDX arm is 116 s (tdx-2 against tdx-3) and within the plain arm 15 s,
so the gap is 1.4x the TDX arm's spread and about 11x the plain arm's; the
closest VMs across the arms (tdx-3, plain-1) are 56 s apart. (The notes say
"2.6x the widest spread", which these figures do not give.)

**Why** (`cpu-table.txt`, ratio TDX to plain): sha256 0.94, aes-256-gcm on 4
processes 0.91, touching a fresh 4 KiB page 1.49 (1.78-1.86 µs against
1.23-1.26), fork and exec 1.18. A Rust build starts thousands of processes
and faults in gigabytes of memory, so about 1.2x is what these predict. The
6-9% gap on pure arithmetic is not something TDX's design should cause; it
could be the TDX hosts' frequency or power settings, or the zone.

**Warm and cold** (`by-vm-warm.txt`, `by-vm-cold.txt`, `split.txt`; a VM's
figure is the median of its runs, or of its 8 cold samples; ms):

| measure | TDX arm | plain arm | ratio | spread within an arm | separated |
|---|---|---|---|---|---|
| warm turn | 616.4 | 624.4 | 0.99 | 54.5 | no |
| the VM's own share of a turn | 301.5 | 280.0 | 1.08 | 18.8 | yes |
| warm wake | 825.3 | 793.9 | 1.04 | 34.8 | yes |
| warm restart | 3654 | 4739 | 0.77 | 1088 | no |
| cold wake | 1225.6 | 1188.0 | 1.03 | 26.6 | yes |
| cold restart | 3738.7 | 3699.8 | 1.01 | 263.7 | no |

The VM's share of a turn is +21.5 ms, larger than either arm's spread; it
does not show in the whole turn because the model part (a refused call over
the internet plus the echo) varies by up to 19 ms between VMs. Wakes
separate warm and cold; the cold gap (37.6 ms) is larger than the spread
within an arm (26.6 ms), the warm gap (31.3 ms) smaller (34.8). Warm
restarts are not comparable: the answer took about 2.9 or about 4.1 s, and
plain VMs hit the slow mode 22 times in 36, TDX VMs once
(`restart-modes.txt`); cold, no VM hit it. The cause was not found.

**Verification on C4** (`quote-stock/`, `img-c4-1/`). A stock C4 TDX VM
passed every quote check, its instance and host in us-central1-f. Against
C3 it has the same MRTD and RTMR1 to RTMR3, and a different FMSPC
(`20A06D080000`), TDX module and RTMR0. Image D booted on C4, and its
predicted RTMR1 and RTMR2 match the C4 quote.

**Not shown, or uncertain.** One zone per arm: the CPU gap, and so the build
and turn-share gaps, may be the zone f hosts and not TDX. Three VMs a side:
separation with a large gap is the strongest statement possible, not proof.
One cold build per VM; echo turns only; the CPU test is short, single-shot.

## 2026-11-11-n2d-sev-snp

**What ran.** Two AMD SEV-SNP VMs against two plain VMs, 2026-11-11
09:02-11:15 UTC, all n2d-standard-4 (4 vCPU, AMD Milan), stock Ubuntu 24.04,
40 GB pd-balanced, all in us-central1-b. The SNP guests' disks are NVMe and
the plain guests' SCSI. Two VMs a side, so one odd VM sets an arm's median;
snp-1 is that VM here. Echo turns (25 settled, 25 fallbacks per warm run).

**Memory** (`cpu/n2d-*.txt`, `/proc/meminfo` when the CPU test began, before
anything was installed). MemTotal is 15,374,424 kB (14.66 GiB) on SNP and
16,369,232 kB (15.61 GiB) plain, 0.95 GiB less on SNP. Not yet accepted by
the SNP kernel: 8,752 MiB (8.55 GiB) on snp-1 and 8,596 MiB (8.39 GiB) on
snp-2, and 7.52 and 7.51 GiB after the test; none on plain. (The notes say
"15.0 GiB MemTotal (14.66 GiB)", "~0.6 GiB less", "8.6-8.8 GiB" and
"7.9 GiB", which the recorded values do not give.)

**Disk, CPU and builds.** The four disks are alike (358-395 µs per 4K read,
`disk-table.txt`). Pure CPU is about 1% slower on SNP, and touching fresh
pages costs about the same (0.99x and 1.04x on two passes, `cpu-table.txt`).
A cold build took 1578 and 1447 s on SNP, 1451 and 1386 s plain (1.07x,
`build-table.txt`): snp-2 built as fast as plain-1, so it is not separable.

**Warm and cold** (`by-vm-warm.txt`, `by-vm-cold.txt`; ms):

| measure | snp-1 | snp-2 | plain-1 | plain-2 | ratio | gap | spread within an arm | separated |
|---|---|---|---|---|---|---|---|---|
| warm turn | 719.2 | 650.1 | 676.1 | 707.2 | 0.99 | −7.0 | 69.1 | no |
| warm wake | 1185.9 | 1132.9 | 1062.0 | 1038.7 | 1.10 | +109.0 | 53.0 | yes |
| warm restart | 5303.2 | 4933.5 | 4866.3 | 4805.6 | 1.06 | +282.4 | 369.7 | yes |
| warm `http.me.total` | 110.3 | 112.4 | 114.3 | 114.7 | 0.97 | −3.1 | 2.2 | yes |
| warm `http.fragments.total` | 112.7 | 116.1 | 118.3 | 118.2 | 0.97 | −3.8 | 3.4 | yes |
| warm `http.channel.total` | 115.1 | 117.6 | 119.5 | 120.5 | 0.97 | −3.6 | 2.4 | yes |
| cold wake | 1526.0 | 1463.5 | 1389.4 | 1505.7 | 1.03 | +47.2 | 116.3 | no |
| cold restart | 5725.5 | 4611.5 | 5220.2 | 5415.3 | 0.97 | −149.2 | 1114.0 | no |

Five warm measures separate with a gap larger than the spread within either
arm: the wake, SNP slower by 109 ms, and four HTTP request times from the
laptop, SNP faster by 3.1 to 3.8 ms: `http.me.total`, `http.me.ttfb`,
`http.fragments.total` and `http.channel.total`. (The run's notes name only
the wake.) The warm restart separates by less than snp-1 differs from snp-2.
The VM's own share of a turn (+10.9 ms, `split.txt`) does not separate.

**What the SNP report proves** (`snp-stock/`, `firmware/`, `registry/`,
`img/`).

- Reports from snp-1, snp-2 and snp-1 after a reboot verify on the laptop to
  AMD's Milan root, with the chip's own key (VCEK), the nonce and debug off.
  These guests run with SMT allowed and enabled, which Google's own
  `go-sev-guest` refuses by default and which the StackWarp research says
  undermines SNP's integrity.
- The measurement (`2279d213...`) is the same on both VMs and after the
  reboot: the 4-vCPU entry of Google's signed endorsement, which lists one
  per vCPU count. It covers Google's firmware and nothing of ours.
- No field names the owner: the chip id names a chip, the field the host
  could set is zero, and Google's host registry has no entry for these
  chips. Only the vTPM's certificate names the instance, and that is
  Google's statement, not bound to the SNP report.
- Our images booted under SNP, but `pod-quote` is built for TDX and refused,
  so **no SNP report from our images was obtained**; that the report cannot
  tell A from B is inferred, not observed. Google's vTPM view does tell them
  apart (its registers PCR 4, 5, 9 and 11, `img/integrity-a1-vs-b1.txt`), and dm-verity
  refused C as on TDX. So pinning the image on Google's SNP rests on Google,
  and no SVSM (a vTPM inside the protected boundary) was found available.

**Not shown, or uncertain.** Two VMs a side: p 0.33 is the floor, and snp-1
drives most SNP medians. The disk compares NVMe with SCSI. One build per VM,
echo turns only, restart modes not analysed. AMD's current TCB was compared
with bulletin text, not a machine-readable source. No TPM quote was checked.

## 2026-11-11-lock-unlock

*This folder holds only `notes.txt`; the steps below are recorded there and
in no other file of the archive.*

**What ran.** Lock and unlock on the demo VM (c3-standard-8, Intel TDX,
us-central1-b), **stock Ubuntu with SSH and Google's guest agent**, so the
release check compares registers only observed on this VM: it proves the
mechanism, not privacy. `pod-init` serves port 7000 in the VM; the `pod`
client runs on the laptop (`attest/crates/`). The volume is the
demo's LUKS2 file; adding the owner's key over SSH was the one step not
attested. Offline: `cargo test` 135 passed, 0 failed, 5 ignored.

**Results.** With the owner recorded, `pod status` passed all five checks,
the first quote verified through the new binding (the quote's REPORTDATA
field covers the nonce, boot key, state, owner and TLS root). After a reboot the VM came up
locked: no mapper device, the volume not mounted, only port 7000 listening.
While locked, six attempts were refused and the pod stayed locked: a policy
with one byte of RTMR1 changed; a policy naming another instance id; the
wrong disk key (403 `wrong_key`); a second claimant with the right disk key
after the host rewrote the owner hint (403 `owner_mismatch`); a middleman
rewriting the reported state; and one swapping the boot key. `pod unlock`
then sent the key sealed and confirmed the pod open on a fresh quote, in
3 s. A replay of that request on the same boot got 409 `wrong_state` (a
later boot was not tried live). The front door's certificate chains to the
attested TLS root, and a browser over the tailnet reached the agent.

**Not shown.** Privacy from the operator (SSH and the guest agent are still
there). `pod init` on a blank pod, live. A block device as the volume, and a
release whose registers come from an image. Rollback of the volume to an
older copy: nothing detects it. A computer awake when the VM goes down loses
what it did since its last save; this run's was asleep.

## 2026-11-12-measured-pod

**What ran.** Two images built from one definition (`image/`, variants
P and P-debug): the measured image of [2026-11-10-measured-image](#2026-11-10-measured-image)
(systemd-stub 257, a read-only root under dm-verity) carrying the whole
platform: celld, the front door, Dex, OpenBao, sandcastle's node and Docker
double, the computers' images, Docker, Tailscale and `pod-init`. Nothing is
fetched or compiled at boot. P has no SSH daemon, no account that can log in
and no Google guest agent; P-debug adds SSH with one key. State lives on a
separate data disk (LUKS2), where an overlay holds what the platform writes
over the image's read-only tree. The images were proved on a blank scratch
disk (c3-standard-4, TDX), then the demo's data was moved onto P
(c3-standard-8). `cargo test` in `attest/`: 139 passed, 0 failed, 6
ignored at that point.

**Results.**

| | P (locked) | P-debug |
|---|---|---|
| RTMR1, RTMR2 | computed from the image file with `pod-verify expect`; the live quote matches | computed likewise; both differ from P's |
| Under the other image's policy | refused at the release check | refused at the release check |
| SSH | connection refused, with the firewall admitting port 22 | one key |
| Create to port 7000 | 74 s | 81 s and 104 s |
| Unlock to front door | 37 s (scratch), 38 s (the demo's data) | about 40 s |

- **`pod init` ran live for the first time,** on a blank block device: all
  five checks, then the volume made and opened; the stack was ready about
  110 s later, about 100 s of it loading the computers' images.
- **The locked image on the same disk** came up locked, unlocked with a
  policy whose RTMR1 and RTMR2 come from the image file, served sign-in and
  the earlier chat, and an agent answered from its computer. A stop and
  start came back locked with a new boot key. Its serial console showed
  boot lines only: no login prompt and nothing from the platform.
- **The demo's data moved** (`cutover/notes.txt`): the volume was copied to
  a new disk and compared byte for byte, opened first under P-debug and then
  under P. Tailnet mode ran here for the first time: Tailscale came up from
  the volume's state as the same node, and the door bound that address
  alone. The unlock took 9 s; the chats were intact and the agent answered.
- **RTMR0 depends on the disks as well as the machine type:** on one VM it
  differed with and without the data disk attached at boot. It was equal on
  the debug and the locked VM (same type, same disks).
- **Two things broke and were fixed in the image:** `/tmp` was on the
  read-only root, so Docker could not start a computer (now a tmpfs); and
  the owner hint, a file, was lost at each reboot (now a token in the
  volume's LUKS2 header, as untrusted as the file was).

**Not shown.** RTMR0 is observed, not computed. Card rendering end to end.
A restart of the locked pod with the demo's data (the scratch run
restarted). That the image builds to the same bytes on another build box;
the computers' images are rebuilt, not pinned by digest. A blank locked pod
has no attested way to receive its first settings (the model key, the
tailnet login): the scratch run placed them through the debug image.
Rollback of the volume: nothing detects it. A computer awake when the VM
goes down loses what it did since its last save.
