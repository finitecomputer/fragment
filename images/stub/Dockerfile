# syntax=docker/dockerfile:1.7
# The stub computer image (docs/computers.md): the bridge with its `script`
# runtime, the fragment CLI its agents ask each other with (`ask`, as our
# Hermes image's do), and a screen page on 6080, nothing else. The
# platform's own lanes run against it, which is the proof that the platform
# needs nothing Hermes-specific (docs/cloudflare-v1.md, the rule).
#
# Build from the repo's root (it carries the CLI, of the root workspace):
# docker build --platform linux/amd64 -f images/stub/Dockerfile .

# The bridge, static (Alpine's Rust targets musl), built where it runs. Its
# sources are touched first: the target cache is every build's on this
# Docker, and cargo takes a path crate as built when its sources are older
# than its last build (images/hermes/Dockerfile).
FROM rust:1.99-alpine AS build
WORKDIR /src
COPY images/Cargo.toml images/Cargo.lock ./
COPY images/bridge bridge
COPY images/hermes/boot hermes/boot
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    find bridge -type f -exec touch {} + \
 && cargo build --release --locked -p fragment-bridge \
 && cp target/release/fragment-bridge /fragment-bridge

# The fragment CLI (cli/), static, as our Hermes image builds it: its
# stage's lines are the Hermes image's (images/hermes/Dockerfile says why
# each), cached by the same ids, and xtask's host test
# `the_cli_stage_copies_what_the_cli_builds_from` reads both.
FROM rust:1.99-alpine AS cli
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY cli cli
COPY crates/core crates/core
COPY crates/nip98 crates/nip98
COPY crates/proto crates/proto
COPY crates/templates crates/templates
COPY templates/blank templates/blank
COPY templates/calories templates/calories
COPY templates/doc templates/doc
COPY templates/inbox templates/inbox
COPY templates/notes templates/notes
COPY templates/todo templates/todo
COPY crates/devstack/Cargo.toml crates/devstack/Cargo.toml
COPY crates/e2e/Cargo.toml crates/e2e/Cargo.toml
COPY crates/fakes/Cargo.toml crates/fakes/Cargo.toml
COPY xtask/Cargo.toml xtask/Cargo.toml
RUN --mount=type=cache,id=cli-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cli-target,target=/src/target \
    for m in crates/devstack crates/e2e crates/fakes xtask; do mkdir -p "$m/src" && : > "$m/src/lib.rs"; done \
 && find cli crates templates -type f -exec touch {} + \
 && cargo build --release --locked -p fragment-cli \
 && mkdir -p /out && cp target/release/fragment /out/

# busybox for the few commands the Computer DO runs in a container (`true`
# while it polls, `touch` for the restore marker); sandbox-shim for its
# DirectoryBackup.
FROM busybox:1.37-musl
COPY --from=docker.io/cloudflare/sandbox:1.0.0 /usr/local/bin/sandbox-shim /usr/local/bin/sandbox-shim
COPY --from=build /fragment-bridge /usr/local/bin/fragment-bridge
COPY --from=cli /out/fragment /usr/local/bin/fragment
COPY images/stub/screen /opt/fragment/screen
# which build this is, served beside the screen (the platform's upgrade and
# rollback lane reads it through the computer's port)
ARG STUB_VERSION=1
RUN echo "$STUB_VERSION" > /opt/fragment/screen/version.txt
# Its answer to a save's hold names `*.scratch` as what the save leaves out
# (docs/computers.md, "The hold"): nothing it keeps another way, only so the
# platform's lanes can see a save leave out what a guest names.
ENV BRIDGE_RUNTIME=script \
    BRIDGE_HELD_LEAVE_OUT=*.scratch \
    BRIDGE_STATE_DIR=/data/bridge \
    BRIDGE_MEDIA_DIR=/tmp/bridge-media \
    BRIDGE_SCRIPT_SCRATCH=/tmp/bridge-script \
    BRIDGE_SCREEN_LISTEN=0.0.0.0:6080 \
    BRIDGE_SCREEN_DIR=/opt/fragment/screen
# no /data: the platform's restore makes it, and a directory from an image
# layer cannot be swapped for a restored one (EXDEV under overlayfs)
RUN mkdir -p /run/computer
EXPOSE 6080
# PID 1 is the bridge: it handles SIGTERM itself (gone within 3 s).
ENTRYPOINT ["/usr/local/bin/fragment-bridge", "run"]
