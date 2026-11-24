# tools/boat/inside.sh COMMIT COMMAND: what tools/boat/run runs on the
# sandbox, sent as it stands in the checkout. It is CI's e2e build on
# another machine (.github/actions/e2e-build): the builds a cache kept (here
# a Boat snapshot), a checkout whose file times say nothing, and stamp
# (xtask/src/stamp.rs) between them, so cargo takes as built what was built
# from this very content. COMMIT is the checkout's commit, or `master` for
# the snapshot's refresh, which builds what copies will need.
set -euo pipefail
exec 2>&1
commit=$1 command=$2 repo=$HOME/Projects/fragment
cd /

# one run at a time on a sandbox: two would check out over each other
exec 9> /tmp/boat-run.lock
flock -n 9 || { echo "waiting for this sandbox's other run"; flock 9; }

# Boat restores the disk after the sandbox is ready. Until then $HOME is a
# lazy view, and a shell whose cwd is in it stays in it, at another path,
# after the real disk takes over: wait for the real disk, then enter.
lazy() { awk -v h="$HOME" '$2 == h { f = 1 } END { exit !f }' /var/lib/ascii-lazy/fuse-roots 2>/dev/null; }
for _ in $(seq 600); do
  [[ $(stat -f -c %T "$HOME") != fuse* ]] && { [ -e /var/lib/ascii-lazy/hydration-done ] || ! lazy; } && break
  sleep 1
done

# What CI's runner has and a Boat sandbox lacks, lost at every start: the
# computers' containers reach the node (ufw drops their input), Chrome's
# sandbox has its user namespaces, and CI's own IPv6 setting (ci.yml).
sudo ufw allow in on docker0 > /dev/null
sudo sysctl -q -w kernel.apparmor_restrict_unprivileged_userns=0 net.ipv6.conf.default.disable_ipv6=1
# A snapshot drops ~/.cache and Docker's build cache: worker-build's
# downloads and the images' layers are kept beside them instead.
mkdir -p ~/.cache ~/.boat-keep/worker-build
rm -rf ~/.cache/worker-build && ln -s ~/.boat-keep/worker-build ~/.cache/worker-build
export FRAGMENT_IMAGE_CACHE="type=local,src=$HOME/.buildcache,dest=$HOME/.buildcache"
export CC_wasm32_unknown_unknown=clang AR_wasm32_unknown_unknown=llvm-ar CHROME_BIN=/usr/bin/google-chrome

if [ "$commit" = master ]; then
  if [ -d "$repo/.git" ]; then
    git -C "$repo" fetch -q origin master
    if [ -e "$repo/target/built-tree" ] && [ "$(git -C "$repo" rev-parse HEAD)" = "$(git -C "$repo" rev-parse origin/master)" ]; then
      echo "the snapshot is at master $(git -C "$repo" rev-parse --short HEAD): nothing to do"
      exit 86
    fi
  else
    git clone -q https://github.com/finitecomputer/fragment "$repo"
  fi
  # a machine's setup (AGENTS.md, Commands), and the builds copies start from
  rustup update stable --no-self-update
  rustup target add wasm32-unknown-unknown
  rustup component add clippy
  commit=origin/master
  command=$(cat <<'BUILDS'
set -e
v=$(sed -n 's/^pub const WORKER_BUILD_VERSION: &str = "\(.*\)";/\1/p' xtask/src/build.rs)
[ -n "$v" ] || { echo "no WORKER_BUILD_VERSION in xtask/src/build.rs"; exit 1; }
worker-build --version 2> /dev/null | grep -qF "$v" || cargo install worker-build --version "$v" --locked
FRAGMENT_IMAGE_CACHE_SAVE=true cargo xtask e2e --build-only
cargo xtask check || echo "check fails on this master: the snapshot is saved all the same"
# last, so that a copy's first xtask build finds target/debug as xtask left it
cargo xtask e2e --build-only
BUILDS
  )
else
  git -C "$repo" fetch -q origin master
  bundle=$HOME/checkout-$commit.bundle
  [ ! -f "$bundle" ] || { git -C "$repo" fetch -q "$bundle" refs/worktree/boat && rm "$bundle"; }
fi

cd "$repo"
[ "$(pwd -P)" = "$repo" ] || { echo "not on the real disk: $(pwd -P)"; exit 1; }
git checkout -q -f --detach "$commit"
# As CI: stamp, compiled alone, before any cargo build. The record is
# written before the build, not after: it says since when each file has
# held its content, which the build's result does not change, and a command
# that fails must not cost the next run every build.
mkdir -p target
rustc --edition 2021 -O -o /tmp/stamp xtask/src/stamp.rs
/tmp/stamp restore target/built-tree /tmp/stamp-state
/tmp/stamp record target/built-tree /tmp/stamp-state > /dev/null
# Boat's user reads any file (CAP_DAC_READ_SEARCH), which CI's does not: a
# test that makes a file unreadable would fail. The command runs without it.
setpriv --inh-caps=-all --ambient-caps=-all -- bash -c "$command"
