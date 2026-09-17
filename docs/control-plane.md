# Control Plane

gRPC (`proto/fleet.proto`): `Register`, `Heartbeat`, `Watch` (bidi stream),
`ReportTelemetry`. The edge always dials out; the cloud never opens inbound
connections (school firewalls block them).

- `Register`: idempotent enrollment, returns desired snapshot.
- `Heartbeat`: unary fallback + reconciliation path, returns
  `(desired_policy_version, policy, desired_software, desired_seq)`.
- `Watch`: persistent stream; edge sends `EdgeReport`, cloud replies with
  `CloudCommand{seq, policy_update|ota_update|noop}`. Either side may drop;
  the edge reconnects with jittered exponential backoff (`edge-protocol`
  `Backoff`: `min(cap, base*2^attempt)` + jitter).
- Operator REST: `GET /devices`, `POST /policy {version,...}`, `GET /healthz`.

## Desired vs reported

Server keeps `policy: Option<Policy>` + monotonic `policy_seq`. `set_policy`
is idempotent on identical version (no seq bump). Devices store
`reported_policy_version` + `applied_seq`. `Watch` only pushes when
`seq > applied_seq` and versions differ; otherwise it sends `Noop`.

## Rollout manager

Rings default to `internal, 0.1%, 1%, 5%, 25%, 100%` (configurable).
`RolloutManager::evaluate(snapshot)` checks crash rate, proxy error rate,
connection success rate, CPU p95, health-check failures, and sample size.
Any breach **latches `paused=true`** and returns `Paused`; the rollout never
auto-resumes — an operator must call `resume()`. Small samples return `Hold`
without pausing.
