# edge-telemetry

Prometheus telemetry and `/metrics` endpoint for edge appliances, from the
[EdgeWarden](https://github.com/yablokolabs/edgewarden-rs) reference
architecture.

One `EdgeMetrics` per instance (isolated registries, no global state):
proxy connections/bytes/errors, connection-duration histogram, CPU/RSS,
control-plane connectivity, reconnects, policy version, and OTA
success/rollback counters.

```rust
use std::sync::Arc;

let metrics = Arc::new(edge_telemetry::EdgeMetrics::default());
metrics.connections_total.inc();
edge_telemetry::spawn_metrics_server(metrics, "127.0.0.1:19091".parse()?).await?;
```
