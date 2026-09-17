# Proxy Datapath

`edge-proxy` is a Tokio TCP forwarder:

- Bounded concurrency via `Semaphore(max_connections)`; over-limit
  connections are refused fast (`refused_over_limit` counter).
- `tokio::io::copy_bidirectional` drives both directions with no userspace
  buffering: backpressure stays in TCP windows, memory stays bounded, and
  half-close semantics come for free.
- Connect timeout (`connect_timeout_secs`) and idle timeout
  (`idle_timeout_secs`, implemented as a `timeout()` around the relay)
  bound every connection's lifetime.
- Graceful shutdown: `serve_until(watch)` stops accepting; in-flight relays
  drain or hit idle timeout.
- Counters (`ConnectionCounters`, atomics) feed `edge-telemetry` without locks.

```text
client ──TCP──► accept ──semaphore──► dial upstream ──► copy_bidirectional ──► WAN
                   │                        │                    ▲
                   │ refused fast           │ timeout            │ TCP backpressure
                   ▼                        ▼                    │
                metrics                  metrics              metrics
```

## Transparent mode

Optional Linux path (`src/transparent.rs`):

```text
client → netfilter/nftables → TPROXY → edgewarden Rust proxy → original destination
```

- `bind_transparent()` sets `IP_TRANSPARENT` (best-effort fallback with a
  warning when unprivileged).
- `original_destination(fd)` reads `SO_ORIGINAL_DST` (getsockopt 80).
- `scripts/nftables-example.sh` installs the divert rules.

Portable mode never touches this module; unit tests run as non-root.
