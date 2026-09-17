# edge-state

Desired vs reported state reconciliation with durable last-known-good for
edge fleets, from the [EdgeWarden](https://github.com/yablokolabs/edgewarden-rs)
reference architecture.

The control plane owns monotonic desired state; the edge owns reported
state plus a crash-safe local copy. Delivery is at-least-once with
idempotent transitions — duplicates and stale messages are safe `Noop`s.
There is no exactly-once claim. The dataplane keeps serving the
last-known-good policy while the cloud is unreachable.

```rust
use edge_state::{DesiredState, Policy, ReportedState, StateStore};

let mut store = StateStore::open("edge-state.json")?;
let action = store.ingest(desired)?;
if let edge_state::ReconcileAction::ApplyPolicy(policy) = action {
    // ... enforce locally, then:
    store.mark_policy_applied(&policy, seq)?;
}
// After a restart, `store.last_good_policy` is intact.
```
