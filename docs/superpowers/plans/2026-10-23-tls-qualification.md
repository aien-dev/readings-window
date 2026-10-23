# Readings TLS qualification implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans inline. One final independent whole-branch review.

**Goal:** Finish the missing acceptance proof for existing readings-window#7 PR#9 without duplicating its TLS implementation.
**Architecture:** Reuse PR9 rustls0.23 transport. Pass an immutable history-file path through a private accept-loop helper; the public serve_on wrapper still binds the existing HISTORY constant. Route/auth logic reads that path, enabling a real temporary-file reading through the real TLS server in tests. No new config/env/CLI endpoint or crypto implementation.
**Tech Stack:** Existing Rust, rustls0.23.46/ring and existing dev-only rcgen; no added dependencies.
**Spec:** https://github.com/aien-dev/readings-window/issues/7; existing PR9 exact source3197d3b2851a60008302b5b5c9c0d82e8ae93cbb supplies the implementation.

## Global constraints
- No live hub/Spark service, credential, certificate or /var/lib data changes; own Mac scratch checkout and loopback ephemeral test ports only.
- Production token/cert/key/history paths and TLS1.2/1.3 policy remain unchanged. Never add network crypto dependencies or hand-roll TLS.
- Fixture readings are explicitly synthetic, not hardware measurements; actual TLS/HTTP/file I/O must be real.
- Reuse original PR9 rather than competing implementation. Preserve current main README changes when integrating source; existing desk remains sole PR writer.
- Old rustls lockfile refusal is dependency-floor RED evidence, not a claimed reproduction of the remote infinite-loop advisory.
- All test/format/clippy failures are evidence to resolve, not skips or blanket pre-existing waivers.

## Review focus
- Real /v1/readings/current must return the exact fixture only with a valid token, under both TLS1.2 and1.3; missing/wrong token401, read-only behavior preserved.
- Legacy TLS refusal must distinguish an alert/closed connection from a timeout. A wedged server is not successful refusal.
- Header terminator just beyond16KiB must not bypass the stated bound; short header plus an over-read body must remain accepted safely. An incomplete TLS record with periodic TCP activity must still hit the existing15s total header budget inside rustls I/O.
- Private history injection must never be selectable through request input, env or a new public option; default paths remain literal constants.
- The complete dependency graph must contain no rustls<0.23.46, and source delivery must not imply deployment or original issue closure.

### Task 1: Close the real-reading and discriminating-refusal proof gaps
**Files:** modify src/main.rs and src/tls.rs only for private history injection and proven bounded-reader defects; create evidence/TLS-QUALIFICATION-L645/; retain current main README.
**Interfaces:** serve_on(TcpListener,Arc<ServerConfig>,String) unchanged; private serve_on_with_history(...,String) carries trusted setup path; handle gains internal history:&str; tests call same real routing/auth through TLS.
- [ ] Test actual HTTPS reading against temporary history first using existing server; expected RED503 because fixed production path cannot see the isolated fixture.
- [ ] Implement private immutable history path seam; expected reading200 and exact JSON, missing/wrong token401 over TLS1.2 and1.3, GET-only behavior intact.
- [ ] Extract legacy refusal classifier in test utilities without changing behavior; new timeout-classification test must RED before rejecting TimedOut/WouldBlock, then legacy network probes must pass with actual alert/close.
- [ ] Test 16KiB terminator overflow and short-header/body case; if boundary RED, make the minimal check before truncating the head and retain all cases.
- [ ] Reproduce fragmented-TLS deadline bypass over real TCP: periodic bytes prevent10s I/O timeout but must not bypass15s total header budget. Watch RED client19s timeout, then enforce remaining budget at TCP Read/Write boundaries while retaining response I/O timeout after header completion.
- [ ] Run full cargo test --locked, cargo fmt --all -- --check, cargo clippy --locked --all-targets -- -D warnings, release build and exact lock-floor check. Preserve baseline format failure; format without semantic changes separately.
- [ ] Publish actual commands/raw results/RED+GREEN, dependency source pins and synthetic-data labels. Independent review and one RED/GREEN fix pass; exact-source successor handoff to sole Renda, no second PR/deployment or automatic issue close.
