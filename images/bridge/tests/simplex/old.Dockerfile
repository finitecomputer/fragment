# syntax=docker/dockerfile:1.7
# The SimpleX lab's old core (images/bridge/tests/simplex.rs, FIN-463): the
# simplex-chat finite.computer's agents ran, v7.0.2 (finite-mono's
# `simplex-chat.nix` pins this binary and checksum), as lab.Dockerfile's
# client is otherwise: its bot API forwarded out on LAB_API_PORT. The old
# agent whose conversation a new one adopts. Nothing here is an image a
# computer runs.
#
# docker build -f images/bridge/tests/simplex/old.Dockerfile -t fragment-simplex-lab-old:7.0.2 images/bridge/tests/simplex
FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
RUN apt-get -o Acquire::Retries=3 update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y --no-install-recommends ca-certificates libgmp10 libffi8 libssl3t64 zlib1g libnuma1 socat \
 && rm -rf /var/lib/apt/lists/*
ADD --checksum=sha256:5afb1d25efe5ccf564a1ab124bc7f410a7a73171c974a4d8b7f4d8f2e3d62e77 https://github.com/simplex-chat/simplex-chat/releases/download/v7.0.2/simplex-chat-ubuntu-22_04-x86_64 /usr/local/bin/simplex-chat
RUN chmod 0755 /usr/local/bin/simplex-chat \
 && /usr/local/bin/simplex-chat --version | grep -q 'v7\.0\.0\.12'
COPY --chmod=755 <<'SH' /usr/local/bin/lab-entry
#!/bin/sh
# forward the bot API (127.0.0.1:5225 inside) to 0.0.0.0:$LAB_API_PORT
set -eu
socat "TCP-LISTEN:${LAB_API_PORT:-5226},fork,reuseaddr" TCP:127.0.0.1:5225 &
exec /usr/local/bin/simplex-chat "$@"
SH
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/lab-entry"]
