# syntax=docker/dockerfile:1
FROM rust:1.89-slim AS build
RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY Cargo.toml Cargo.lock* ./
COPY proto ./proto
COPY crates ./crates
RUN cargo build --release -p edge-agent -p edge-proxy

FROM debian:12-slim AS edge
RUN useradd -r -u 10001 edge \
  && mkdir -p /var/lib/edgewarden \
  && chown 10001:10001 /var/lib/edgewarden \
  && apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /work/target/release/edge-agent /usr/local/bin/edge-agent
COPY --from=build /work/target/release/edge-proxy /usr/local/bin/edge-proxy
USER 10001
VOLUME /var/lib/edgewarden
EXPOSE 18080 19091
ENV EDGE_STATE_PATH=/var/lib/edgewarden/edge-state.json
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -sf http://127.0.0.1:19091/metrics || exit 1
ENTRYPOINT ["/usr/local/bin/edge-agent"]
