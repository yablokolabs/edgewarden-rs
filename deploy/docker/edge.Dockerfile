# syntax=docker/dockerfile:1
FROM rust:1.78-slim AS build
RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY Cargo.toml Cargo.lock* ./
COPY proto ./proto
COPY crates ./crates
RUN cargo build --release -p edge-agent -p edge-proxy

FROM debian:12-slim AS edge
RUN useradd -r -u 10001 edge && apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /work/target/release/edge-agent /usr/local/bin/edge-agent
COPY --from=build /work/target/release/edge-proxy /usr/local/bin/edge-proxy
USER 10001
EXPOSE 18080 19091
ENTRYPOINT ["/usr/local/bin/edge-agent"]
