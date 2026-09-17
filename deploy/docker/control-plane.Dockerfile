# syntax=docker/dockerfile:1
FROM rust:1.78-slim AS build
RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY Cargo.toml Cargo.lock* ./
COPY proto ./proto
COPY crates ./crates
RUN cargo build --release -p control-plane -p edge-agent -p edge-proxy -p storm

FROM debian:12-slim AS control-plane
RUN useradd -r -u 10001 edge \
  && mkdir -p /var/lib/edgewarden /var/log/edgewarden \
  && chown 10001:10001 /var/lib/edgewarden /var/log/edgewarden \
  && apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /work/target/release/control-plane /usr/local/bin/control-plane
USER 10001
VOLUME /var/lib/edgewarden
EXPOSE 50051 8080
ENV CONTROL_PERSIST_PATH=/var/lib/edgewarden/registry.json
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -sf http://127.0.0.1:8080/readyz || exit 1
ENTRYPOINT ["/usr/local/bin/control-plane"]
