# readings-window

A read-only, token-protected HTTPS window onto the Spark's GPU and CPU readings, served from the project's Raspberry Pi house server.

Supports [Aienix](https://github.com/aien-dev/aienix), the core tree of AIEN. It lets the people and agents working on AIEN see how the development machine is doing without logging in to it.

**Status (2026-10-18): running.** The health endpoint answered with status ok when checked on 2026-10-17. The code was last changed on 2026-10-04 (the chat feed routes); the last change of any kind was a README correction on 2026-10-13.

A small Rust service. `collect` fetches one JSON line of readings from the Spark over ssh (a timer on the Pi runs it every 15 seconds, see `NOTES.md`; the timer is not in this repository) and keeps 30 days of history. `serve` hands it out to anyone holding the bearer token.

## Endpoints
- `GET /health`: no token needed. Returns `status`, `last_reading_age_s` and an `exposure` field (`ok|changed|unknown`).
- `GET /v1/readings/current`, `GET /v1/readings/history?hours=N` (default 24, max 720).
- `GET /v1/status`, `GET /v1/results`, `GET /v1/exposure`.
- `GET /v1/chat` returns `{"agents":["claude","codex","opencode","gemini","muse"]}` so a client can discover the feeds.
- `GET /v1/chat/<agent>` (plain text) and `GET /v1/chat/<agent>.jsonl` (common JSON records, `application/x-ndjson`) for exactly those five agents; any other name is 404. `?lines=N` gives the last N lines (default 500, max 50000) of the rolling window of the agent archive. `?all=1` gives the whole rolling window (plain) or the complete record history, all dates in order (jsonl). Proxied live from the Spark on every request (nothing is written to the Pi's disk). 502 `{"error":"spark unreachable"}` if the Spark cannot be reached or the remote command fails (for example the archive file is missing).
- All `/v1/*` routes need `Authorization: Bearer <token>`. The token and the TLS key live on the Pi only (`/etc/readings-window/`), never in this repository.

## Commands
- `readings-window collect`: fetch one reading from the Spark, append it to the history file and drop entries older than 30 days. It also refreshes the cached copies of the Spark's status and results files that `/v1/status` and `/v1/results` serve.
- `readings-window serve`: HTTPS server on port 9443.

Spark SSH targets are local configuration, not source constants. Prepare `/etc/readings-window/spark.conf` using `spark.conf.example` and the migration procedure in `NOTES.md` before deploying a build that uses it. Keep real targets private. The current services run as root and use a root-owned mode-600 file. `/health` only reads local state and does not prove Spark fetch readiness.
The optional `identity=PATH` setting selects the hub-side SSH key for all targets (first nonempty value wins); without it OpenSSH uses the service user's SSH configuration/default identities. Preserve the previously used identity in private config before upgrading. Paths with spaces are literal values, not shell-quoted strings.

`scripts/` holds the exposure watch: a 15-minute timer that scans our own public IP from inside the LAN (the router hairpin view, which is not the same as the internet view) and flags any change against an accepted baseline in `/etc/exposure-watch/baseline.json`. It also flags UPnP being on and the `hs` and `readings` DNS records not matching the public IP.
Operational notes, unit names and the deployment layout are in `NOTES.md`.

Build: `cargo build --release`. No Python. License: AGPL-3.0-or-later (see LICENSE).
