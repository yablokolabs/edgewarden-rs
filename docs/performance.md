# Performance

Methodology: `storm` reproduces the 8:00 AM school-start thundering herd —
at T=0 all N clients connect simultaneously and do M roundtrips of P bytes.
Measures connections/sec, throughput, p50/p95/p99 latency, errors, and
process RSS; writes machine-readable JSON (`--out`).

```sh
cargo build -p edge-proxy -p storm
./target/debug/storm --target 127.0.0.1:18080 --clients 500 --rounds 10 \
  --payload-bytes 1024 --out /tmp/storm.json
```

Criterion benches (`cargo bench`): `state_diff` (reconciliation hot path),
`backoff_delay` (reconnect math). Profiling:

```sh
cargo flamegraph -p edge-proxy --bin edge-proxy -- --upstream 127.0.0.1:18081
perf record -g ./target/release/edge-proxy ... && perf report
cargo bench
```

## Measured vs expected

- **Measured** (`benchmarks/storm-local.json`, debug build, loopback,
  Python echo upstream, 200 clients × 10 rounds × 1024 B):

  | metric | value |
  |---|---|
  | total requests | 2000, errors 0 |
  | elapsed | 0.106 s |
  | connections/sec | 1887 |
  | throughput | 309 Mbps |
  | latency p50 / p95 / p99 | 0.77 ms / 47.7 ms / 82.5 ms |
  | storm RSS | ~8 MB |

  The p50/p99 spread is the thundering herd itself: all 200 clients connect
  at T=0 and contend on loopback. CI does not assert perf numbers — re-run
  `storm` on your hardware and commit the JSON before quoting capacity.

- **Measured through the Docker fleet** (release proxy in container,
  host-run `storm`, 100 clients × 5 rounds × 256 B, control plane stopped
  mid-run to prove partition behavior): **500 requests, 0 errors,
  2605 conns/sec, 53 Mbps, p50 2.0 ms / p95 22.1 ms / p99 24.1 ms**,
  `edge_control_plane_connected 0`, `edge_proxy_errors_total 0` during the
  outage. Full Compose evidence (register → policy 42 → echo on all three
  edges → kill cloud → storm → restore → policy 43 → all reported=43,
  Prometheus `up` on all edges) is the documented demo path in
  `docs/interview-demo.md`.
- **Architectural expectations** (not promises): single Tokio task per
  connection with no per-connection heap buffering beyond kernel sockets
  scales to thousands of concurrent flows on modest hardware; the bound is
  file descriptors + `max_connections`, not memory per byte proxied. Verify
  on your hardware with `storm` before quoting numbers.
