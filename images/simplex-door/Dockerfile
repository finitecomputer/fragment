# syntax=docker/dockerfile:1.7
# SPIKE (branch spike/simplex-door, never merged): the SimpleX door's
# container: SimpleX's core (simplex-chat v7.0.3, pinned by checksum, the
# computer image's own) and the door's runner (src/main.rs), on the Debian
# the bridge's SimpleX lab runs the same binary on. A `lite` instance,
# started by the door's Durable Object (cell/door.mjs).
#
# Build from images/:
#   docker build --platform linux/amd64 -f simplex-door/Dockerfile .
FROM rust:1.99-alpine AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY bridge bridge
COPY hermes/boot hermes/boot
COPY simplex-door simplex-door
# its own workspace (simplex-door/Cargo.toml), the bridge a path crate of it
RUN --mount=type=cache,id=images-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=sxdoor-target,target=/src/simplex-door/target \
    find bridge simplex-door -type f -exec touch {} + \
 && cargo build --release --locked --manifest-path simplex-door/Cargo.toml \
 && cp simplex-door/target/release/simplex-door /simplex-door

FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
RUN apt-get -o Acquire::Retries=3 update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y --no-install-recommends ca-certificates libgmp10 libffi8 libssl3t64 zlib1g \
 && rm -rf /var/lib/apt/lists/*
ADD --checksum=sha256:895fb14cfaa662d1c0947f2f871141fc89680fe3e70bff64c366cbe9e59aa4f0 https://github.com/simplex-chat/simplex-chat/releases/download/v7.0.3/simplex-chat-ubuntu-24_04-x86_64 /opt/door/simplex-chat
RUN chmod 0755 /opt/door/simplex-chat && /opt/door/simplex-chat --version | grep -q 'v7\.0\.0\.12'
COPY --from=build /simplex-door /opt/door/simplex-door
ENV DOOR_DIR=/data/door DOOR_CORE_BIN=/opt/door/simplex-chat
EXPOSE 8080
ENTRYPOINT ["/opt/door/simplex-door"]
