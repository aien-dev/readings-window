//! readings-window: read-only HTTPS endpoint for Spark readings.
//! `collect` appends one reading to the history file; `serve` publishes it.
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Response, Server, SslConfig};

const HISTORY: &str = "/var/lib/readings-window/history.jsonl";
const TOKEN: &str = "/etc/readings-window/token";
const CERT: &str = "/etc/readings-window/fullchain.pem";
const KEY: &str = "/etc/readings-window/key.pem";
const STATUS: &str = "/var/lib/readings-window/status.json";
const RESULTS: &str = "/var/lib/readings-window/results.json";
const STATUS_DEFAULT: &str = r#"{"ts":0,"label":"unknown"}"#;
const RESULTS_DEFAULT: &str = r#"{"updated":0,"results":[]}"#;
const EXPOSURE: &str = "/var/lib/readings-window/exposure.json";
const EXPOSURE_DEFAULT: &str = r#"{"status":"unknown"}"#;
const EXPOSURE_STALE_SECS: u64 = 3600;
const KEEP_SECS: u64 = 30 * 86400;
const MAX_HOURS: u64 = 720;
const DEFAULT_HOURS: u64 = 24;
const FIELDS: [&str; 10] = [
    "ts", "gpu_util_pct", "gpu_temp_c", "gpu_power_w", "load1", "load5", "load15",
    "cpu_temp_c", "mem_used_mb", "mem_total_mb",
];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Constant-time equality (length leak only).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn auth_ok(header: Option<&str>, token: &str) -> bool {
    match header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => ct_eq(t.trim().as_bytes(), token.as_bytes()),
        None => false,
    }
}

fn clamp_hours(q: Option<&str>) -> u64 {
    let h = q
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("hours=")))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_HOURS);
    h.clamp(1, MAX_HOURS)
}

fn ts_of(line: &str) -> Option<u64> {
    serde_json::from_str::<Value>(line).ok()?.get("ts")?.as_u64()
}

/// Lines whose ts is at or after `cutoff`.
fn filter_since<'a>(lines: impl Iterator<Item = &'a str>, cutoff: u64) -> Vec<&'a str> {
    lines.filter(|l| ts_of(l).is_some_and(|t| t >= cutoff)).collect()
}

fn validate(line: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("bad json: {e}"))?;
    for f in FIELDS {
        if v.get(f).is_none() {
            return Err(format!("missing field {f}"));
        }
    }
    v["ts"].as_u64().ok_or("ts not integer")?;
    Ok(v)
}

/// Spark addresses: LAN first (works today), private-net address as fallback.
const SPARK_HOSTS: [&str; 2] = ["192.168.1.108", "100.64.0.2"];

fn fetch(remote: &str) -> Result<std::process::Output, String> {
    let mut last = String::from("no hosts");
    for host in SPARK_HOSTS {
        let r = Command::new("ssh")
            .args(["-i", "/root/.ssh/id_ed25519_headscale_backup", "-o", "BatchMode=yes",
                   "-o", "ConnectTimeout=5", &format!("drakestapleton@{host}"), remote])
            .output();
        match r {
            Ok(o) if o.status.success() => return Ok(o),
            Ok(o) => last = format!("{host}: ssh exit {}", o.status),
            Err(e) => last = format!("{host}: ssh failed to start: {e}"),
        }
    }
    Err(last)
}

/// Status feed: object with integer ts and string label.
fn validate_status(s: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(s.trim()).map_err(|e| format!("bad json: {e}"))?;
    v.get("ts").and_then(Value::as_u64).ok_or("status: ts not integer")?;
    v.get("label").and_then(Value::as_str).ok_or("status: label not string")?;
    if v.get("detail").is_some_and(|d| !d.is_string()) {
        return Err("status: detail not string".into());
    }
    Ok(v)
}

/// Results feed: integer updated, list of {name, verdict, date, receipt, note?}.
fn validate_results(s: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(s.trim()).map_err(|e| format!("bad json: {e}"))?;
    v.get("updated").and_then(Value::as_u64).ok_or("results: updated not integer")?;
    let list = v.get("results").and_then(Value::as_array).ok_or("results: results not a list")?;
    for r in list {
        for k in ["name", "date", "receipt"] {
            r.get(k).and_then(Value::as_str).ok_or(format!("results: {k} not string"))?;
        }
        match r.get("verdict").and_then(Value::as_str) {
            Some("PASS" | "FAIL" | "INCONCLUSIVE") => {}
            _ => return Err("results: bad verdict".into()),
        }
        if r.get("note").is_some_and(|n| !n.is_string()) {
            return Err("results: note not string".into());
        }
    }
    Ok(v)
}

/// Best-effort copy of one feed file to the hub; a bad or missing file keeps the old cache.
fn collect_feed(remote_path: &str, dest: &str, check: fn(&str) -> Result<Value, String>) {
    let res = fetch(&format!("cat {remote_path}")).and_then(|o| {
        let v = check(&String::from_utf8_lossy(&o.stdout))?;
        let body = serde_json::to_string(&v).map_err(|e| e.to_string())?;
        let tmp = format!("{dest}.tmp");
        fs::write(&tmp, body).map_err(|e| e.to_string())?;
        fs::rename(&tmp, dest).map_err(|e| e.to_string())
    });
    if let Err(e) = res {
        eprintln!("readings-window: feed {dest}: {e}");
    }
}

/// Serve a cached feed, or the default if missing or invalid.
fn feed_body(path: &str, check: fn(&str) -> Result<Value, String>, default: &str) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| check(&s).ok())
        .and_then(|v| serde_json::to_string(&v).ok())
        .unwrap_or_else(|| default.into())
}

fn validate_exposure(s: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    match v.get("status").and_then(Value::as_str) {
        Some("ok") | Some("changed") => Ok(v),
        _ => Err("bad status".into()),
    }
}

/// "ok", "changed" or "unknown" (missing, invalid, or older than an hour).
fn exposure_state(s: Option<&str>, now_s: u64) -> &'static str {
    let Some(v) = s.and_then(|s| validate_exposure(s).ok()) else { return "unknown" };
    let ts = v.get("ts").and_then(Value::as_u64).unwrap_or(0);
    if ts == 0 || now_s.saturating_sub(ts) > EXPOSURE_STALE_SECS {
        return "unknown";
    }
    if v["status"] == "ok" { "ok" } else { "changed" }
}

fn collect() -> Result<(), String> {
    let out = fetch("/usr/local/bin/spark-readings")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("");
    let v = validate(line)?;
    let compact = serde_json::to_string(&v).map_err(|e| e.to_string())?;
    let old = fs::read_to_string(HISTORY).unwrap_or_default();
    let cutoff = now().saturating_sub(KEEP_SECS);
    let mut keep = filter_since(old.lines(), cutoff).join("\n");
    if !keep.is_empty() {
        keep.push('\n');
    }
    keep.push_str(&compact);
    keep.push('\n');
    let tmp = format!("{HISTORY}.tmp");
    fs::File::create(&tmp).and_then(|mut f| f.write_all(keep.as_bytes())).map_err(|e| e.to_string())?;
    fs::rename(&tmp, HISTORY).map_err(|e| e.to_string())
}

fn last_line() -> Option<String> {
    fs::read_to_string(HISTORY).ok()?.lines().rev().find(|l| ts_of(l).is_some()).map(String::from)
}

const CHAT_DEFAULT_LINES: u64 = 500;
const CHAT_MAX_LINES: u64 = 50000;
const JSON_TYPE: &str = "application/json";
const TEXT_TYPE: &str = "text/plain; charset=utf-8";
const NDJSON_TYPE: &str = "application/x-ndjson";

/// One allowed agent: every remote path is a constant, never built from request input.
struct Agent {
    name: &'static str,
    latest_txt: &'static str,
    latest_jsonl: &'static str,
    records_glob: &'static str,
}

macro_rules! agent {
    ($n:literal) => {
        Agent {
            name: $n,
            latest_txt: concat!("/home/drakestapleton/archive/agent-chats/latest/", $n, ".txt"),
            latest_jsonl: concat!("/home/drakestapleton/archive/agent-chats/latest/", $n, ".jsonl"),
            records_glob: concat!("/home/drakestapleton/archive/agent-chats/records/", $n, "/*.jsonl"),
        }
    };
}

/// The fixed allow-list, matched exactly.
const AGENTS: [Agent; 5] = [agent!("claude"), agent!("codex"), agent!("opencode"), agent!("gemini"), agent!("muse")];
const AGENTS_JSON: &str = r#"{"agents":["claude","codex","opencode","gemini","muse"]}"#;

/// What one chat request reads on the Spark.
#[derive(Debug, PartialEq)]
struct ChatFeed {
    /// Rolling window file, read with `tail` (and with `cat` for `?all=1` on plain text).
    latest: &'static str,
    /// What `?all=1` reads: the same file (plain) or the records glob (jsonl).
    all: &'static str,
    ctype: &'static str,
}

/// Number of lines asked for in `?lines=N`: default 500, clamped to 1..=50000.
fn clamp_lines(q: Option<&str>) -> u64 {
    let n = q
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("lines=")))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(CHAT_DEFAULT_LINES);
    n.clamp(1, CHAT_MAX_LINES)
}

/// Remote command for a chat feed: only constant paths and a validated integer reach the shell.
fn chat_command(feed: &ChatFeed, q: Option<&str>) -> String {
    if q.is_some_and(|q| q.split('&').any(|kv| kv == "all=1")) {
        format!("cat {}", feed.all)
    } else {
        format!("tail -n {} {}", clamp_lines(q), feed.latest)
    }
}

/// `/v1/chat/<agent>` and `/v1/chat/<agent>.jsonl` for allow-listed agents only.
fn chat_route(path: &str) -> Option<ChatFeed> {
    let rest = path.strip_prefix("/v1/chat/")?;
    let (name, jsonl) = match rest.strip_suffix(".jsonl") {
        Some(n) => (n, true),
        None => (rest, false),
    };
    let a = AGENTS.iter().find(|a| a.name == name)?;
    Some(if jsonl {
        ChatFeed { latest: a.latest_jsonl, all: a.records_glob, ctype: NDJSON_TYPE }
    } else {
        ChatFeed { latest: a.latest_txt, all: a.latest_txt, ctype: TEXT_TYPE }
    })
}

/// Proxy a chat feed live from the Spark; nothing is written or cached on the hub.
fn chat_feed(feed: &ChatFeed, q: Option<&str>) -> (u16, String, &'static str) {
    match fetch(&chat_command(feed, q)) {
        Ok(o) => (200, String::from_utf8_lossy(&o.stdout).into_owned(), feed.ctype),
        Err(e) => {
            eprintln!("readings-window: chat feed {}: {e}", feed.latest);
            (502, r#"{"error":"spark unreachable"}"#.into(), JSON_TYPE)
        }
    }
}

fn resp(code: u16, ctype: &str, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", ctype).unwrap())
}

fn handle(method: &Method, url: &str, auth: Option<&str>, token: &str) -> (u16, String, &'static str) {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (url, None),
    };
    let chat = chat_route(path);
    let known = chat.is_some() || matches!(path, "/health" | "/v1/readings/current" | "/v1/readings/history" | "/v1/status" | "/v1/results" | "/v1/exposure" | "/v1/chat");
    if !known {
        return (404, r#"{"error":"not found"}"#.into(), JSON_TYPE);
    }
    if *method != Method::Get {
        return (405, r#"{"error":"method not allowed"}"#.into(), JSON_TYPE);
    }
    if path == "/health" {
        let ex = exposure_state(fs::read_to_string(EXPOSURE).ok().as_deref(), now());
        let age = match last_line().and_then(|l| ts_of(&l)) {
            Some(t) => now().saturating_sub(t).to_string(),
            None => "null".into(),
        };
        return (200, format!(r#"{{"status":"ok","last_reading_age_s":{age},"exposure":"{ex}"}}"#), JSON_TYPE);
    }
    if !auth_ok(auth, token) {
        return (401, r#"{"error":"unauthorized"}"#.into(), JSON_TYPE);
    }
    if path == "/v1/exposure" {
        return (200, feed_body(EXPOSURE, validate_exposure, EXPOSURE_DEFAULT), JSON_TYPE);
    }
    if path == "/v1/status" {
        return (200, feed_body(STATUS, validate_status, STATUS_DEFAULT), JSON_TYPE);
    }
    if path == "/v1/results" {
        return (200, feed_body(RESULTS, validate_results, RESULTS_DEFAULT), JSON_TYPE);
    }
    if path == "/v1/chat" {
        return (200, AGENTS_JSON.into(), JSON_TYPE);
    }
    if let Some(feed) = &chat {
        return chat_feed(feed, query);
    }
    if path == "/v1/readings/current" {
        return match last_line() {
            Some(l) => (200, l, JSON_TYPE),
            None => (503, r#"{"error":"no readings yet"}"#.into(), JSON_TYPE),
        };
    }
    let cutoff = now().saturating_sub(clamp_hours(query) * 3600);
    let all = fs::read_to_string(HISTORY).unwrap_or_default();
    let rows = filter_since(all.lines(), cutoff);
    (200, format!("[{}]", rows.join(",")), JSON_TYPE)
}

fn serve() -> Result<(), String> {
    let token = fs::read_to_string(TOKEN).map_err(|e| format!("token: {e}"))?.trim().to_string();
    if token.len() < 32 {
        return Err("token too short".into());
    }
    let ssl = SslConfig {
        certificate: fs::read(CERT).map_err(|e| format!("cert: {e}"))?,
        private_key: fs::read(KEY).map_err(|e| format!("key: {e}"))?,
    };
    let server = Server::https("0.0.0.0:9443", ssl).map_err(|e| format!("bind: {e}"))?;
    for req in server.incoming_requests() {
        let auth = req.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.to_string());
        let (code, body, ctype) = handle(req.method(), req.url(), auth.as_deref(), &token);
        let _ = req.respond(resp(code, ctype, body));
    }
    Ok(())
}

fn main() {
    let r = match std::env::args().nth(1).as_deref() {
        Some("collect") => {
            let r = collect();
            collect_feed("/home/drakestapleton/status/current.json", STATUS, validate_status);
            collect_feed("/home/drakestapleton/status/results.json", RESULTS, validate_results);
            r
        }
        Some("serve") => serve(),
        _ => Err("usage: readings-window collect|serve".into()),
    };
    if let Err(e) = r {
        eprintln!("readings-window: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exposure_parsing() {
        let n = 10_000;
        let ok = format!(r#"{{"ts":{},"status":"ok"}}"#, n - 60);
        let ch = format!(r#"{{"ts":{},"status":"changed"}}"#, n - 60);
        let old = format!(r#"{{"ts":{},"status":"ok"}}"#, n - 4000);
        assert_eq!(exposure_state(Some(&ok), n), "ok");
        assert_eq!(exposure_state(Some(&ch), n), "changed");
        assert_eq!(exposure_state(Some(&old), n), "unknown");
        assert_eq!(exposure_state(None, n), "unknown");
        assert_eq!(exposure_state(Some("junk"), n), "unknown");
        assert_eq!(exposure_state(Some(r#"{"ts":9990,"status":"weird"}"#), n), "unknown");
    }

    #[test]
    fn token_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(auth_ok(Some("Bearer s3cret"), "s3cret"));
        assert!(!auth_ok(Some("Bearer nope"), "s3cret"));
        assert!(!auth_ok(Some("s3cret"), "s3cret"));
        assert!(!auth_ok(None, "s3cret"));
    }
    #[test]
    fn hours_clamp() {
        assert_eq!(clamp_hours(None), 24);
        assert_eq!(clamp_hours(Some("hours=5")), 5);
        assert_eq!(clamp_hours(Some("hours=99999")), 720);
        assert_eq!(clamp_hours(Some("hours=0")), 1);
        assert_eq!(clamp_hours(Some("hours=abc")), 24);
    }
    #[test]
    fn history_filter() {
        let l = ["{\"ts\":100}", "{\"ts\":200}", "garbage", "{\"ts\":300}"];
        assert_eq!(filter_since(l.into_iter(), 200), vec!["{\"ts\":200}", "{\"ts\":300}"]);
    }
    #[test]
    fn validate_rejects_partial() {
        assert!(validate("{\"ts\":1}").is_err());
        assert!(validate("nope").is_err());
    }
    #[test]
    fn status_validation() {
        assert!(validate_status(r#"{"ts":5,"label":"x","detail":"d"}"#).is_ok());
        assert!(validate_status(r#"{"ts":5,"label":"x"}"#).is_ok());
        assert!(validate_status(r#"{"ts":"5","label":"x"}"#).is_err());
        assert!(validate_status(r#"{"ts":5}"#).is_err());
        assert!(validate_status(r#"{"ts":5,"label":"x","detail":1}"#).is_err());
        assert!(validate_status("").is_err());
    }
    #[test]
    fn results_validation() {
        let ok = r#"{"updated":1,"results":[{"name":"a","verdict":"PASS","date":"2026-10-04","receipt":"r","note":"n"}]}"#;
        assert!(validate_results(ok).is_ok());
        assert!(validate_results(r#"{"updated":0,"results":[]}"#).is_ok());
        assert!(validate_results(&ok.replace("PASS", "MAYBE")).is_err());
        assert!(validate_results(r#"{"updated":1}"#).is_err());
        assert!(validate_results(r#"{"updated":1,"results":[{"name":"a"}]}"#).is_err());
        assert!(validate_results("[]").is_err());
    }
    #[test]
    fn lines_clamp() {
        assert_eq!(clamp_lines(None), 500);
        assert_eq!(clamp_lines(Some("lines=0")), 1);
        assert_eq!(clamp_lines(Some("lines=1")), 1);
        assert_eq!(clamp_lines(Some("lines=500")), 500);
        assert_eq!(clamp_lines(Some("lines=60000")), 50000);
        assert_eq!(clamp_lines(Some("lines=abc")), 500);
        assert_eq!(clamp_lines(Some("lines=1;rm -rf /")), 500);
        assert_eq!(clamp_lines(Some("lines=-5")), 500);
    }
    const A: &str = "/home/drakestapleton/archive/agent-chats";
    fn route(p: &str) -> ChatFeed {
        chat_route(p).unwrap()
    }
    #[test]
    fn chat_commands_plain() {
        let f = route("/v1/chat/codex");
        let t = format!("{A}/latest/codex.txt");
        assert_eq!(chat_command(&f, None), format!("tail -n 500 {t}"));
        assert_eq!(chat_command(&f, Some("lines=60000")), format!("tail -n 50000 {t}"));
        assert_eq!(chat_command(&f, Some("lines=7")), format!("tail -n 7 {t}"));
        assert_eq!(chat_command(&f, Some("all=1")), format!("cat {t}"));
        assert_eq!(chat_command(&f, Some("all=1;id")), format!("tail -n 500 {t}"));
    }
    #[test]
    fn chat_commands_jsonl() {
        let f = route("/v1/chat/opencode.jsonl");
        assert_eq!(chat_command(&f, None), format!("tail -n 500 {A}/latest/opencode.jsonl"));
        assert_eq!(chat_command(&f, Some("lines=7&all=1")), format!("cat {A}/records/opencode/*.jsonl"));
        assert_eq!(chat_command(&f, Some("all=1")), format!("cat {A}/records/opencode/*.jsonl"));
    }
    #[test]
    fn chat_allow_list() {
        for a in ["claude", "codex", "opencode", "gemini", "muse"] {
            assert_eq!(route(&format!("/v1/chat/{a}")).ctype, "text/plain; charset=utf-8");
            assert_eq!(route(&format!("/v1/chat/{a}.jsonl")).ctype, "application/x-ndjson");
            assert_eq!(route(&format!("/v1/chat/{a}")).latest, format!("{A}/latest/{a}.txt"));
        }
        for bad in ["/v1/chat/other", "/v1/chat/claude.txt", "/v1/chat/", "/v1/chat/claude/", "/v1/chat/Claude",
                    "/v1/chat/../claude", "/v1/chat/claude.jsonl.jsonl", "/v1/chat/.jsonl", "/v1/chat/claude;id"] {
            assert_eq!(chat_route(bad), None, "{bad}");
        }
        let tok = "s3cret";
        assert_eq!(handle(&Method::Get, "/v1/chat/other", Some("Bearer s3cret"), tok).0, 404);
        assert_eq!(handle(&Method::Get, "/v1/chat/claude.txt", None, tok).0, 404);
    }
    #[test]
    fn chat_auth_and_method() {
        let tok = "s3cret";
        let mut paths: Vec<String> = vec!["/v1/chat".into()];
        for a in ["claude", "codex", "opencode", "gemini", "muse"] {
            paths.push(format!("/v1/chat/{a}"));
            paths.push(format!("/v1/chat/{a}.jsonl"));
        }
        for p in &paths {
            assert_eq!(handle(&Method::Get, p, None, tok).0, 401, "{p}");
            assert_eq!(handle(&Method::Get, &format!("{p}?lines=5"), Some("Bearer wrong"), tok).0, 401, "{p}");
            assert_eq!(handle(&Method::Post, p, None, tok).0, 405, "{p}");
        }
    }
    #[test]
    fn chat_agent_list() {
        let (code, body, ctype) = handle(&Method::Get, "/v1/chat", Some("Bearer s3cret"), "s3cret");
        assert_eq!((code, ctype), (200, "application/json"));
        assert_eq!(body, r#"{"agents":["claude","codex","opencode","gemini","muse"]}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["agents"].as_array().unwrap().len(), AGENTS.len());
    }
    #[test]
    fn feed_defaults() {
        assert_eq!(feed_body("/nonexistent/x", validate_status, STATUS_DEFAULT), STATUS_DEFAULT);
        assert_eq!(feed_body("/nonexistent/x", validate_results, RESULTS_DEFAULT), RESULTS_DEFAULT);
    }
}
