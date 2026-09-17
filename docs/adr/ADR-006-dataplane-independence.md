# ADR-006: Dataplane Independence

Date: 2026-09-17 · Status: Accepted

## Context

Schools lose internet when middleboxes depend on the cloud. That failure
mode is unacceptable during class.

## Decision

**The dataplane never synchronously depends on the cloud.** Proxy boots
from last-known-good, serves unconditionally, and treats cloud sync as
advisory updates only.

## Rationale

- Partitions are normal (maintenance, ISP outages, firewall changes), not
  exceptional.
- Proven by `crates/e2e/tests/partition.rs`: cloud down → forwarding
  continues → cloud back → reconcile.

## Consequences

- Policy staleness during partitions is accepted and surfaced
  (`reported` vs `desired` age in `/devices`).
- Features requiring live cloud (new policy, OTA) degrade to "keep serving
  old" rather than failing closed.
