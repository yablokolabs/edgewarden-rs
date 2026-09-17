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

- **Measured**: report only numbers you actually ran (commit the JSON).
  Example local runs are in `benchmarks/` when present; CI does not assert
  perf numbers.
- **Architectural expectations** (not promises): single Tokio task per
  connection with no per-connection heap buffering beyond kernel sockets
  scales to thousands of concurrent flows on modest hardware; the bound is
  file descriptors + `max_connections`, not memory per byte proxied. Verify
  on your hardware with `storm` before quoting numbers.
