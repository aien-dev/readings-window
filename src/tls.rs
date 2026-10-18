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
            buf.truncate(i + 2);
            return Some(buf);
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
    None
}

fn serve_conn(tcp: TcpStream, cfg: Arc<ServerConfig>, token: &str) {
    let _ = tcp.set_read_timeout(Some(IO_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(IO_TIMEOUT));
    let Ok(conn) = ServerConnection::new(cfg) else {
        return;
    };
    let mut tls = StreamOwned::new(conn, tcp);
    let Some(head) = read_head(&mut tls) else {
        return;
    };
    let out = match parse_head(&head) {
        Some((m, t, a)) => {
            let (code, body, ctype) = crate::handle(&m, &t, a.as_deref(), token);
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
    let live = Arc::new(AtomicUsize::new(0));
    let token = Arc::new(token);
    for tcp in listener.incoming().flatten() {
        if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            live.fetch_sub(1, Ordering::SeqCst);
            continue; // dropped: shed load instead of queueing
        }
        let (cfg, live, token) = (cfg.clone(), live.clone(), token.clone());
        std::thread::spawn(move || {
            serve_conn(tcp, cfg, &token);
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
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cfg = tls_config(
            ck.cert.pem().as_bytes(),
            ck.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || serve_on(listener, cfg, TOKEN.to_string()));
        (addr, ck.cert.der().clone())
    }

    fn get(
        addr: std::net::SocketAddr,
        cert: &CertificateDer<'static>,
        auth: Option<&str>,
    ) -> String {
        let mut roots = RootCertStore::empty();
        roots.add(cert.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let conn = ClientConnection::new(Arc::new(cfg), ServerName::try_from("localhost").unwrap())
            .unwrap();
        let mut s = StreamOwned::new(conn, TcpStream::connect(addr).unwrap());
        let a = auth
            .map(|a| format!("Authorization: {a}\r\n"))
            .unwrap_or_default();
        s.write_all(format!("GET /v1/status HTTP/1.1\r\nHost: localhost\r\n{a}\r\n").as_bytes())
            .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out); // server closes without close_notify on some paths
        out
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
            let n = s.read(&mut buf).unwrap_or(0);
            // Either closed, or a TLS alert (0x15). Never a ServerHello (0x16).
            assert!(
                n == 0 || buf[0] == 0x15,
                "TLS 1.{minor} got {:?}",
                &buf[..n.min(8)]
            );
        }
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
