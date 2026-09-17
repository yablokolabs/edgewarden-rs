# Failure Model

Delivery is **at-least-once** with idempotent transitions — never
exactly-once (see `docs/adr/ADR-003`). Consequences:

| Failure | Behaviour |
|---|---|
| Control-plane outage | Proxy keeps forwarding on last-known-good; agent backs off with jitter; `edge_control_plane_connected=0` |
| Slow network / latency | Heartbeat timeouts; no dataplane impact |
| Packet loss | TCP retransmits; relay errors counted, connection closed, new ones accepted |
| Edge restart | State reloaded from disk; proxy binds before cloud sync |
| Proxy crash (task) | Supervisor counts failures; `MockBypassController` only engages on proxy death when fail-open is configured; cloud loss alone never triggers bypass |
| Corrupt config file | `StateStore::open` errors; caller falls back to compiled defaults, never panics |
| Invalid policy (v0/empty) | Treated as no-op; reported version unchanged |
| OTA health failure | Automatic rollback, `edge_ota_rollback_total++` |
| Bad certificate | TLS handshake fails; agent keeps serving + retries with backoff |
| Duplicate command | Same `seq`+version → `Noop` |
| Stale desired state | `seq <= applied_seq` → ignored, never regresses |

Run `./scripts/failure-demo.sh` for the live tour. Integration proof:
`crates/e2e/tests/{partition,reconcile,failures}.rs`.
