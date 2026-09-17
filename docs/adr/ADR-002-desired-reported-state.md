# ADR-002: Desired / Reported State Reconciliation

Date: 2026-09-17 · Status: Accepted

## Context

Edge and cloud disagree transiently (partitions, restarts, redelivery).
Need convergence without distributed locks.

## Decision

Cloud owns monotonic `desired{seq, policy_version, software_version}`;
edge owns `reported{policy_version, applied_seq}` + durable
last-known-good. Edge diffs on every heartbeat/`Watch` message and applies
idempotent transitions; highest `seq` wins.

## Rationale

- Converges under at-least-once redelivery; duplicates/stale are `Noop`.
- Dataplane never blocks: last-known-good serves during partitions.
- Operators reason about one number (`seq`) plus versions.

## Consequences

- Eventual consistency only; no instant global rollout guarantee.
- Requires monotonic seq discipline on the server (enforced in `Registry`).
