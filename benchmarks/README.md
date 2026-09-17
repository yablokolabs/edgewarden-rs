# Benchmarks

- `storm` crate: school start-time connection storm generator. Run it, keep the JSON.
- Criterion: `cargo bench` (`edge-state: reconcile`, `edge-proxy: relay`).
- Profiling: `cargo flamegraph`, `perf`, see `docs/performance.md`.

Record measured results here (example — replace with your runs):

```sh
cargo build --release -p edge-proxy -p storm
./target/release/storm --target 127.0.0.1:18080 --clients 500 --rounds 10 \
  --payload-bytes 1024 --out benchmarks/storm-local.json
```

Do not invent numbers. Commit the JSON you actually measured.
