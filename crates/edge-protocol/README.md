# edge-protocol

[![CI](https://github.com/yablokolabs/edgewarden-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/yablokolabs/edgewarden-rs/actions/workflows/ci.yml)
[![edge-protocol](https://img.shields.io/crates/v/edge-protocol)](https://crates.io/crates/edge-protocol)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/yablokolabs/edgewarden-rs/blob/main/LICENSE)

Outbound-only fleet gRPC protocol and reconnect backoff for cloud-managed
edge appliances, from the [EdgeWarden](https://github.com/yablokolabs/edgewarden-rs)
reference architecture.

The edge always dials out (school firewalls block inbound connections);
the cloud never dials in. `FleetService` provides `Register`, `Heartbeat`,
a persistent bidirectional `Watch` stream, and `ReportTelemetry`.
`Backoff` implements jittered exponential backoff so a fleet-wide outage
does not thundering-herd the control plane on recovery.

```rust
use edge_protocol::Backoff;
use std::time::Duration;

let mut backoff = Backoff::new(Duration::from_millis(200), Duration::from_secs(30));
loop {
    match try_connect().await {
        Ok(_) => {
            backoff.reset();
            break;
        }
        Err(_) => tokio::time::sleep(backoff.next_delay()).await,
    }
}
```

The canonical IDL is `proto/fleet.proto` in this package (mirrored at the
workspace root; a sync test fails on drift).
