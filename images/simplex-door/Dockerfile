# syntax=docker/dockerfile:1.7
# A SimpleX door's container (docs/computers.md, "F: messengers outside the
# computer"): SimpleX's core, pinned by version and checksum (the bridge's
# pin, simplex/mod.rs), and the door's runner (src/main.rs), on the Debian
# the core's release binary runs on (the lab's and our Hermes image's).
# No computer, no runtime: a `lite` instance its door starts.
#
# Build from the repo's root (as the computer images are):
# docker build --platform linux/amd64 -f images/simplex-door/Dockerfile .

# The runner, static (Alpine's Rust targets musl), its sources touched first
# (images/hermes/Dockerfile says why). The workspace's other members are
# manifests and empty sources: cargo reads them, and builds only this.
FROM rust:1.99-alpine AS build
WORKDIR /src
COPY images/Cargo.toml images/Cargo.lock ./
COPY images/bridge bridge
COPY images/hermes/boot/Cargo.toml hermes/boot/Cargo.toml
COPY images/simplex-door simplex-door
RUN --mount=type=cache,id=images-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=door-target,target=/src/target \
    mkdir -p hermes/boot/src && echo 'fn main() {}' > hermes/boot/src/main.rs \
 && find bridge simplex-door -type f -exec touch {} + \
 && cargo build --release --locked -p fragment-simplex-door \
 && mkdir -p /out && cp target/release/simplex-door /out/

FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
RUN apt-get -o Acquire::Retries=3 update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y --no-install-recommends ca-certificates libgmp10 libffi8 libssl3t64 zlib1g \
 && rm -rf /var/lib/apt/lists/*
# simplex-chat v7.0.3's Ubuntu 24.04 x86_64 binary, its SHA-256 the
# release's own; it reports itself as v7.0.0.12
ADD --checksum=sha256:895fb14cfaa662d1c0947f2f871141fc89680fe3e70bff64c366cbe9e59aa4f0 https://github.com/simplex-chat/simplex-chat/releases/download/v7.0.3/simplex-chat-ubuntu-24_04-x86_64 /opt/door/simplex-chat
RUN chmod 0755 /opt/door/simplex-chat \
 && /opt/door/simplex-chat --version | grep -q 'v7\.0\.0\.12'
COPY --from=build /out/simplex-door /opt/door/simplex-door
ENV DOOR_DIR=/data/door DOOR_CORE_BIN=/opt/door/simplex-chat DOOR_HTTP=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/opt/door/simplex-door"]
