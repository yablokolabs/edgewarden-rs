# EdgeWarden

Cloud-managed Rust edge proxy appliance reference architecture.

![EdgeWarden architecture](docs/assets/edgewarden-architecture.svg)

## Why EdgeWarden

Schools put a proxy appliance between classrooms and the internet, then lose
the cloud link (ISP blip, firewall change, maintenance window) — and classes
go dark because the box was designed cloud-first. EdgeWarden inverts that:
the **dataplane serves from last-known-good unconditionally**, and the cloud
is advisory. Everything else — fleet state, canary OTA, health gates,
telemetry — exists to make that inversion safe to operate at 10k-device
scale.

## Architecture

```text
                     CLOUD

              +------------------+
              |  Control Plane   |
              +--------+---------+
                       |
                   gRPC/mTLS
                       |
           +-----------+-----------+
           |           |           |
        Edge-01     Edge-02     Edge-03
           |           |           |
       Rust Proxy  Rust Proxy  Rust Proxy
           |           |           |
         LAN/WAN     LAN/WAN     LAN/WAN
```

Control plane (cloud) owns *desired* state; each appliance owns *reported*
state plus a durable last-known-good copy. The edge always dials out
(outbound gRPC + mTLS + jittered backoff); the cloud never dials in.
Details: `docs/architecture.md`, `docs/control-plane.md`,
`docs/proxy-datapath.md`.

```mermaid
flowchart TB
    CP[Control Plane<br/>registry · desired state · rollout] -->|gRPC/mTLS outbound| A1[Edge-01 agent]
    CP -->|gRPC/mTLS outbound| A2[Edge-02 agent]
    CP -->|gRPC/mTLS outbound| A3[Edge-03 agent]
    A1 --> P1[Rust proxy] --> WAN1[LAN/WAN]
    A2 --> P2[Rust proxy] --> WAN2[LAN/WAN]
    A3 --> P3[Rust proxy] --> WAN3[LAN/WAN]
```

## Core invariants

> Loss of cloud connectivity must not cause loss of local traffic forwarding.

Proven by `crates/e2e/tests/partition.rs`: connected → policy downloaded →
cloud killed → proxy still forwards → cloud back → auto-reconcile.

## Quick Start

```sh
cargo build --workspace
./target/debug/control-plane &          # gRPC :50051, REST :8080
python3 deploy/docker/echo-upstream.py & # demo upstream :18081
./target/debug/edge-agent --device-id edge-01 &
curl -s localhost:8080/devices | python3 -m json.tool
```

## 3-node local fleet demo

```sh
docker compose -f deploy/compose.yaml up --build
curl -s localhost:8080/devices | python3 -m json.tool
curl -s localhost:19091/metrics | grep edge_policy_version
```

Push policy and watch reconciliation:

```sh
curl -s -X POST localhost:8080/policy -H 'content-type: application/json' \
  -d '{"version":42,"max_connections":512,"idle_timeout_secs":300,"default_upstream":"upstream:18081"}'
sleep 6
curl -s localhost:8080/devices | grep -E 'device_id|reported_policy'
```

## Practical usage examples

Real tasks operators perform with this repo (all runnable locally):

**1. Survive a 9 AM ISP outage without losing classrooms.**
Kill the control plane mid-day; forwarding continues on last-known-good and
reconciles when the link returns. This is the partition test made manual:

```sh
printf 'lesson-plan' | ncat 127.0.0.1 18080   # works while cloud is up
# stop control-plane (Ctrl-C / docker stop)
printf 'still-teaching' | ncat 127.0.0.1 18080  # still echoes back
# restart control-plane; agent re-registers and converges automatically
```

**2. Roll out a stricter filtering policy to one school before the district.**
Bump desired policy, confirm one edge converges and error counters stay flat,
then promote fleet-wide:

```sh
curl -s -X POST localhost:8080/policy -H 'content-type: application/json' \
  -d '{"version":43,"max_connections":256,"idle_timeout_secs":120,"default_upstream":"upstream:18081"}'
sleep 6
curl -s localhost:8080/devices | grep reported_policy   # expect 43
curl -s localhost:19091/metrics | grep -E 'edge_proxy_errors_total|edge_policy_version'
```

**3. Prove the 8:00 AM bell won't melt the proxy.**
Replay the connection storm against a real proxy and keep the JSON as your
capacity receipt:

```sh
./target/debug/storm --target 127.0.0.1:18080 --clients 500 --rounds 10 \
  --payload-bytes 1024 --out /tmp/storm.json
cat /tmp/storm.json   # conns/sec, p50/p95/p99, errors, RSS
```

**4. Ship an OTA to canary, watch a bad build roll itself back.**
The state machine is exercised directly (no real partitions touched):

```sh
cargo test -p edge-ota -- --nocapture
# successful_update_flips_slot ... ok        (B becomes active)
# health_failure_rolls_back_to_a ... ok      (auto-rollback, A stays active)
# corrupt_artifact_fails_verify ... ok       (hash gate)
# bad_signature_fails_verify ... ok          (signature gate)
```

Pair with canary gates in code (`control-plane::rollout`): a ring whose
crash rate, proxy error rate, or health-check failures breach thresholds
**pauses and never auto-resumes** — an operator must `resume()`.

**5. Triage "is it the edge or the cloud?" in one minute.**
Device registry plus Prometheus answers it without SSH:

```sh
curl -s localhost:8080/devices | python3 -m json.tool  # reported vs desired, last_seen age
curl -s localhost:19091/metrics | grep -E 'edge_control_plane_connected|edge_proxy_active|edge_cpu_usage|edge_ota_rollback'
./scripts/failure-demo.sh   # end-to-end fault tour: outage, crash, corrupt config, bad OTA
```

**6. Stand up mTLS for a pilot school.**
Mint short-lived dev certs (7-day, gitignored) and restart both ends with TLS:

```sh
./scripts/gen-certs.sh certs
CONTROL_TLS_CERT=$PWD/certs/server.crt CONTROL_TLS_KEY=$PWD/certs/server.key ./target/debug/control-plane &
EDGE_TLS_CA=$PWD/certs/ca.crt EDGE_CONTROL_PLANE=https://127.0.0.1:50051 ./target/debug/edge-agent &
# production: replace with EST/SPIRE-issued certs + pinned CA (docs/security.md)
```

## TCP dataplane

Tokio `copy_bidirectional`, semaphore-bounded concurrency, connect + idle
timeouts, graceful drain, atomic counters. TPROXY is opt-in and isolated
(`edge-proxy::transparent`, `scripts/nftables-example.sh`); dev mode needs
no root. See `docs/proxy-datapath.md`.

## Fleet control plane

`Register` / `Heartbeat` / `Watch` (bidi) / `ReportTelemetry`, plus
`GET /devices`, `POST /policy`. Outbound-only with jittered backoff.
See `docs/control-plane.md`.

## Desired vs reported state

Cloud: `desired{seq, policy_version}` (monotonic). Edge:
`reported{policy_version, applied_seq}` + `edge-state.json`. Diff yields
idempotent `ApplyPolicy` / `StageOta` / `Noop`; duplicates and stale seq are
safe. At-least-once, never exactly-once. See `docs/adr/ADR-002*`, `ADR-003`.

## Network partition behaviour

Cloud loss flips `edge_control_plane_connected=0` and schedules reconnect;
the accept loop never stops. Restarting the cloud replays the newest desired
state and the edge converges. See `docs/failure-model.md`,
`docs/adr/ADR-006`.

## OTA / rollback

Simulated A/B slots + journaled state machine
(`Idle → Downloading → Verifying → Staged → RebootPending →
BootingCandidate → HealthChecking → Committed`, with `RollbackPending →
RolledBack` / `Failed`). SHA-256 + dev-HMAC verify; crash/power-loss
recovery tested. Production swaps in real bootloader slots +
Sigstore/cosign/TUF. See `docs/ota.md`.

## Security model

rustls + mTLS, pinned CA, per-device identity, least-privilege containers,
no committed secrets. Dev PKI via `scripts/gen-certs.sh`. Threat model
(compromised edge, MITM, stolen cert, malicious update, replay, rollback,
control-plane compromise) in `docs/security.md`.

## Observability

Prometheus `/metrics` on every edge: connections, bytes, errors, latency
histogram, CPU/RSS, `edge_control_plane_connected`, reconnects, policy
version, OTA success/rollback. Scraped by `deploy/prometheus.yml`.

## Performance testing

`storm` (T=0 thundering herd) + Criterion benches + flamegraph/perf notes.
Measured numbers live in committed JSON; architectural expectations are
labelled as such. See `docs/performance.md`.

## Failure injection

`crates/e2e/tests/{partition,reconcile,failures}.rs` plus
`./scripts/failure-demo.sh` (outage, crash, corrupt config, invalid policy,
bad OTA, duplicates, stale state).

## Linux transparent proxy mode

Portable mode by default. For interception: nftables → TPROXY →
`IP_TRANSPARENT` listener → `SO_ORIGINAL_DST` recovery. See
`scripts/nftables-example.sh`, `docs/proxy-datapath.md`, `docs/adr/ADR-007`.

## Architecture decisions

`docs/adr/ADR-001` gRPC vs MQTT · `ADR-002` desired/reported ·
`ADR-003` at-least-once · `ADR-004` immutable edge · `ADR-005` A/B OTA ·
`ADR-006` dataplane independence · `ADR-007` TPROXY · `ADR-008` eBPF
boundaries (userspace first; no buzzword eBPF shipped).

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo bench
./scripts/failure-demo.sh
```

5-minute demo script: `docs/interview-demo.md`.

## Known limitations

Production-oriented reference, not production-ready: plaintext demo mode
(mTLS opt-in), dev-HMAC update signatures (not Sigstore/TUF), simulated
(not real) partition flips and bypass relay (`MockBypassController`),
in-memory registry (no Postgres), no Docker daemon in this build env
(compose files provided, daemon demo not executed here), perf numbers are
local-only until you run `storm` on your hardware.
