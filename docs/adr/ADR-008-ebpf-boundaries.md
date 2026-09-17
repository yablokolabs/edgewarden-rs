# ADR-008: eBPF / XDP Boundaries

Date: 2026-09-17 · Status: Accepted

## Context

eBPF/XDP tempts as a performance story, but most proxy logic (TCP
reassembly, TLS, policy) does not belong there.

## Decision

**Userspace first; eBPF only for telemetry/classification behind an
optional feature — and this reference ships documentation, not code.**
No eBPF is compiled or tested here; adding it for buzzwords would be fake
complexity.

## Rationale

- `copy_bidirectional` in Tokio already keeps the hot path in the kernel
  (TCP windows, zero userspace copies per byte beyond `splice`-class
  paths). The bottleneck is sockets/policy, not packet parsing.
- Legitimate eBPF uses: SYN/drop counters, per-SNI flow labels, DDoS
  early-drop. All are observability/safety, never correctness.

## Consequences

- If eBPF lands later: small, optional, tested C/BPF + loader with a
  userspace fallback; never required for `cargo test`.
- Performance claims must come from `storm` + flamegraphs, not from
  mentioning XDP.
