# OTA

Simulated atomic A/B updates (`edge-ota`). No real partitions are touched;
slots are directories + a JSON journal (`ota-journal.json`, atomic rename).

States: `Idle → Downloading → Verifying → Staged → RebootPending →
BootingCandidate → HealthChecking → Committed`, with `RollbackPending →
RolledBack` and `Failed` on any fault.

- `start_download` is idempotent on duplicate version, errors when busy.
- `verify` checks SHA-256 then dev HMAC-SHA256 signature; mismatch →
  `Failed`, active slot unchanged.
- `simulate_reboot` flips the boot pointer only in memory *after*
  persisting; `health_check(false)` auto-rolls back.
- `recover()` after crash: pre-`Staged` restarts at `Idle`; `RebootPending`
  and later re-enter `HealthChecking` so a bad candidate still rolls back.

Production differences: real bootloader slot selection (e.g. `rauc`,
`bootupd`, dm-verity), Sigstore/cosign or TUF signatures instead of dev
HMAC, streaming verify of large artifacts, and hardware watchdog-gated
commits. The state machine shape stays the same.

Tests cover: success, corrupt artifact, bad signature, health failure,
crash mid-download, power loss during candidate, duplicate start.
