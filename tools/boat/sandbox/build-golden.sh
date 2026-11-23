#!/usr/bin/env bash
# tools/boat/sandbox/build-golden.sh: on the golden builder (tools/boat/golden
# copies this folder to ~/.boat-keep and runs it detached). Sets a fresh
# sandbox up, or takes the last snapshot's, brings the warm tree to
# origin/master and builds what copies will need, leaving it ready to save.
# Log: ~/golden.log; exit code: ~/golden.status.
set -euo pipefail
rm -f ~/golden.status
exec > ~/golden.log 2>&1
trap 'echo $? > ~/golden.status' EXIT
t() { local s; s=$(date +%s); "$@"; echo "== $(( $(date +%s) - s ))s  $*"; }
REPO=$HOME/Projects/fragment
KEEP=$HOME/.boat-keep
cd /

# what every login shell needs (.bashrc returns early when non-interactive)
grep -q boat-keep/env.sh ~/.profile || printf '\n# fragment on Boat (tools/boat)\n[ -f ~/.boat-keep/env.sh ] && . ~/.boat-keep/env.sh\n' >> ~/.profile
. "$KEEP/env.sh"
t "$KEEP/start.sh"

# the image's Rust can trail the stable that master follows
t rustup update stable --no-self-update
t rustup target add wasm32-unknown-unknown
t rustup component add clippy
cargo install --list | grep -q '^worker-build v0\.8\.5:' || t cargo install worker-build --version 0.8.5 --locked

mkdir -p ~/Projects ~/.buildcache
[ -d "$REPO/.git" ] || t git clone -q https://github.com/finitecomputer/fragment "$REPO"
cd "$REPO"
[ "$(pwd -P)" = "$REPO" ] || { echo "not on the real disk: $(pwd -P)"; exit 1; }
# xtask finds the repo by devstack's compile-time CARGO_MANIFEST_DIR. If it
# was ever built inside the lazy view, every xtask build goes there: build
# devstack (and xtask with it) again from here.
if grep -qa /var/lib/ascii-lazy target/debug/xtask 2>/dev/null; then
  echo "== xtask was built in Boat's lazy view: rebuilding devstack from the real disk"
  touch crates/devstack/src/lib.rs
fi
t git fetch -q origin master
t git checkout -q -f --detach origin/master
git log -1 --format="== master %h %s"
FRAGMENT_IMAGE_CACHE_SAVE=true t cargo xtask e2e --build-only   # writes ~/.buildcache
t cargo xtask check || echo "== check failed on this master (the snapshot is still saved)"
# the last build of target/release must be xtask's: a plain `cargo build`
# inherits no CARGO_PKG_NAME, which flips ring's fingerprint
t cargo xtask e2e --build-only
rm -rf "$KEEP/worker-build" && cp -a ~/.cache/worker-build "$KEEP/worker-build"
# a build that ran anywhere but the real disk would be every copy's rebuild
if grep -q /var/lib/ascii-lazy ~/golden.log; then echo "a build ran in Boat's lazy view: not saving"; exit 1; fi
# last, right before the snapshot: the times every copy puts back
python3 "$KEEP/times.py" save
du -sh target cell/target ~/.cargo ~/.buildcache
git log -1 --format="== warm at %h %s"
