# Security Policy

Report vulnerabilities privately to the repository maintainers. Do not open
public issues for unpatched security bugs.

- Supported: latest `main`.
- mTLS is required in production; plaintext demo mode is not supported for
  internet-facing deployments.
- Dev HMAC update signatures are reference-only; production must use
  Sigstore/cosign or TUF.
- See `docs/security.md` for the threat model.
