# readings-window

A read-only, token-protected HTTPS window onto the Spark's GPU and CPU readings, served from the Pi "hub".
A small Rust service: it collects one JSON line of readings from the Spark every 15 seconds over ssh, keeps 30 days of history, and serves it to anyone holding the bearer token.

## Endpoints
- `GET /health`: liveness plus an `exposure` field (`ok|changed|unknown`).
- `GET /v1/readings/current`, `GET /v1/readings/history?hours=N` (default 24, max 720).
- `GET /v1/status`, `GET /v1/results`, `GET /v1/exposure`.
- All `/v1/*` routes need `Authorization: Bearer <token>`. The token and the TLS key live on the hub only (`/etc/readings-window/`), never in this repository.

## Commands
- `readings-window collect`: fetch one reading from the Spark and append it to the history file.
- `readings-window serve`: HTTPS server on port 9443.

`scripts/` holds the exposure watch: a 15-minute timer that scans our own public IP from inside the LAN (the router hairpin view, which is not the same as the internet view) and flags any change against an accepted baseline in `/etc/exposure-watch/baseline.json`.
Operational notes, unit names and the deployment layout are in `NOTES.md`.

Build: `cargo build --release`. No Python. License: AGPL-3.0-or-later (see LICENSE).
