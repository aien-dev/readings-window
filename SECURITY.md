# Security policy

## Reporting a vulnerability

Report privately. Do not open a public issue, pull request or discussion about it.

1. GitHub: open this repository's Security tab and choose "Report a vulnerability". This
   creates a private advisory that only the maintainers can see. If that option is not shown,
   private vulnerability reporting has not been enabled for this repository yet; use email.
2. Email: drake@aienos.com with the subject `[readings-window] security report`.

Include the commit or release you tested, the component (path), steps to reproduce, and what an
attacker gains. If the report involves a secret, say where it was exposed and do not send the
secret itself. Encrypted mail is not required; do not post proof-of-concept payloads publicly.

## Response targets

This project has one maintainer; the targets below are best effort and stated honestly.

- Acknowledgement: within 3 working days.
- Triage and severity: within 7 days of acknowledgement.
- Fix or mitigation for confirmed issues: aim for 30 days for high and critical, 90 days
  otherwise, with a status update at least every 14 days until closed.
- Disclosure: coordinated. An advisory is published with credit (unless you ask otherwise)
  once the fix is on the default branch. Default embargo 90 days, negotiable.

## Scope

`readings-window` is a Rust service: `collect` fetches readings from a development machine over
ssh and keeps 30 days of history; `serve` hands readings, cached status and results files, the
exposure-watch result and agent chat feeds to bearer-token holders over HTTPS on port 9443.
`scripts/exposure-watch.sh` scans the house's own public IP.

In scope:

- Any `/v1/*` response without a valid bearer token, or a token comparison that leaks more than
  the token length.
- Anything user-supplied (path, query string, headers) reaching the remote shell run over ssh.
  AGENTS.md states that agent names are matched against a fixed allow-list and `?lines=` is parsed
  to an integer and clamped.
- A write to the remote machine or an upload path; AGENTS.md says the service is read-only by
  design.
- Resource exhaustion that bypasses the limits in `src/tls.rs` (16 KiB request head, 64
  connections, handshake and header deadline).
- TLS configuration in `src/tls.rs` (rustls, ring provider, TLS 1.2 and 1.3).
- `scripts/exposure-watch.sh` scanning anything other than the house's own public IP.

Out of scope:

- The remote-side readings script, the systemd units on the server, the token, TLS key and
  certificate, and the exposure baseline: none of them are in this repository.
- rustls, ring, OpenSSH, nmap, upnpc and the host operating system.

## Supported versions

Only the default branch `main`; there is no release (Cargo version `0.1.0`). Reports against older commits are welcome but fixes land on the default branch only.

## Status

README (2026-10-18): running. CI (`.github/workflows/ci.yml`, added by the audit-readiness
change) runs `cargo test --locked`. No certification, no bug bounty. See AUDIT_SCOPE.md for what
to review and how to build and test it.
