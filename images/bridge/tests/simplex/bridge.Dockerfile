# syntax=docker/dockerfile:1.7
# The SimpleX lab's computer (images/bridge/tests/simplex.rs): the bridge
# as the stub image runs it (its scripted runtime, its screen on 6080), with
# SimpleX's core beside it, on the Debian our Hermes image's base runs. Not
# an image a computer runs: the stub is busybox on musl, and the core's
# release binary needs glibc; the real image's own lane is docker.rs's.
#
# Build from images/:
#   docker build -f bridge/tests/simplex/bridge.Dockerfile -t fragment-simplex-bridge:test .
FROM rust:1.99-alpine AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY bridge bridge
COPY hermes/boot hermes/boot
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    find bridge -type f -exec touch {} + \
 && cargo build --release --locked -p fragment-bridge \
 && cp target/release/fragment-bridge /fragment-bridge

FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
RUN apt-get -o Acquire::Retries=3 update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y --no-install-recommends ca-certificates libgmp10 libffi8 libssl3t64 zlib1g \
 && rm -rf /var/lib/apt/lists/*
ADD --checksum=sha256:895fb14cfaa662d1c0947f2f871141fc89680fe3e70bff64c366cbe9e59aa4f0 https://github.com/simplex-chat/simplex-chat/releases/download/v7.0.3/simplex-chat-ubuntu-24_04-x86_64 /usr/local/bin/simplex-chat
RUN chmod 0755 /usr/local/bin/simplex-chat && /usr/local/bin/simplex-chat --version | grep -q 'v7\.0\.0\.12'
COPY --from=build /fragment-bridge /usr/local/bin/fragment-bridge
COPY stub/screen /opt/fragment/screen
ENV BRIDGE_RUNTIME=script \
    BRIDGE_STATE_DIR=/data/bridge \
    BRIDGE_MEDIA_DIR=/tmp/bridge-media \
    BRIDGE_SCRIPT_SCRATCH=/tmp/bridge-script \
    BRIDGE_SCREEN_LISTEN=0.0.0.0:6080 \
    BRIDGE_SCREEN_DIR=/opt/fragment/screen \
    BRIDGE_SIMPLEX_BIN=/usr/local/bin/simplex-chat
# the test runs it as its own user, so /data (a directory of the host's) is
# its to read and restore; the hold's files are anyone's
RUN mkdir -p /run/computer && chmod 1777 /run/computer
EXPOSE 6080
ENTRYPOINT ["/usr/local/bin/fragment-bridge", "run"]
