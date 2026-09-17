# Contributing

Production-oriented reference implementation: every PR must keep
`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
and `cargo test --workspace` green.

- No `unwrap()`/`expect()` in production paths; `anyhow` only at binaries.
- No unbounded channels, no global mutable state, no blocking in async tasks.
- Document tradeoffs in `docs/adr/` for architectural changes.
- Add or update tests for dataplane, reconciliation, OTA, and rollout logic.
- Never commit secrets, certs, keys, `.env`, PDFs, or `target/`.
