# edge-ota

[![CI](https://github.com/yablokolabs/edgewarden-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/yablokolabs/edgewarden-rs/actions/workflows/ci.yml)
[![edge-ota](https://img.shields.io/crates/v/edge-ota)](https://crates.io/crates/edge-ota)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/yablokolabs/edgewarden-rs/blob/main/LICENSE)

Persistent A/B OTA update state machine with Ed25519 verification and
rollback protection, from the [EdgeWarden](https://github.com/yablokolabs/edgewarden-rs)
reference architecture.

`Idle → Downloading → Verifying → Staged → RebootPending →
BootingCandidate → HealthChecking → Committed`, with `RollbackPending →
RolledBack` and `Failed`. Every transition is journaled (atomic rename +
fsync), so crashes and power loss resume correctly. Artifacts are verified
by SHA-256 plus an Ed25519 release signature against the device-baked key;
signed downgrades are rejected at download (rollback-attack protection —
genuine rollback is a slot-pointer flip, never a reinstall).

```rust
use edge_ota::{signing_key_from_seed, sign_artifact, OtaManager};

let signing = signing_key_from_seed(&[7u8; 32]); // release infra only
let (sha, sig) = sign_artifact(&bytes, &signing);
// ... ship (sha, sig) in the manifest; the device verifies:
let mut ota = OtaManager::open(dir, &verifying_key)?;
ota.start_download(&manifest)?;
ota.ingest_chunk(&bytes, &manifest)?;
ota.verify(&manifest)?; // hash + signature, else Failed
```
