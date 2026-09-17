# syntax=docker/dockerfile:1
FROM rust:1.78-slim AS build
RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY Cargo.toml Cargo.lock* ./
COPY proto ./proto
COPY crates ./crates
RUN cargo build --release -p control-plane -p edge-agent -p edge-proxy -p storm

FROM debian:12-slim AS control-plane
RUN useradd -r -u 10001 edge && apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /work/target/release/control-plane /usr/local/bin/control-plane
USER 10001
EXPOSE 50051 8080
ENTRYPOINT ["/usr/local/bin/control-plane"]
