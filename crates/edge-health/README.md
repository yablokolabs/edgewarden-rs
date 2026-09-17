# edge-health

Health supervision, watchdogs, and fail-open bypass control for edge
appliances, from the [EdgeWarden](https://github.com/yablokolabs/edgewarden-rs)
reference architecture.

Scope discipline: software cannot implement physical fail-to-wire, so this
crate exposes a `BypassController` trait. `MockBypassController` covers
dev/test; `ExecBypassController` shells out to site hardware commands
(bypass-NIC CLI, relay script) with a timeout, leaving state unchanged on
failure so the supervisor retries. Cloud loss alone never marks the edge
unhealthy — local forwarding continues on last-known-good.

```rust
use edge_health::{MockBypassController, Supervisor, SupervisorInput};
use std::sync::Arc;
use std::time::Duration;

let supervisor = Supervisor::new(Arc::new(MockBypassController::new()), 0.9, Duration::from_secs(5));
let state = supervisor.evaluate(&SupervisorInput { proxy_alive: true, ..Default::default() });
```
