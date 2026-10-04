# readings-window

Read-only, token-protected HTTPS window onto the Spark's GPU/CPU readings, hosted on the Pi "hub".

## What exists
- Spark: `/usr/local/bin/spark-readings` (bash, one JSON line, ~0.05 s).
- Hub: `/usr/local/bin/readings-window` (this crate). Runs as root (needs the key and the root ssh key).
  - `readings-collect.timer` (every 15 s) runs `readings-window collect`: ssh to the Spark
    (LAN 192.168.1.108 first, private net 100.64.0.2 as fallback; the private-net address timed out when tested),
    appends to `/var/lib/readings-window/history.jsonl`, prunes lines older than 30 days.
  - `readings-window.service` runs `readings-window serve` on 0.0.0.0:9443 (HTTPS).
- Secrets: `/etc/readings-window/token` (root 600), `/etc/readings-window/key.pem`.
- Cert: readings.aienos.com via acme.sh (Let's Encrypt, DNS-01 through Gandi), RSA 2048, auto-renews, reloadcmd restarts the service.
- DNS: `gandi-ddns.sh` keeps both `hs` and `readings` A records at the home public IP.
- Registered in pi-register as `readings-window` and `readings-collect`.

## How Muse calls it
- Needs router forward of 9443/tcp to 192.168.1.192 (Drake).
- `GET https://readings.aienos.com:9443/health` (no auth): `{"status":"ok","last_reading_age_s":N}`
- `GET /v1/readings/current`, `GET /v1/readings/history?hours=N` (default 24, max 720), header `Authorization: Bearer <token>`.

    curl -sS -H "Authorization: Bearer $TOKEN" https://readings.aienos.com:9443/v1/readings/current

Fields: ts, gpu_util_pct, gpu_temp_c, gpu_power_w, load1, load5, load15, cpu_temp_c, mem_used_mb, mem_total_mb.
Wrong/missing token 401; unknown path 404; non-GET 405.
Note: cpu_temp_c is max over all thermal zones (the Spark's zones are all type "acpitz", none say "cpu").
tiny_http 0.12 pulls rustls 0.20 (older); acceptable for read-only data, UNVERIFIED against heavy scanning.

## Status and results feeds (added 2026-10-04)
- Spark files (written by the orchestrator, see ~/status/README.md): `~/status/current.json`, `~/status/results.json`.
- `collect` also runs `ssh ... cat <file>` for each, validates the JSON, and caches to `/var/lib/readings-window/{status,results}.json` (atomic rename). Bad or missing file: old cache kept.
- `GET /v1/status` and `GET /v1/results` (Bearer token) serve the cache; defaults `{"ts":0,"label":"unknown"}` and `{"updated":0,"results":[]}` if none. History format unchanged. No new unit or timer, so pi-register entries are unchanged.
