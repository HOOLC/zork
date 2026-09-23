# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates
COPY vendor ./vendor
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p zork-station -p zork-agent-server \
    && cp target/release/zork-station target/release/zork-agent /tmp/

FROM debian:bookworm-slim
ARG SOURCE_REVISION
LABEL org.opencontainers.image.revision=$SOURCE_REVISION
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /tmp/zork-station /usr/local/bin/zork-station
COPY --from=build /tmp/zork-agent /usr/local/bin/zork-agent
ENTRYPOINT ["/usr/local/bin/zork-station"]
