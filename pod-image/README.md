# pod-image: the pod's measured image

The boot image a pod runs as an Intel TDX confidential VM on Google Cloud
(`docs/pod.md`). One definition makes two images: the **locked** image,
and its **debug** sibling, which adds SSH for one key and measures
differently by construction. The tooling is `cargo xtask pod`
(`xtask/src/pod/`, `docs/pod.md` "The image"); nothing here is run by
hand.

This folder sits beside `images/`, not in it: `images/` is the computer
images' workspace, and nothing here is an input to their key.

## What the image is

- One unified kernel image (UKI) at `\EFI\BOOT\BOOTX64.EFI`: the kernel,
  the initrd and the command line in one PE file, started by the firmware
  directly (no shim, no GRUB), wrapped in systemd-stub 257 so each section
  is measured into RTMR2.
- A read-only erofs root under dm-verity, whose root hash is on the UKI's
  command line: the UKI's hash covers every byte of the root.
- Ubuntu 24.04 packages from a pinned snapshot (`Snapshot=` in
  `mkosi/mkosi.conf`) and Google's kernel.
- `pod-init` on port 7000, built from the tree being built; it keeps the
  data disk (device name `pod-data`) locked until the owner's CLI has
  checked the pod's attestation, then runs `pod-opened`, which starts the
  platform from the image with its state on the volume.
- The locked image has no SSH, no login and no cloud agent; getty,
  rescue, emergency and the debug shell are masked, and the console shows
  boot lines only.

Evidence: `docs/explorations/private-pod-findings.md`, sections
`2026-11-10-measured-image`, `2026-11-10-cmdline-swap` and
`2026-11-12-measured-pod`.

## Layout

| Path | What |
|---|---|
| `pins.jsonc` | the tools: mkosi's commit, systemd-stub 257's package and hash |
| `mkosi/` | the locked image: `mkosi.conf`, `mkosi.conf.d/50-platform.conf` (the platform's packages), `mkosi.repart/` (ESP, root, verity), `mkosi.extra/` (the files the image carries), `mkosi.postinst.chroot` |
| `debug/mkosi/` | what the debug sibling adds: sshd, sudo, a host key made in memory at each boot |
| `pinned-platform/` | **debt**: the platform the image carries, pinned (`pins.jsonc`, `debs.sha256`, two patches); see `docs/technical-debt-ledger.md` |

The build adds, per image: `mkosi.extra/usr/bin/pod-init`, systemd-stub
257 at `mkosi.extra/usr/lib/systemd/boot/efi/linuxx64.efi.stub`,
`mkosi.extra/usr/lib/pod/variant` (`P` or `P-debug`, the names the proof
of concept's images were measured under), `mkosi.conf.d/40-platform-paths.conf`
(the staged platform and its packages), and for the debug image the one
SSH key.

## The shell that remains

mkosi and systemd need scripts, so a few remain. They are the image's
content, never tooling, and no developer runs them:

- `mkosi/mkosi.postinst.chroot`: mkosi's hook inside the image (the
  stack's user, ownership, no getty, root locked).
- `mkosi/mkosi.extra/usr/lib/pod/`: `pod-opened` (pod-init's open hook),
  `pod-addr`, `pod-computers`, `pod-stack` (units' `ExecStart`) and
  `gce-disk-name` (a udev `IMPORT{program}`).

Which of these should become Rust is a debt entry
(`docs/technical-debt-ledger.md`, "The pod image's unit helpers are
shell"). `cargo xtask check` refuses a script anywhere else in this
folder, and any `.sh` file in the repo.
