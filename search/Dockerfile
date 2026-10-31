# syntax=docker/dockerfile:1.7
# The search service (search/README.md): fragment-search, on Debian's slim
# base (its binary links glibc alone: TLS is rustls). One image for the
# platform's Containers and for a self-hosted deployment.
#
# Build from search/: docker build --platform linux/amd64 -t fragment-search .

FROM rust:1.99-bookworm AS build
# Lance's build reads protobuf definitions, the well-known types among them
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
# its sources touched first: the target cache is every build's on this
# Docker (images/stub/Dockerfile)
ARG JOBS=8
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    find src -type f -exec touch {} + \
 && cargo build --release --locked -j "$JOBS" \
 && cp target/release/fragment-search /fragment-search

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /fragment-search /usr/local/bin/fragment-search
# nobody: a disk store's directory is theirs to write (README.md)
USER 65534:65534
ENV SEARCH_LISTEN=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/fragment-search"]
