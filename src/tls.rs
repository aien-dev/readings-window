//! HTTPS front end: rustls 0.23 terminates TLS, then a deliberately tiny HTTP/1.1
//! reader hands (method, target, Authorization) to `handle`. One request per connection.
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_HEAD: usize = 16 * 1024;
const MAX_CONNS: usize = 64;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const HEAD_DEADLINE: Duration = Duration::from_secs(15);

// Enforce the total header budget at TCP I/O boundaries, including reads inside
// rustls' handshake/record loop. A timer outside StreamOwned::read cannot do that.
struct HeadBudgetIo {
    tcp: TcpStream,
    deadline: Option<Instant>,
}
impl HeadBudgetIo {
    fn timeout(&self) -> std::io::Result<Duration> {
        match self.deadline {
            None => Ok(IO_TIMEOUT),
            Some(deadline) => {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .filter(|left| !left.is_zero())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "TLS/header deadline elapsed",
                        )
                    })?;
                Ok(IO_TIMEOUT.min(left))
            }
        }
    }
}
impl Read for HeadBudgetIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.tcp.set_read_timeout(Some(self.timeout()?))?;
        self.tcp.read(buf)
    }
}
impl Write for HeadBudgetIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tcp.set_write_timeout(Some(self.timeout()?))?;
        self.tcp.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

/// TLS 1.2 and 1.3 only (rustls does not implement older versions), no client certificates.
pub fn tls_config(cert_pem: &[u8], key_pem: &[u8]) -> Result<Arc<ServerConfig>, String> {
    let certs = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("cert: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| format!("key: {e}"))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| format!("tls: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("cert/key: {e}"))?;
    Ok(Arc::new(cfg))
}

/// Parsed request head: method, target, Authorization header value.
pub fn parse_head(head: &[u8]) -> Option<(String, String, Option<String>)> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return None;
    }
    let auth = lines.take_while(|l| !l.is_empty()).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("authorization")
            .then(|| v.trim().to_string())
    });
    Some((method.to_string(), target.to_string(), auth))
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

fn response_bytes(code: u16, ctype: &str, body: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(code),
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn read_head<S: Read>(s: &mut S) -> Option<Vec<u8>> {
    let start = Instant::now();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while start.elapsed() < HEAD_DEADLINE {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if i + 4 > MAX_HEAD {
                return None;
            }
            buf.truncate(i + 2);
            return Some(buf);
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
    None
}

fn serve_conn(tcp: TcpStream, cfg: Arc<ServerConfig>, token: &str, history: &str) {
    let Ok(conn) = ServerConnection::new(cfg) else {
        return;
    };
    let io = HeadBudgetIo {
        tcp,
        deadline: Some(Instant::now() + HEAD_DEADLINE),
    };
    let mut tls = StreamOwned::new(conn, io);
    let Some(head) = read_head(&mut tls) else {
        return;
    };
    // The request head is complete; normal response I/O retains its per-call timeout.
    tls.sock.deadline = None;
    let out = match parse_head(&head) {
        Some((m, t, a)) => {
            let (code, body, ctype) = crate::handle(&m, &t, a.as_deref(), token, history);
            response_bytes(code, ctype, &body)
        }
        None => response_bytes(400, crate::JSON_TYPE, r#"{"error":"bad request"}"#),
    };
    let _ = tls.write_all(&out);
    tls.conn.send_close_notify();
    let _ = tls.flush();
}

/// Accept loop: one thread per connection, at most MAX_CONNS at once.
pub fn serve_on(listener: TcpListener, cfg: Arc<ServerConfig>, token: String) {
    serve_on_with_history(listener, cfg, token, crate::HISTORY.to_string());
}

// Trusted immutable setup only: never derived from an HTTP request or environment.
fn serve_on_with_history(
    listener: TcpListener,
    cfg: Arc<ServerConfig>,
    token: String,
    history: String,
) {
    let live = Arc::new(AtomicUsize::new(0));
    let token = Arc::new(token);
    let history = Arc::new(history);
    for tcp in listener.incoming().flatten() {
        if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            live.fetch_sub(1, Ordering::SeqCst);
            continue; // dropped: shed load instead of queueing
        }
        let (cfg, live, token, history) =
            (cfg.clone(), live.clone(), token.clone(), history.clone());
        std::thread::spawn(move || {
            serve_conn(tcp, cfg, &token, &history);
            live.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{pki_types::ServerName, ClientConfig, ClientConnection, RootCertStore};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn start() -> (std::net::SocketAddr, CertificateDer<'static>) {
        start_at_history(crate::HISTORY.to_string())
    }

    fn start_at_history(history: String) -> (std::net::SocketAddr, CertificateDer<'static>) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cfg = tls_config(
            ck.cert.pem().as_bytes(),
            ck.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            serve_on_with_history(listener, cfg, TOKEN.to_string(), history)
        });
        (addr, ck.cert.der().clone())
    }

    fn get(
        addr: std::net::SocketAddr,
        cert: &CertificateDer<'static>,
        auth: Option<&str>,
    ) -> String {
        get_path(
            addr,
            cert,
            auth,
            "/v1/status",
            &[&rustls::version::TLS13, &rustls::version::TLS12],
        )
    }

    fn get_path(
        addr: std::net::SocketAddr,
        cert: &CertificateDer<'static>,
        auth: Option<&str>,
        path: &str,
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> String {
        let mut roots = RootCertStore::empty();
        roots.add(cert.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let conn = ClientConnection::new(Arc::new(cfg), ServerName::try_from("localhost").unwrap())
            .unwrap();
        let mut s = StreamOwned::new(conn, TcpStream::connect(addr).unwrap());
        s.sock
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        s.sock
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let a = auth
            .map(|a| format!("Authorization: {a}\r\n"))
            .unwrap_or_default();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{a}\r\n").as_bytes())
            .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out); // server closes without close_notify on some paths
        assert!(
            versions
                .iter()
                .any(|v| Some(v.version) == s.conn.protocol_version()),
            "requested TLS version must actually negotiate"
        );
        out
    }

    struct FixtureDir(std::path::PathBuf);
    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reading_file_roundtrip_is_token_protected_under_tls12_and_tls13() {
        let dir = std::env::temp_dir().join(format!("readings-tls-fixture-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let fixture_dir = FixtureDir(dir);
        let fixture = serde_json::json!({"ts":1,"gpu_util_pct":0,"gpu_temp_c":0,"gpu_power_w":0,"load1":0,"load5":0,"load15":0,"cpu_temp_c":0,"mem_used_mb":0,"mem_total_mb":0,"fixture":"synthetic-not-hardware"});
        let line = fixture.to_string();
        crate::validate(&line).unwrap();
        let path = fixture_dir.0.join("history.jsonl");
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let (addr, cert) = start_at_history(path.to_str().unwrap().to_string());
        for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
            let response = get_path(
                addr,
                &cert,
                Some(&format!("Bearer {TOKEN}")),
                "/v1/readings/current",
                &[version],
            );
            let (headers, body) = response
                .split_once("\r\n\r\n")
                .expect("complete HTTP response");
            assert!(headers.starts_with("HTTP/1.1 200 OK"), "{response}");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(body).unwrap(),
                fixture
            );
            for auth in [None, Some("Bearer wrong")] {
                let denied = get_path(addr, &cert, auth, "/v1/readings/current", &[version]);
                assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
                assert!(!denied.contains("synthetic-not-hardware"));
            }
            println!(
                "{version:?}: real TLS reading200 exact synthetic fixture; absent/wrong token401"
            );
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            format!("{line}\n"),
            "HTTPS reads must not modify the fixture"
        );
    }

    #[test]
    fn handshake_with_token_serves_and_without_is_refused() {
        let (addr, cert) = start();
        let ok = get(addr, &cert, Some(&format!("Bearer {TOKEN}")));
        assert!(ok.starts_with("HTTP/1.1 200 OK"), "{ok}");
        assert!(ok.contains(r#""label""#), "{ok}");
        let no = get(addr, &cert, None);
        assert!(no.starts_with("HTTP/1.1 401"), "{no}");
        let bad = get(addr, &cert, Some("Bearer wrong"));
        assert!(bad.starts_with("HTTP/1.1 401"), "{bad}");
    }

    fn legacy_refused(result: &std::io::Result<usize>, buf: &[u8]) -> bool {
        match result {
            Ok(0) => true,
            Ok(_) => buf.first() == Some(&0x15),
            Err(e) => matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ),
        }
    }

    #[test]
    fn tls10_and_tls11_clients_are_refused() {
        let (addr, _) = start();
        for minor in [0x01u8, 0x02] {
            // ClientHello: legacy version TLS 1.0/1.1, one CBC suite, no extensions.
            let mut hello = vec![0x03, minor];
            hello.extend([0u8; 32]); // random
            hello.push(0); // session id
            hello.extend([0x00, 0x02, 0x00, 0x2f]); // TLS_RSA_WITH_AES_128_CBC_SHA
            hello.extend([0x01, 0x00]); // null compression
            let mut hs = vec![0x01, 0, 0, hello.len() as u8];
            hs.extend(hello);
            let mut rec = vec![0x16, 0x03, 0x01, 0, hs.len() as u8];
            rec.extend(hs);
            let mut s = TcpStream::connect(addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            s.write_all(&rec).unwrap();
            let mut buf = [0u8; 64];
            let result = s.read(&mut buf);
            // An alert or a closed connection is refusal; retain the result for diagnostics.
            let version_minor = minor - 1;
            assert!(
                legacy_refused(&result, &buf),
                "TLS 1.{version_minor} got {result:?}, bytes {:?}",
                &buf[..8]
            );
            println!("TLS1.{version_minor}: actual legacy ClientHello refused: {result:?}");
        }
    }

    #[test]
    fn legacy_refusal_never_treats_a_timeout_as_success() {
        use std::io::{Error, ErrorKind};
        for kind in [ErrorKind::TimedOut, ErrorKind::WouldBlock, ErrorKind::Other] {
            assert!(
                !legacy_refused(&Err(Error::from(kind)), &[0u8; 64]),
                "{kind:?} is not protocol refusal"
            );
        }
        assert!(legacy_refused(&Ok(0), &[]));
        assert!(legacy_refused(&Ok(1), &[0x15]));
        assert!(!legacy_refused(&Ok(1), &[0x16]));
    }

    #[test]
    fn request_head_bound_includes_the_terminator() {
        use std::io::Cursor;
        let mut at_limit = b"GET /health HTTP/1.1\r\nX-Pad: ".to_vec();
        at_limit.resize(MAX_HEAD - 4, b'a');
        at_limit.extend_from_slice(b"\r\n\r\n");
        assert_eq!(at_limit.len(), MAX_HEAD);
        assert!(read_head(&mut Cursor::new(at_limit.clone())).is_some());
        let mut overflow = at_limit[..MAX_HEAD - 4].to_vec();
        overflow.push(b'a');
        overflow.extend_from_slice(b"\r\n\r\n");
        assert!(
            read_head(&mut Cursor::new(overflow)).is_none(),
            "terminator beyond MAX_HEAD must be refused"
        );
        let expected = b"GET /health HTTP/1.1\r\nHost: localhost\r\n";
        let mut body = expected.to_vec();
        body.extend_from_slice(b"\r\n");
        body.resize(MAX_HEAD * 2, b'x');
        assert_eq!(read_head(&mut Cursor::new(body)).unwrap(), expected);
    }

    #[test]
    fn fragmented_tls_cannot_keep_the_header_deadline_alive() {
        let (addr, _) = start();
        let mut reader = TcpStream::connect(addr).unwrap();
        reader
            .set_read_timeout(Some(HEAD_DEADLINE + Duration::from_secs(4)))
            .unwrap();
        // An incomplete TLS record never yields plaintext to read_head. Keep TCP active
        // more often than IO_TIMEOUT, without completing the record or handshake.
        reader.write_all(&[0x16, 0x03, 0x01, 0x00, 0x40]).unwrap();
        let mut writer = reader.try_clone().unwrap();
        writer
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let keep = active.clone();
        let sender = std::thread::spawn(move || {
            while keep.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
                if !keep.load(Ordering::SeqCst) || writer.write_all(&[0]).is_err() {
                    break;
                }
            }
        });
        let mut buf = [0u8; 64];
        let result = reader.read(&mut buf);
        active.store(false, Ordering::SeqCst);
        sender.join().unwrap();
        assert!(
            legacy_refused(&result, &buf),
            "TCP activity must not bypass the total TLS/header deadline: {result:?}"
        );
        println!("fragmented TLS record: header deadline closed connection despite periodic TCP bytes: {result:?}");
    }

    #[test]
    fn head_parsing() {
        let h = b"GET /x?y=1 HTTP/1.1\r\nHost: a\r\nauthorization:  Bearer t \r\n";
        assert_eq!(
            parse_head(h),
            Some(("GET".into(), "/x?y=1".into(), Some("Bearer t".into())))
        );
        assert!(parse_head(b"GET x HTTP/1.1\r\n").is_none());
        assert!(parse_head(b"GET / SPDY\r\n").is_none());
    }
}
