# ADR-007: TPROXY Approach

Date: 2026-09-17 · Status: Accepted

## Context

Schools need interception without per-client proxy config. Linux offers
TPROXY + policy routing; it needs privileges and network cooperation.

## Decision

**Portable proxy by default; TPROXY isolated behind an opt-in module**
(`edge-proxy::transparent`, `scripts/nftables-example.sh`). Reference
implements `IP_TRANSPARENT` bind + `SO_ORIGINAL_DST` recovery and documents
the nftables divert; it does not require root for build/test/demo.

## Rationale

- Keeps development, CI, and Docker demo rootless and portable.
- Transparent mode is a deployment configuration, not a code fork: same
  relay, different listener + destination lookup.

## Consequences

- Full interception (TLS inspection, per-user policy) is out of scope for
  the reference; hooks are documented for follow-ups.
- Operators must still do the privileged network setup per site.
