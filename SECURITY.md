# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Reporting a Vulnerability

If you discover a security vulnerability in ForkTTY, please report it responsibly:

1. **Do NOT open a public GitHub issue** for security vulnerabilities
2. Email the maintainer directly or use [GitHub's private vulnerability reporting](https://github.com/Lucenx9/forktty/security/advisories/new)
3. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if any)

We aim to acknowledge reports within 48 hours and provide a fix within 7 days for critical issues.

## Security Model

ForkTTY is a **local desktop application**. Its threat model assumes:

- The user running the app is trusted
- The local filesystem is not compromised
- The Unix socket API is restricted to the current user (permissions `0600`)

### Security Boundaries

| Component | Protection |
|-----------|-----------|
| **Unix socket** | Owner-only permissions (`0600`), `XDG_RUNTIME_DIR` default, 1 MiB request size limit, 100 max pending requests |
| **PTY spawn** | Shell path must be absolute and exist; CWD validated as absolute and existing |
| **Worktree operations** | Name validation rejects `/`, `\`, `..`, `\0`; all paths canonicalized and verified within git working directory |
| **Hook execution** | Only `.forktty/setup` and `.forktty/teardown` allowed; paths canonicalized; argv splitting (no `sh -c`) |
| **Notification command** | Must be absolute path; argv splitting only; title/body passed as env vars, not arguments |
| **CSP** | `default-src 'self'; script-src 'self'` — no `unsafe-eval`, no remote scripts |
| **Ghostty theme parser** | Theme names validated against alphanumeric allowlist (no path traversal) |

## Dependencies

- Rust dependencies are audited with `cargo audit` in CI
- npm dependencies are audited with `npm audit` in CI
- Dependabot vulnerability alerts are enabled on the repository
