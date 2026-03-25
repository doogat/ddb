# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Use [GitHub Security Advisories](https://github.com/doogat/ddb/security/advisories/new) to report vulnerabilities privately. If you cannot use GitHub, email tomas@buvis.net with "DDB Security" in the subject.

Include:

- Description of the vulnerability
- Steps to reproduce
- Affected version(s)
- Impact assessment if known

## Response Timeline

- **Acknowledge**: within 72 hours
- **Triage**: within 1 week
- **Fix**: within 30 days for critical issues, best-effort for others

This is a solo-developer project. Timelines are best-effort commitments.

## Disclosure Policy

Coordinated disclosure: vulnerabilities are kept private until a fix is released. Credit is given to reporters in the release notes unless they prefer anonymity.

## Scope

The following are considered security issues:

- Authentication or authorization bypass
- Path traversal or directory escape
- Data corruption or unauthorized data access
- Remote code execution
- Denial of service via crafted input

Out of scope:

- Issues requiring physical access to the device
- Social engineering
- Vulnerabilities in dependencies (report upstream; see our `cargo deny` policy)
