# Security

- Transport: gRPC with rustls; **mTLS** in production (device client cert +
  pinned CA). Demo defaults to plaintext with a warning; pass
  `CONTROL_TLS_CERT/KEY` (server) and `EDGE_TLS_CA` (agent) to enable TLS.
  Dev certs: `./scripts/gen-certs.sh` (openssl, 7-day, `certs/` gitignored).
  Production: per-device enrollment (EST/SCEP/SPIRE or cloud IoT PKI),
  short-lived certs, HSM/TPM-backed keys, rotation + revocation.
- Updates: SHA-256 + dev HMAC signature in this reference; production must
  use Sigstore/cosign or TUF with rollback-version protection.
- Containers: least-privilege (`USER 10001`, no secrets in images).
- `cargo audit` in CI (see `ci.yml`); no committed secrets/keys/certs (see
  `.gitignore`).

## Threat model

| Threat | Mitigation / residual |
|---|---|
| Compromised edge | Least-privilege container, immutable image, no cloud creds on device; attacker gets only that LAN's traffic until cert revoked |
| MITM cloud link | mTLS + pinned CA; plaintext demo mode is explicitly non-production |
| Stolen device cert | Short-lived certs + revocation + per-device identity limits blast radius |
| Malicious update | Signature verify before stage; rollback on health failure; TUF in prod |
| Replay / duplicate | Monotonic `seq`; stale/duplicate → `Noop` |
| Rollback attack | Persisted `applied_seq` + version monotonicity; prod adds TUF snapshot version |
| Control-plane compromise | Edge keeps serving last-known-good; damage limited to future desired pushes until keys rotated |

Never commit `certs/`, `*.key`, `*.pem`, `.env`, or `target/`.
