# Security Policy

## Supported Versions

A2A Warden is in early development. Security updates will be provided for the latest release on the `main` branch.

| Version | Supported          |
| ------- | ------------------ |
| main    | :white_check_mark: |
| < 0.1.0 | :x:                |

## Reporting a Vulnerability

**Please do not create public issues for security vulnerabilities.**

Report security vulnerabilities privately to: **security@a2a-warden.io**

Include the following information:

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Any suggested mitigations

We aim to acknowledge receipt within 48 hours and provide a preliminary assessment within 7 days.

## Security Considerations

### Data Protection

A2A Warden records all inter-agent communication for audit purposes. This includes:

- Task requests and responses
- Message content (including artifacts)
- Human approval decisions
- Agent Card information

**Operators must ensure:**

- Database encryption at rest (PostgreSQL TDE, SQLite SEE, or filesystem encryption)
- TLS for all network connections
- Access controls on the administration API
- Regular audit log review

### Network Security

- The warden acts as a man-in-the-middle for A2A traffic
- All external connections should use mTLS
- The embedded web console must not be exposed publicly without authentication
- Push notification destinations are validated against an allow-list (anti-SSRF)

### Cryptography

- Signed Agent Card verification uses JWS (RFC 7515) over JCS (RFC 8785)
- Supported algorithms: ES256/ES384/ES512, EdDSA, RS256
- JWKS are pinned by configuration
- Keys are never logged

### Authentication & Authorization

- Administration API uses static token for legacy auth (to be replaced with OIDC)
- Policy engine (Cedar) enforces deny-by-default
- Multi-tenancy isolation in Phase 6

## Dependency Security

- `cargo audit` runs in CI on every push
- Dependabot monitors for vulnerable dependencies
- Minimal dependency policy - each crate addition requires justification

## Secure Deployment

### Minimum Hardening Checklist

- [ ] Run behind a reverse proxy with TLS termination
- [ ] Restrict administration API to internal network or VPN
- [ ] Enable database encryption
- [ ] Configure audit log retention and purging
- [ ] Set up monitoring and alerting
- [ ] Regular security updates
- [ ] Review and approve all Agent Cards before registration

### Docker Deployment

```dockerfile
# Use the official from-scratch image
FROM ghcr.io/a2a-warden/a2a-warden:latest

# Run as non-root user
USER 1000:1000

# Read-only root filesystem
# (requires writable /data for SQLite)
```

## Disclosure Policy

When a security vulnerability is confirmed:

1. A fix is developed privately
2. A patch release is prepared
3. The vulnerability is disclosed publicly with the release
4. Credit is given to the reporter (unless anonymity is requested)

## Contact

- Security email: security@a2a-warden.io
- PGP key: Available on keys.openpgp.org for security@a2a-warden.io