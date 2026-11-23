#!/usr/bin/env bash
# tools/boat/sandbox/start.sh [SRC SHA]: on a sandbox started from the warm
# snapshot, run before its first build (tools/boat/run runs it each time;
# it is cheap once done). With SRC and SHA, it then makes
# ~/Projects/fragment what the checkout pushed to SRC is: HEAD at SHA, and
# the files as they are in SRC.
set -euo pipefail
REPO=$HOME/Projects/fragment
KEEP=$HOME/.boat-keep
TIMES=$(cd "$(dirname "$0")" && pwd)/times.py

# Boat's restore runs on after the sandbox is `ready`. Until it ends,
# /home/user is a lazy FUSE view whose folders show the time they were made
# again; the real disk, with the folder times put back, takes /home/user
# over about 3 to 16 s AFTER the hydration-done marker (daemon.out:
# "retired /home/user"). A build started in that window sees fresh folder
# times (cargo rebuilds fragment-templates and the cell), and a shell whose
# cwd was in the old view stays in it, at /var/lib/ascii-lazy/retired/home
# (another CARGO_MANIFEST_DIR: ring and everything above it rebuild). So
# wait for both, and reach the repo by its absolute path after this.
# (A sandbox made without --from restores nothing: no lazy view, no marker.)
lazy_home() { awk -v h="$HOME" '$2 == h { f = 1 } END { exit !f }' /var/lib/ascii-lazy/fuse-roots 2>/dev/null; }
for _ in $(seq 1 600); do
  if [[ "$(stat -f -c %T "$HOME")" != fuse* ]]; then
    { [ -e /var/lib/ascii-lazy/hydration-done ] || ! lazy_home; } && break
  fi
  sleep 1
done
[[ "$(stat -f -c %T "$HOME")" != fuse* ]] || { echo "start.sh: $HOME still on Boat's lazy view after 600 s" >&2; exit 1; }
cd /

# Restored symlinks keep the time they were made again, which
# crates/templates/build.rs's directory watch counts, and a folder written
# to during hydration keeps its fresh time: put back the saved times.
python3 "$TIMES" restore
if grep -qa /var/lib/ascii-lazy "$REPO/target/debug/xtask" 2>/dev/null; then
  echo "start.sh: this snapshot's xtask was built in Boat's lazy view and builds there; run tools/boat/golden" >&2
fi

# The snapshot drops ~/.cache: worker-build's downloads (wasm-bindgen,
# wasm-opt, esbuild) are kept in ~/.boat-keep.
mkdir -p ~/.cache
[ -d ~/.cache/worker-build ] || [ ! -d "$KEEP/worker-build" ] || cp -a "$KEEP/worker-build" ~/.cache/worker-build
# What CI's runner has and a Boat sandbox does not; these reset on a resume.
# Containers (stub computers) must reach the node on the host: ufw drops input.
sudo ufw allow in on docker0 >/dev/null
# Chrome's sandbox (miniflare's Chrome for Testing, the preview cards) needs
# unprivileged user namespaces, which Ubuntu 24.04's AppArmor blocks.
sudo sysctl -q -w kernel.apparmor_restrict_unprivileged_userns=0
# CI's Chrome quirk (ci.yml): no IPv6 on links made from here on.
sudo sysctl -q -w net.ipv6.conf.default.disable_ipv6=1

[ $# -eq 2 ] || exit 0
src=$1 sha=$2
cd "$REPO"
[ "$(pwd -P)" = "$REPO" ] || { echo "start.sh: not on the real disk: $(pwd -P)" >&2; exit 1; }
git fetch -q origin master
[ ! -f ~/fragment-head.bundle ] || git fetch -q ~/fragment-head.bundle HEAD
git cat-file -e "$sha^{commit}" 2>/dev/null || { echo "start.sh: no commit $sha (not on origin/master or in the bundle)" >&2; exit 1; }
# HEAD and the index only: the files stay as they are, so a file is
# written (and its time moves) only when its contents differ.
git reset -q "$sha"
rsync -rlcp --exclude=/.git --exclude=/target "$src"/ "$REPO"/
# Remove what the checkout does not have: files deleted there, and files a
# previous run on this sandbox sent that are gone since.
git ls-files -z --cached --others --exclude-standard | python3 -I -c '
import os, sys
src = sys.argv[1]
for rel in sys.stdin.buffer.read().split(b"\0"):
    if rel and not os.path.lexists(os.path.join(src.encode(), rel)) and os.path.lexists(rel):
        os.remove(rel)
' "$src"
