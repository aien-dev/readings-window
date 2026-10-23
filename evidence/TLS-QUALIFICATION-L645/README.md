# readings-window#7: existing TLS PR qualification (L645)

Final qualified source: `ed539838e0881b57aef1e48251d291bf3f1e227f`.
Reused original PR #9: `3197d3b2851a60008302b5b5c9c0d82e8ae93cbb`.
Current-main README was carried from `33cbf275b22f15fbcf420fceb58496c24e99e8ac`.
This is a successor qualification branch for the existing PR, not a competing
TLS implementation. No live service was changed or production key/token read.

## Final actual result

[final-receipts/manifest.json](final-receipts/manifest.json) has `complete=true`:

- **21 passed, 0 failed** in the complete native suite.
- Format check, Clippy with `-D warnings`, and release build passed.
- Real loopback TCP/rustls HTTPS fetched the exact contents of an actual isolated
  temporary history file via `/v1/readings/current`. Both TLS1.2 and TLS1.3 actually
  negotiated. Valid synthetic test credentials: 200; absent/wrong credentials:401.
  The file was unchanged. The reading is **explicitly synthetic**, not hardware
  measurement data; HTTP/TLS/file I/O are real, not mocked responses.
- Actual TLS1.0 and1.1 ClientHellos received alert/close; timeout and WouldBlock
  are no longer counted as successful refusal.
- A terminator beyond the 16KiB head bound is refused. The exact boundary and
  short-header/large-over-read-body cases remain accepted.
- A real incomplete TLS record with periodic TCP bytes cannot evade the existing
  15s total header budget. The previous implementation waited until the client's
  19s guard timed out (RED); the corrected transport closes the connection while
  the peer is still sending (GREEN). This is a timeout correctness control, not
  a throughput benchmark or generic denial-of-service qualification.
- Before/main lock: rustls0.20.9 deliberately refused by the version-floor check;
  candidate has only rustls0.23.46. This is dependency-floor RED evidence, **not**
  a claimed reproduction of the old complete_io advisory exploit.
- Fresh cargo-audit0.22.2 found **RUSTSEC-2026-0382** in original PR's
  rustls-webpki0.103.15. Upgrading that existing transitive crate to0.103.16 gives
  **0 reported vulnerabilities, no ignored advisories**. The final scanner used
  an isolated configuration and a populated registry, without `--no-yanked` or
  stale/ignore exemptions. RustSec Git pin:
  `640bf5a9f06e66d8721e8772766531fe77c8630d`.
- The production HISTORY, TOKEN, CERT and KEY literal paths match the original
  PR exactly. No request/env/CLI can choose the private test history setup path.
  ServerConfig remains ring with TLS1.2/1.3; no crypto was implemented here.

## Why and what changed

The original PR already supplied the rustls0.23 transport replacement; it was
reused, not rebuilt. Its documented choice removed tiny_http, whose listener
interface did not provide the required external TLS stream integration without
an extra proxy. Parent changes fill its missing actual-reading proof with a
private immutable history-path seam, strengthen the legacy negative probe,
fix the proven 16KiB terminator boundary and put the total deadline inside TCP
Read/Write calls reached from rustls. Normal response I/O retains its existing
per-call timeout after the head is complete. No new network-facing crate was
added; the only dependency change beyond original PR9 is the webpki patch.

Baseline source formatting failed; a separate formatting commit fixes it rather
than claiming a pre-existing exemption. All original tests are retained. The
main README update is preserved rather than overwritten with the PR's older copy.

## Reproduce

Python3.11+ is needed only for the read-only TOML checker/receipt tooling; the
server itself does not use Python. This host's default Python3.9 lacks tomllib;
commands used the discovered Python3.12 instead.

```sh
cargo test --locked -- --nocapture
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
python3.12 evidence/TLS-QUALIFICATION-L645/check-lock.py Cargo.lock
```

[qualify.py](qualify.py) runs those checks plus dependency-floor RED/GREEN and a
scanner against a fetched RustSec Git DB. The root README gives installation and
invocation commands. Stdout/stderr are stored separately and hashed; the final
manifest binds source, lockfiles, tool versions, release binary and unchanged
production paths. It refuses to overwrite an output directory.

Historical data is preserved, not relabelled:
- `baseline/`: original17-pass suite, actual reading503 RED, control RED/GREEN,
  header-deadline RED/GREEN, old/new lockfloor and first advisory results.
- `receipts-runner-incomplete/`: the first runner had an isolated empty registry
  and combined scanner streams; yanked-index errors caused JSON parsing failure.
  That run is **not final qualification**. The repaired runner bootstraps the
  locked index, retains both streams and fails on scanner errors.
- `receipts/`: the earlier20-test source snapshot, before the subsequently proven
  fragmented-TLS deadline defect was corrected.
- `final-receipts/`: authoritative21-test corrected-source qualification.

L647 independent whole-candidate security review has been dispatched. At this
receipt publication its findings have not been consumed; no READY/merge approval,
deployment, issue closure or broad security-audit claim is inferred. Renda remains
sole PR writer; the old PR9 head still lacks these fixes and must not be merged
as though this successor's results applied to it.
