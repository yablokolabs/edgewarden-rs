# ADR-004: Immutable Edge Model

Date: 2026-09-17 · Status: Accepted

## Context

Configuration drift and on-box tampering are top K-12 support costs.

## Decision

Appliance runs an **immutable image** (minimal OS + container); all mutable
state is confined to versioned files (`edge-state.json`, OTA journal).
No SSH-and-edit workflows; changes arrive as desired-state versions or OTA
slots.

## Rationale

- Drift becomes impossible by construction; support reproduces from version
  numbers alone.
- Rollback = point at previous version/slot (fast, tested).

## Consequences

- Debugging needs explicit escape hatches (metrics, journaled logs), not
  live edits.
- Reference uses Flatcar/Talos-style posture conceptually; Docker demo
  stands in for the image pipeline.
