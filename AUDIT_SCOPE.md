# Audit scope

Written for an outside, independent security audit. Revision described:
`c6e2ab34598303f2d9f72bf1194e19074bdf51a6` on `main`, 2026-10-26. Everything below is taken from the repository's own files at
that revision; items marked NOT_RUN were not executed while writing this document.

## What this repository is

A small Rust service, plus a shell exposure watch, that runs on the project's house server.
`readings-window collect` fetches one JSON line of GPU and CPU readings from the development
machine over ssh and keeps 30 days of history. `readings-window serve` listens on `0.0.0.0:9443`
with TLS and serves `/health` without a token and every `/v1/*` route to holders of a bearer
token. Code: `src/main.rs` (561 lines) and `src/tls.rs` (380 lines, the TLS listener and a
hand-written HTTP/1.1 head reader that replaced `tiny_http`). `scripts/exposure-watch.sh` (98
lines) with a systemd `.service` and `.timer`. `spark.conf.example` documents the ssh target file.
Licence AGPL-3.0-or-later.

## What to review, in priority order

1. Authentication (`auth_ok`, `ct_eq`, `handle` in `src/main.rs`). `Authorization: Bearer <token>`
   is trimmed and compared with a fold over XOR after a length check. `serve` refuses to start with
   a token shorter than 32 characters. Check every `/v1/*` route sits behind the check and that
   `/health` reveals nothing sensitive (it returns `status`, `last_reading_age_s`, `exposure`).
2. Remote command construction (`chat_route`, `chat_command`, `clamp_lines`, `fetch`,
   `spark_ssh`). The remote command string is built from constant paths in the `AGENTS` table and
   a clamped integer; `?all=1` switches to `cat` of a constant path or glob. ssh runs with
   `BatchMode=yes`, `ConnectTimeout=5`, optional `-i <identity>`, then the target, then the
   command. Targets and identity come from the root-owned `/etc/readings-window/spark.conf`, read
   on each fetch, with no shell expansion. Check that no request byte reaches the remote command,
   and the effect of a config value beginning with `-`.
3. HTTP and TLS (`src/tls.rs`): `parse_head`, the head deadline reader, `MAX_HEAD` 16 KiB,
   `MAX_CONNS` 64, one thread per connection, read and write timeouts. NOTES.md records that the
   handshake and header deadlines do not bound ssh execution or total response time, and that
   `?all=1` on a large file is a big response (UNVERIFIED under load).
4. Response validation of fetched data (`validate`, `validate_status`, `validate_results`,
   `validate_exposure`) before it is cached and served.
5. History handling (`collect`, `ts_of`, `clamp_hours`: default 24, max 720 hours; `KEEP_SECS`
   30 days) and file writes under `/var/lib/readings-window/`.
6. `scripts/exposure-watch.sh`: gets the public IP from an external echo service (with a DNS
   fallback), runs `nmap` TCP and UDP scans against that IP only, lists UPnP mappings with `upnpc`,
   checks that named DNS records resolve to the public IP (`EXPOSURE_DNS_HOSTS` overrides the
   list), and builds the JSON result with `printf`. Check quoting of values interpolated into that
   JSON and that the scan target cannot be redirected.

## Trust boundaries

- Untrusted: every HTTPS client and request byte; unauthenticated callers reach `/health` and the
  401, 404 and 405 paths.
- Trusted: the token, key and certificate files under `/etc/readings-window/`, the ssh target file
  and identity, and the remote machine's output after validation.
- The service runs as root on the server (README); the remote ssh account runs the fixed read
  commands.
- The exposure watch trusts the external IP echo service and DNS for the IP it scans.

## Threat model summary

- Assets: the bearer token; confidentiality of the chat feeds and readings; the remote machine
  (only read commands reach it).
- Attackers considered: unauthenticated network clients; slow or malformed TLS and HTTP clients
  (tests cover fragmented handshakes and incomplete records against the head deadline); a token
  holder trying to inject into the remote command.
- Defended: token check on `/v1/*`, allow-listed agent names, clamped integers, head size and
  connection caps, deadlines, rustls only.
- Not defended / explicitly out of scope: a token holder can request full chat histories (by
  design); concurrent chat requests can start concurrent ssh processes (NOTES.md); the exposure
  watch sees the LAN hairpin view, not the internet view (README). No rate limiting per client is
  described in the repository.

## Build from source at a pin

Needs a stable Rust toolchain (version not pinned). `Cargo.lock` committed (68 packages).

```sh
cargo build --release
```

Dependencies: `rustls` 0.23.46 (no default features; `ring`, `std`, `tls12`), `serde_json` 1;
dev-dependency `rcgen` 0.14 for the TLS tests.

## Run every test

NOT_RUN while writing this document. From AGENTS.md:

```sh
cargo test                       # unit tests in src/main.rs and src/tls.rs
cargo clippy --all-targets       # lint
bash -n scripts/exposure-watch.sh
```

AGENTS.md says 20 tests; the tree at this revision has 26 `#[test]` attributes (18 in
`src/main.rs`, 8 in `src/tls.rs`), so expect `test result: ok` with the count cargo prints.
`cargo fmt --check` does not pass on the current tree (AGENTS.md).

Do not run `collect`, `serve` or `scripts/exposure-watch.sh` on a development machine: `collect`
needs root, the server's ssh key and the remote machine; `serve` needs the token, key and
certificate and binds port 9443; the exposure watch scans a public IP and writes under `/etc` and
`/var/lib`. NOT_RUN.

## Known limitations and NOT_RUN

- No CI.
- Behaviour under load UNVERIFIED (NOTES.md).
- Exposure watch findings marked UNVERIFIED in NOTES.md (hairpin vantage; one UDP port reported
  `open|filtered`).
- `/health` does not prove the remote fetch works (README).
- Deployed binary, units, token and baseline are outside the repository and not checkable here.

## Dependencies and provenance

- `Cargo.lock` present; crates fetched from crates.io.
- AGENTS.md: advisory-driven dependency updates are separate commits naming the advisory (for
  example `RUSTSEC-2026-0382`); `tiny_http` was removed because it pinned a rustls with an
  advisory.
- Run-time host tools: `ssh`; for the exposure watch `curl`, `dig`, `nmap`, `upnpc`, `timeout`.
- No GitHub Actions.

## Evidence

No receipts are stored in this repository. `NOTES.md` records deployment layout, feed history and
unverified points.
