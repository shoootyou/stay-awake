# Security Policy

## Supported Versions

Only the latest stable release of Stay Awake receives security updates.

| Version | Supported          |
| ------- | ------------------ |
| 1.x     | :white_check_mark: |
| < 1.0   | :x:                |

## Reporting a Vulnerability

If you discover a security vulnerability in Stay Awake, please report it privately. **Do not open a public GitHub issue.**

**Primary channel:** email [rodolfo@shoootyou.dev](mailto:rodolfo@shoootyou.dev)

**Alternative:** use [GitHub Private Vulnerability Reporting](https://github.com/shoootyou/stay-awake/security/advisories/new).

## What to Include

To help us investigate quickly, please include:

- A clear description of the vulnerability
- Steps to reproduce
- Affected Stay Awake version
- Your macOS version and architecture (Apple Silicon / Intel)
- Potential impact and any suggested mitigation

## Response Expectations

- **Acknowledgement:** within 72 hours of your report
- **Status update:** within 7 days
- **Fix timeline:** depends on severity — critical issues are prioritized for the next patch release

## Scope

Stay Awake is a local-only macOS desktop application. It has no server component, no remote backend, and does not transmit user data. The scope of this policy covers:

- The Stay Awake macOS application binary
- The Tauri shell and Rust backend
- The bundled frontend assets
- The auto-updater channel (latest.json signature validation)

Out of scope: third-party dependencies (please report those upstream), and the user's macOS system configuration.

## Disclosure

We follow coordinated disclosure. Please give us reasonable time to investigate and release a fix before publishing details.
