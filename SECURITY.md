# Security Policy

Report vulnerabilities privately to the repository maintainers. Do not open
public issues for unpatched security bugs.

- Supported: latest `main`.
- mTLS is required in production; plaintext demo mode is not supported for
  internet-facing deployments.
- OTA artifacts carry SHA-256 + Ed25519 release signatures with journaled
  rollback protection; multi-signer quorum (Sigstore/TUF) is the path for
  stricter supply-chain requirements.
- See `docs/security.md` for the threat model.
