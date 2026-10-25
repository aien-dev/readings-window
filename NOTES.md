# readings-window

Read-only, token-protected HTTPS window onto the Spark's GPU/CPU readings, hosted on the Pi "hub".

## What exists
- Spark: `/usr/local/bin/spark-readings` (bash, one JSON line, ~0.05 s).
- Hub: `/usr/local/bin/readings-window` (this crate). Runs as root (needs the key and the root ssh key).
  - `readings-collect.timer` (every 15 s) runs `readings-window collect`: ssh to the Spark
    (targets from `/etc/readings-window/spark.conf`, local-only, format in `spark.conf.example`: LAN first, private net as fallback),
    appends to `/var/lib/readings-window/history.jsonl`, prunes lines older than 30 days.
  - `readings-window.service` runs `readings-window serve` on 0.0.0.0:9443 (HTTPS).
- Secrets: `/etc/readings-window/token` (root 600), `/etc/readings-window/key.pem`.
- Cert: readings.aienos.com via acme.sh (Let's Encrypt, DNS-01 through Gandi), RSA 2048, auto-renews, reloadcmd restarts the service.
- DNS: `gandi-ddns.sh` keeps both `hs` and `readings` A records at the home public IP.
- Registered in pi-register as `readings-window` and `readings-collect`.

## Spark target configuration migration

The target configuration introduced in PR #12 replaces embedded SSH targets. Before deploying that binary, recover the existing deployment's exact targets and fallback order into a private `spark.conf`. Do not copy the example's placeholders into production or publish recovered values in a PR, command output or logs. `spark.conf` is gitignored; keep backups outside the checkout.

On the deployment host:

1. Inspect both service identities with `systemctl show readings-window.service readings-collect.service -p Id -p User -p Group -p DynamicUser`. Empty `User=` with `DynamicUser=no` means root; confirm running processes with `ps -C readings-window -o user=,group=,args=`.
2. Check whether `/etc/readings-window/spark.conf` already exists, including a dangling symlink. Never overwrite an existing file as part of migration. If absent, stage the recovered bytes in a root-owned temporary file in `/etc/readings-window/`, set mode 600, then publish with `ln -T TEMP /etc/readings-window/spark.conf`. Hard-link creation refuses an existing destination; remove only the temporary name afterward.
3. Root-owned mode 600 is correct for the current root services. If a deployment uses a non-root identity, stop and establish an explicitly approved ownership/access policy first. That deployment also needs access to its selected SSH identity; making this config readable alone is insufficient.
4. Compare the installed bytes to the private recovered file without printing either. Confirm owner, mode and readability as the actual service user.
5. Check the existing service without collecting or restarting it: `systemctl is-active readings-window.service` and `curl --fail --silent --show-error --resolve readings.aienos.com:9443:127.0.0.1 https://readings.aienos.com:9443/health`.

The config is read on each Spark fetch; installing it does not require a restart. Installing config is not a binary deployment. `/health` checks local history and exposure state, not SSH reachability, config loading or chat retrieval. Remote paths remain relative to the configured remote user's home. Collection and live-chat readiness need separate authorized checks; a health response alone does not establish them.

On 2026-10-25, the missing config was restored from the original two targets in their original order. Both service units used root (`User=` empty, `DynamicUser=no`), and both running processes were root. The file was published without overwriting via a hard link, with owner/group root and mode 600; a byte comparison against the private recovered file matched. No binary was deployed and no service was restarted.
The read-only post-install smoke returned `active` from `systemctl is-active readings-window.service` and `{"status":"ok","last_reading_age_s":13,"exposure":"ok"}` from the local HTTPS `/health` request with certificate validation. No collection command, chat fetch or model request was made by this check.

### SSH identity migration

The source no longer embeds a deployment-specific SSH key pathname. An optional `identity=PATH` line applies to every target; the first nonempty value is passed as one `ssh -i` argument. Leading/trailing whitespace is trimmed, but internal spaces are preserved and values are not shell-expanded or unquoted. With no nonempty identity, OpenSSH chooses from the service user's SSH configuration/default identities. Target ordering, batch mode and the five-second connect timeout are unchanged.

Before deploying this change, recover the previously selected identity path privately. For an existing config, preserve its exact target bytes, stage a mode-600 root-owned replacement with the explicit identity, verify the original has not changed, and atomically replace it. Check that the service user can read the identity and that installed bytes match the private backup without printing either. Do not replace another operator's existing identity choice.

On 2026-10-25, the installed root-owned mode-600 config and private backup were extended with the exact previously embedded identity path. The original target bytes were preserved, installed bytes were compared, owner/group and mode were checked, and the identity was readable as root. No key contents or private values were printed, no Spark fetch was made, no binary was deployed and no service was restarted. The local test smoke substitutes `printf` for SSH; it establishes argument boundaries only, not network authentication.

## How Muse calls it
- Needs router forward of 9443/tcp to the hub's LAN address (Drake).
- `GET https://readings.aienos.com:9443/health` (no auth): `{"status":"ok","last_reading_age_s":N}`
- `GET /v1/readings/current`, `GET /v1/readings/history?hours=N` (default 24, max 720), header `Authorization: Bearer <token>`.

    curl -sS -H "Authorization: Bearer $TOKEN" https://readings.aienos.com:9443/v1/readings/current

Fields: ts, gpu_util_pct, gpu_temp_c, gpu_power_w, load1, load5, load15, cpu_temp_c, mem_used_mb, mem_total_mb.
Wrong/missing token 401; unknown path 404; non-GET 405.
Note: cpu_temp_c is max over all thermal zones (the Spark's zones are all type "acpitz", none say "cpu").
TLS is terminated by rustls 0.23 (ring provider, TLS 1.2 and 1.3 only) in `src/tls.rs`; a minimal HTTP/1.1 reader (one request per connection, 16 KiB head cap, 64 connections max) replaced tiny_http, which pinned rustls 0.20 (RUSTSEC-2024-0336).

## Status and results feeds (added 2026-10-04)
- Spark files (written by the orchestrator, see ~/status/README.md): `~/status/current.json`, `~/status/results.json`.
- `collect` also runs `ssh ... cat <file>` for each, validates the JSON, and caches to `/var/lib/readings-window/{status,results}.json` (atomic rename). Bad or missing file: old cache kept.
- `GET /v1/status` and `GET /v1/results` (Bearer token) serve the cache; defaults `{"ts":0,"label":"unknown"}` and `{"updated":0,"results":[]}` if none. History format unchanged. No new unit or timer, so pi-register entries are unchanged.

## Exposure watch (added 2026-10-04)
- Hub: `/usr/local/bin/exposure-watch.sh`, run by `exposure-watch.timer` (2 min after boot, then every 15 min). Scans only our own public IP: TCP 1-1024 plus a list of common ports (nmap), UDP 3478 and 41641 (nmap; UDP cannot tell open from filtered, so only "open" counts), UPnP via `upnpc -l` (off is the good state), DNS A records of hs and readings vs the public IP.
- Baseline `/etc/exposure-watch/baseline.json` (expected TCP 443, 9443; UDP 3478). Result `/var/lib/readings-window/exposure.json` (status ok|changed, plain-English `changes`). Changed results are appended to `/var/lib/exposure-watch/alerts.log`; short run log in `last-run.txt`.
- `GET /v1/exposure` (Bearer token) serves the file; `/health` gains `"exposure":"ok|changed|unknown"` (unknown if missing, invalid or older than 1 h).
- First run found TCP 53 open on the public IP (DNS answered a recursive query). Source UNVERIFIED: hairpin from inside the LAN may hit the router itself rather than the hub. UDP 3478 not confirmed open by nmap (open|filtered), UNVERIFIED.
- Registered in pi-register as `exposure-watch` (health: last-run.txt younger than 20 min).

## Live Claude chat feed (added 2026-10-04)
- Why: Drake wants the whole Claude chat stream visible to Muse on his MacBook and phone.
- `GET /v1/chat/claude` serves `~/shared/agents/claude-chat.txt`, `GET /v1/chat/claude.jsonl` serves `claude-chat.jsonl` (Bearer token). `?lines=N` (default 500, clamped 1..50000) runs `tail -n N` on the Spark; `?all=1` runs `cat`. Only constant paths and a parsed integer reach the remote shell.
- Proxied live over ssh on each request. NOTHING is written or cached on the hub, because its root disk is a fragile USB stick. Spark unreachable gives 502 `{"error":"spark unreachable"}`.
- Each request blocks the single-threaded server for the ssh round trip (up to ~5 s connect timeout per host); `?all=1` on a large file is a big response. UNVERIFIED under load.

## Per-agent chat routes (added 2026-10-04, replaces the Claude-only feed above)
- Why: Drake 2026-10-04 wants every agent's chat visible to Muse from one archive. Design: `~/handoffs/2026-10-04-agent-archive.md` (program `agent-archive`).
- Allow-list, matched exactly, each name mapped to constant paths in code (the name is never put into a command): `claude`, `codex`, `opencode`, `gemini`, `muse`. `GET /v1/chat` lists them.
- Spark files: `~/archive/agent-chats/latest/<agent>.txt|.jsonl` (rolling, last 20000 lines) and `records/<agent>/<YYYY-MM-DD>.jsonl` (complete history).
- `?lines=N` runs `tail -n N` on the latest file. `?all=1`: plain runs `cat latest/<agent>.txt`; jsonl runs `cat records/<agent>/*.jsonl` (shell glob on the Spark, date order because file names sort by date). A missing file or an empty records folder makes the remote command fail, which shows as 502 "spark unreachable" (same code path as a real outage).
- After the archive cutover `~/shared/agents/claude-chat.*` become symlinks to `latest/claude.*`; the hub no longer reads those paths. The hub must run this build and the archive must exist before `/v1/chat/*` returns data. Not deployed by this change.
