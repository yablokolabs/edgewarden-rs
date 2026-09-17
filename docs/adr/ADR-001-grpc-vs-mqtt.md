# ADR-001: gRPC vs MQTT for Fleet Transport

Date: 2026-09-17 · Status: Accepted

## Context

Tens of thousands of firewalled school appliances need policy push +
telemetry. Candidates: gRPC/mTLS (HTTP/2 streams), MQTT (brokered
pub/sub), AWS IoT Core (managed MQTT).

## Decision

**gRPC over mTLS, edge-initiated**, with unary heartbeat + persistent
`Watch` bidi stream. MQTT/IoT Core remain viable managed alternatives but
are not required for the reference.

## Rationale

- Strong typing via protobuf; unary + streaming in one transport.
- mTLS is first-class (rustls/tonic); per-device identity maps to certs.
- Works outbound-only through school NAT/firewalls; no inbound ports.
- No broker to operate for the reference implementation; control plane is a
  plain Rust binary.

## Consequences

- Must implement reconnect + backoff ourselves (done: jittered exponential).
- Very large fleets may still want IoT Core for managed fan-out; the
  `FleetService` IDL ports cleanly to MQTT topics (`device/{id}/desired`,
  `device/{id}/reported`).
