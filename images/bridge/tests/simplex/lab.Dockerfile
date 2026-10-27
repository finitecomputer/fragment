# syntax=docker/dockerfile:1.7
# The SimpleX lab's client (images/bridge/tests/simplex.rs): simplex-chat,
# pinned by version and checksum, on the Debian our Hermes image's base runs
# (trixie), with the libraries its release binary links. The bot API (`-p`)
# listens on 127.0.0.1 only; the entrypoint forwards it out of the container
# on LAB_API_PORT, for the test on the host. Nothing here is an image a
# computer runs.
#
# docker build -f images/bridge/tests/simplex/lab.Dockerfile -t fragment-simplex-lab:7.0.3 images/bridge/tests/simplex
FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
RUN apt-get -o Acquire::Retries=3 update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y --no-install-recommends ca-certificates libgmp10 libffi8 libssl3t64 zlib1g socat \
 && rm -rf /var/lib/apt/lists/*
# simplex-chat v7.0.3, the latest stable release on 2026-10-22: the release's
# Ubuntu 24.04 x86_64 binary, its SHA-256 the release's own (GitHub's asset
# digest). It reports itself as v7.0.0.12.
ADD --checksum=sha256:895fb14cfaa662d1c0947f2f871141fc89680fe3e70bff64c366cbe9e59aa4f0 https://github.com/simplex-chat/simplex-chat/releases/download/v7.0.3/simplex-chat-ubuntu-24_04-x86_64 /usr/local/bin/simplex-chat
RUN chmod 0755 /usr/local/bin/simplex-chat \
 && /usr/local/bin/simplex-chat --version | grep -q 'v7\.0\.0\.12'
COPY --chmod=755 <<'EOF' /usr/local/bin/lab-entry
#!/bin/sh
# forward the bot API (127.0.0.1:5225 inside) to 0.0.0.0:$LAB_API_PORT
set -eu
socat "TCP-LISTEN:${LAB_API_PORT:-5226},fork,reuseaddr" TCP:127.0.0.1:5225 &
exec /usr/local/bin/simplex-chat "$@"
EOF
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/lab-entry"]
