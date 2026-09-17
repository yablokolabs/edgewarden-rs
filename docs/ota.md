# OTA

Atomic A/B updates (`edge-ota`). Slot storage is directories + a JSON
journal (`ota-journal.json`, atomic rename + fsync) standing in for real
disk partitions; the state machine, verification, and recovery semantics
are production.

States: `Idle → Downloading → Verifying → Staged → RebootPending →
BootingCandidate → HealthChecking → Committed`, with `RollbackPending →
RolledBack` and `Failed` on any fault.

- `start_download` is idempotent on duplicate version, errors when busy,
  and **rejects signed downgrades** at or below the journaled
  `highest_committed_version` (rollback-attack protection). Genuine
  rollback is the slot-pointer flip in `rollback()`, never a reinstall.
- `verify` checks SHA-256 then the **Ed25519 release signature** over the
  artifact bytes against the device-baked verifying key; mismatch or
  wrong-key artifacts → `Failed`, active slot unchanged.
- `simulate_reboot` flips the boot pointer only *after* persisting;
  `health_check(false)` auto-rolls back and bumps `edge_ota_rollback_total`.
- `recover()` after crash: pre-`Staged` restarts at `Idle`; `RebootPending`
  and later re-enter `HealthChecking` so a bad candidate still rolls back.

Production mapping: real bootloader slot selection (e.g. `rauc`,
`bootupd`, dm-verity) replaces the directory flip; the release signing key
moves to a KMS/HSM and distribution/revocation rides the fleet PKI;
streaming verify covers large artifacts; hardware watchdog gates the
commit. The state machine shape stays the same.

Tests cover: success, corrupt artifact, bad signature, wrong-key signature,
health failure, crash mid-download, power loss during candidate, duplicate
start, signed-downgrade rejection (including across restart), version
ordering.
