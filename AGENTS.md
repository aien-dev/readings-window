# readings-window (read-only HTTPS window onto the Spark's GPU and CPU readings)

> A small Rust service that runs on the project's Raspberry Pi house server. `readings-window collect` fetches one JSON line of readings from the DGX Spark over ssh and keeps 30 days of history; `readings-window serve` hands the readings, cached status and results files, the exposure-watch result and the agent chat feeds to anyone holding the bearer token, over HTTPS on port 9443. `scripts/` holds the exposure watch, a shell script and systemd timer that scans the house's own public IP. There is no CI in this repository.

Shared rules: read https://www.aienos.com/agents/RULES.md first. This file adds what is specific to this repository; where the two differ, this file wins here.

## Scope

- Applies to the whole repository: the crate (`src/main.rs`, `src/tls.rs`) and the exposure watch (`scripts/exposure-watch.sh`, `.service`, `.timer`).
- The service is read-only by design. It serves files and runs fixed read commands on the Spark; it never writes to the Spark and never accepts uploads. Keep it that way.
- Not in this repository: the Spark-side `spark-readings` script, the `readings-collect` timer and `readings-window` service units on the Pi, the token, the TLS key and certificate, and the exposure baseline. `NOTES.md` describes where they live.
- `target/`, `.crumb.local` and `spark.conf` are ignored (`.gitignore`). Never commit them or a real deployment target list.

## Installation

```bash
git clone https://github.com/aien-dev/readings-window.git
cd readings-window
cargo build --release
```

Needs a stable Rust toolchain only. Dependencies are `rustls` (ring provider, TLS 1.2 and 1.3) and `serde_json`; `rcgen` is a dev-dependency for the TLS tests. No Python. The binary is `target/release/readings-window`.

## Configuration

Spark SSH targets come from `/etc/readings-window/spark.conf`; `spark.conf.example` documents the format. Each nonempty `target=USER@HOST` value is tried in file order. Keep real targets local-only, with the same order when migrating an existing deployment. There are no environment-variable or flag overrides.
Optional `identity=PATH` selects an SSH identity on the hub for all targets; the first nonempty value wins. Omission delegates identity selection to the service user's OpenSSH configuration/defaults. Values are trimmed, with no shell expansion or quote removal. Preserve an existing deployment's identity explicitly before deploying this change; keep its real path local-only.

- Pi-side files: token `/etc/readings-window/token`, certificate `/etc/readings-window/fullchain.pem`, key `/etc/readings-window/key.pem`; data under `/var/lib/readings-window/` (`history.jsonl`, `status.json`, `results.json`, `exposure.json`).
- Listen address `0.0.0.0:9443` (`serve`).
- The target file is read on each fetch, not at server startup. Missing or unreadable config makes Spark fetches fail; `/health` can still return successfully because it reads local history. See the migration procedure in `NOTES.md`.
- Other settings remain constants in `src/main.rs`: home-relative Spark status/results and agent archive paths (`AGENTS`, five fixed names). The configured SSH user's home must contain those paths. Changing one of these is a deployment change; say so in the pull request.
- Limits: history `?hours=` default 24, max 720 (`DEFAULT_HOURS`, `MAX_HOURS`); chat `?lines=` default 500, max 50000; HTTP head cap 16 KiB and at most 64 connections (`src/tls.rs`); history kept 30 days (`KEEP_SECS`).
- The exposure watch reads its baseline from `/etc/exposure-watch/baseline.json` (created on first run) and writes `/var/lib/readings-window/exposure.json`, which `/v1/exposure` serves.

## Usage

```bash
# Unit tests (20 tests: request parsing, routes, auth, TLS handshake with a generated cert)
cargo test

# Lint (clean at the time of writing)
cargo clippy --all-targets

# Syntax check of the exposure watch
bash -n scripts/exposure-watch.sh
```

Running the binary with no argument prints `usage: readings-window collect|serve` and exits 1. Do not run `collect` or `serve` on a development machine: `collect` needs root, the Pi's ssh key and the Spark on the network; `serve` needs the token, key and certificate under `/etc/readings-window/` and binds port 9443. Do not run `scripts/exposure-watch.sh` either: it scans the public IP with `nmap`, queries UPnP and writes under `/etc` and `/var/lib`. All three only make sense on the Pi.

Endpoints are listed in `README.md`: `/health` needs no token; every `/v1/*` route needs `Authorization: Bearer <token>`. Wrong or missing token gives 401, unknown path 404, non-GET 405.

## Conventions

- Keep the dependency set minimal. The hand-written HTTP/1.1 reader in `src/tls.rs` replaced `tiny_http` because it pinned a rustls with a security advisory; do not bring back a framework without a reason in the pull request.
- Nothing user-supplied reaches the remote shell. Agent names are matched exactly against the allow-list and mapped to constant paths; `?lines=` is parsed to an integer and clamped. Keep both properties, and add a test when you touch a route.
- Tests live in `#[cfg(test)]` modules at the bottom of `src/main.rs` and `src/tls.rs`.
- `cargo fmt --check` does not pass on the current tree. Do not reformat files you are not otherwise changing; a formatting-only change belongs in its own pull request.
- Dependency updates for advisories are separate commits that name the advisory (for example `RUSTSEC-2026-0382`).
- Record operational changes (new route, new file, new unit) in `NOTES.md` and the endpoint list in `README.md`.

## Safety and approvals

- Never commit the token, TLS key, certificate, ssh keys or the exposure baseline. They exist only on the Pi.
- Never widen what the service exposes (a new route, a new remote command, a write path, auth removed from a route) without explicit approval from the maintainer.
- Ask before changing the listen port, the host list or any path constant; the live Pi deployment depends on them.
- The exposure watch must scan only the house's own public IP. Never point it at another host.
- Deploying to the Pi is out of scope for an agent working from a clone. Merge through a pull request; the maintainer deploys.

## Further reading

- `README.md`: endpoints, commands and current status.
- `NOTES.md`: deployment layout on the Pi, unit names, feed history and known unverified points.
- `src/tls.rs`: the TLS listener and HTTP reader, with its limits.
