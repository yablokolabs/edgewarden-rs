# Security

- Transport: gRPC with rustls; **mTLS is enforced in production**: the
  control plane requires `CONTROL_TLS_CERT` + `CONTROL_TLS_KEY` +
  `CONTROL_TLS_CLIENT_CA` together and refuses to start with one-way TLS;
  the agent requires `EDGE_TLS_CA` + `EDGE_TLS_CERT` + `EDGE_TLS_KEY`
  together and fails fast on a half-configured identity. Plaintext runs
  only when *no* TLS variables are set (local demo), with a warning on both
  ends. Proven by `crates/e2e/tests/mtls.rs`: full-mTLS register/reconcile
  succeeds, cert-less clients are rejected at the handshake, half-TLS agent
  configs error before touching the network.
  Dev certs: `./scripts/gen-certs.sh` (openssl, 7-day, `certs/` gitignored).
  Production: per-device enrollment (EST/SCEP/SPIRE or cloud IoT PKI),
  short-lived certs, HSM/TPM-backed keys, rotation + revocation.
- Updates: SHA-256 + **Ed25519 release signatures** verified against a
  device-baked verifying key, plus journaled rollback protection (signed
  downgrades rejected at `start_download`). Fleet-wide transparency
  (Sigstore) / TUF snapshot semantics remain the path for multi-signer
  quorum; the wire format (`artifact_sha256_hex`, `signature_hex`) already
  carries what a TUF delegations layer needs.
- Registry: crash-safe snapshot (`CONTROL_PERSIST_PATH`); corrupt snapshots
  fail startup loudly rather than silently dropping the fleet roster.
- Containers: least-privilege (`USER 10001`, `cap_drop: ALL`,
  `no-new-privileges`, read-only root FS + tmpfs, CPU/memory limits,
  healthchecks, persistent volumes for state). No secrets in images.
- `cargo audit` in CI (see `ci.yml`): zero vulnerabilities. One accepted
  *warning* remains — `rustls-pemfile` unmaintained (RUSTSEC-2025-0134),
  transitive via `tonic 0.12` with no fixed release on that branch;
  tracked for the tonic 0.13+ migration. No committed secrets/keys/certs
  (see `.gitignore`).

## Threat model

| Threat | Mitigation / residual |
|---|---|
| Compromised edge | Least-privilege container, immutable image, no cloud creds on device; attacker gets only that LAN's traffic until cert revoked |
| MITM cloud link | Enforced mTLS + pinned CA + device client certs; plaintext exists only as an explicit local-demo mode |
| Stolen device cert | Short-lived certs + revocation + per-device identity limits blast radius |
| Malicious update | Ed25519 verify before stage; wrong-key artifacts rejected; rollback on health failure |
| Replay / duplicate | Monotonic `seq`; stale/duplicate → `Noop` |
| Rollback attack | Journaled `highest_committed_version`; signed-but-older artifacts rejected at download; slot rollback is a pointer flip, never a reinstall |
| Control-plane compromise | Edge keeps serving last-known-good; registry snapshots bound recovery; damage limited to future desired pushes until keys rotated |

Never commit `certs/`, `*.key`, `*.pem`, `.env`, or `target/`.
