# ADR-003: At-Least-Once vs Exactly-Once Delivery

Date: 2026-09-17 · Status: Accepted

## Context

Fleet messaging crosses unreliable school uplinks. Exactly-once is tempting
but expensive.

## Decision

**At-least-once with idempotent receivers.** Retries and duplicates are
expected; applying the same `seq`/version twice is a safe no-op. We never
claim exactly-once.

## Rationale

- Exactly-once needs transactions/dedup windows across restarts — unjustified
  complexity for policy/OTA pointers.
- Idempotency (`mark_policy_applied`, OTA `start_download` dedup, rollout
  seq checks) gives the same observable outcome cheaper.

## Consequences

- Every handler must be retry-safe (tested: duplicate/stale cases).
- Telemetry may double-count under retry; counters remain monotonic so
  Prometheus `rate()` stays correct.
