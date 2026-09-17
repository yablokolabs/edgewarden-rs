# ADR-005: A/B OTA Strategy

Date: 2026-09-17 · Status: Accepted

## Context

No physical access to schools; a bad update must self-heal.

## Decision

**A/B slots with health-gated commit.** Download → verify (SHA-256 +
signature) → stage inactive slot → simulated reboot → health checks →
commit; any failure rolls back to the previous slot. State is journaled so
crash/power loss resumes correctly.

## Rationale

- Failed candidates never become permanent; the known-good slot is always
  one pointer flip away.
- Explicit state machine (`edge-ota`) makes untested transitions
  unrepresentable and reviewable.

## Consequences

- 2× artifact storage; large images need streaming verify (noted, not in
  reference).
- Requires trustworthy health signals; weak checks turn rollback into
  theater (gates documented in `control-plane::rollout`).
