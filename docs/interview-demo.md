# Interview Demo (5 minutes)

Prereqs: `cargo build --workspace` (or `docker compose` in `deploy/`).

## 1. Start control plane + three appliances (0:00)

```sh
# Terminal 1: control plane (gRPC :50051, REST :8080)
./target/debug/control-plane

# Terminal 2-4 (or docker compose): echo upstream + 3 edges
python3 deploy/docker/echo-upstream.py &  # :18081
EDGE_DEVICE_ID=edge-01 EDGE_CONTROL_PLANE=http://127.0.0.1:50051 \
  EDGE_LISTEN=127.0.0.1:18080 EDGE_UPSTREAM=127.0.0.1:18081 \
  ./target/debug/edge-agent &
# repeat for edge-02 (:18082), edge-03 (:18083)
# OR: docker compose -f deploy/compose.yaml up --build
```

## 2. Device registration (0:45)

```sh
curl -s localhost:8080/devices | python3 -m json.tool
# three devices, reported_policy_version=0, last_seen_age_secs small
curl -s localhost:8080/healthz  # ok
```

## 3-4. Push policy, watch reconciliation (1:15)

```sh
curl -s -X POST localhost:8080/policy \
  -H 'content-type: application/json' \
  -d '{"version":42,"max_connections":512,"idle_timeout_secs":300,"default_upstream":"upstream:18081"}'
sleep 6  # heartbeat interval
curl -s localhost:8080/devices | grep -E 'device_id|reported_policy'
# reported_policy_version=42 on all three; edge logs "applying desired policy"
curl -s localhost:19091/metrics | grep edge_policy_version  # 42
```

## 5-6. Kill cloud, prove dataplane survives (2:30)

```sh
# Stop control-plane (Ctrl-C) or: docker stop edgewarden-demo-control-plane-1
printf 'hello-during-outage' | ncat 127.0.0.1 18080  # echoes back
./target/debug/storm --target 127.0.0.1:18080 --clients 50 --rounds 3 --out /tmp/partition-storm.json
cat /tmp/partition-storm.json  # errors=0 while cloud is down
```

Say the invariant out loud: **loss of cloud connectivity must not cause
loss of local traffic forwarding.**

## 7-8. Restore cloud, auto-reconcile (3:30)

```sh
./target/debug/control-plane &  # restart
curl -s -X POST localhost:8080/policy -H 'content-type: application/json' -d '{"version":43}'
sleep 6
curl -s localhost:8080/devices | grep reported_policy  # 43 everywhere
```

## 9-11. OTA: good update, then bad update + rollback (4:00)

```sh
cargo test -p edge-ota -- --nocapture 2>&1 | grep -E '^test '
# successful_update_flips_slot ... ok
# health_failure_rolls_back_to_a ... ok
# corrupt_artifact_fails_verify ... ok
```

Narrate: stage B → verify SHA-256/signature → simulated reboot → health
gates → commit; on failure automatic `RolledBack`, active slot stays A.

## 12-13. Connection storm + metrics (4:40)

```sh
./target/debug/storm --target 127.0.0.1:18080 --clients 200 --rounds 5 \
  --payload-bytes 256 --out /tmp/storm.json
cat /tmp/storm.json  # conns/sec, p50/p95/p99, errors, RSS
curl -s localhost:19091/metrics | grep -E 'edge_proxy_(connections_total|active|errors)'
```

Close: point at `docs/architecture.md`, `docs/adr/`, and
`crates/e2e/tests/partition.rs` as the receipts.
