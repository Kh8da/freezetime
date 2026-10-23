//! The local HTTP endpoint CS2 posts to. Deliberately small: one blocking
//! accept loop on 127.0.0.1, so it costs nothing while CS2 is closed.
//!
//! Every request must carry the token from the cfg file. Requests with an
//! `Origin` header are refused outright: CS2 never sends one, and every browser
//! does on a cross-site POST, so a web page can't feed in fake match data.

use crate::gsi::{self, Payload};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Real payloads are a few KB. Anything near this is not CS2.
const MAX_BODY_BYTES: usize = 512 * 1024;
const MAX_CONNECTIONS: usize = 8;
/// A kept-alive connection that's quiet this long is closed.
const IDLE: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum Incoming {
    Payload(Box<Payload>),
    /// A CS2 payload with the wrong token: the game was started with an older cfg.
    WrongToken,
}

pub type Handler = Arc<dyn Fn(Incoming) + Send + Sync>;

pub struct Listener {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    /// Binds 127.0.0.1:`port` (0 picks a free port) and starts accepting.
    pub fn start(port: u16, token: &str, handler: Handler) -> io::Result<Listener> {
        let socket = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        let addr = socket.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let token = Arc::new(token.to_string());
        let open = Arc::new(AtomicUsize::new(0));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new().name("gsi-listener".into()).spawn(move || {
            for conn in socket.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
                    continue;
                }
                if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue;
                }
                open.fetch_add(1, Ordering::SeqCst);
                let (token, handler, count) = (token.clone(), handler.clone(), open.clone());
                let spawned = std::thread::Builder::new().name("gsi-conn".into()).spawn(move || {
                    let _ = serve(stream, &token, &handler);
                    count.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    open.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
        Ok(Listener { addr, stop, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so the thread sees the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.shutdown();
        }
    }
}

struct Request {
    method: String,
    origin: bool,
    close: bool,
    length: Option<usize>,
    chunked: bool,
}

fn read_head(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut total = 0;
    let mut line = String::new();
    let mut req: Option<Request> = None;
    loop {
        line.clear();
        let n = reader.by_ref().take((MAX_HEADER_BYTES - total + 1) as u64).read_line(&mut line)?;
        if n == 0 {
            // Connection closed (cleanly between requests, or mid-header).
            return Ok(None);
        }
        total += n;
        if total > MAX_HEADER_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "headers too large"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        match &mut req {
            None => {
                if trimmed.is_empty() {
                    continue; // stray CRLF between kept-alive requests
                }
                let method = trimmed.split(' ').next().unwrap_or("").to_string();
                let close = trimmed.ends_with("HTTP/1.0");
                req = Some(Request { method, origin: false, close, length: None, chunked: false });
            }
            Some(r) => {
                if trimmed.is_empty() {
                    return Ok(req);
                }
                let Some((name, value)) = trimmed.split_once(':') else { continue };
                let value = value.trim();
                match name.trim().to_ascii_lowercase().as_str() {
                    "origin" => r.origin = true,
                    "content-length" => r.length = value.parse().ok(),
                    "transfer-encoding" => r.chunked = value.to_ascii_lowercase().contains("chunked"),
                    "connection" => r.close = value.eq_ignore_ascii_case("close"),
                    _ => {}
                }
            }
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, close: bool) -> io::Result<()> {
    let conn = if close { "close" } else { "keep-alive" };
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: {conn}\r\n\r\n");
    stream.write_all(head.as_bytes())
}

/// Compares without stopping at the first different byte, so response timing
/// says nothing about how much of a guessed token was right.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn serve(stream: TcpStream, token: &str, handler: &Handler) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_nodelay(true)?;
    let mut out = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let Some(req) = read_head(&mut reader)? else { return Ok(()) };
        if req.origin {
            return respond(&mut out, "403 Forbidden", true);
        }
        if req.method != "POST" {
            return respond(&mut out, "405 Method Not Allowed", true);
        }
        if req.chunked {
            return respond(&mut out, "411 Length Required", true);
        }
        let Some(len) = req.length else {
            return respond(&mut out, "411 Length Required", true);
        };
        if len > MAX_BODY_BYTES {
            return respond(&mut out, "413 Payload Too Large", true);
        }
        let mut body = vec![0; len];
        reader.read_exact(&mut body)?;
        let Ok(payload) = gsi::parse(&body) else {
            return respond(&mut out, "400 Bad Request", true);
        };
        let sent = payload.auth.as_ref().map(|a| a.token.as_str()).unwrap_or("");
        if !same(sent, token) {
            if payload.provider.as_ref().is_some_and(|p| p.appid == 730) {
                handler(Incoming::WrongToken);
            }
            return respond(&mut out, "401 Unauthorized", true);
        }
        // Answer first: CS2 waits for the response before it sends the next update.
        respond(&mut out, "200 OK", req.close)?;
        handler(Incoming::Payload(Box::new(payload)));
        if req.close {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn start() -> (Listener, Arc<Mutex<Vec<String>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let l = Listener::start(
            0,
            "secret",
            Arc::new(move |inc| {
                g.lock().unwrap().push(match inc {
                    Incoming::Payload(p) => p.map.map(|m| m.name).unwrap_or_else(|| "menu".into()),
                    Incoming::WrongToken => "wrong token".into(),
                })
            }),
        )
        .unwrap();
        (l, got)
    }

    fn send(port: u16, raw: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut resp = String::new();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let _ = s.read_to_string(&mut resp);
        resp
    }

    fn post(body: &str, extra: &str) -> String {
        format!("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    #[test]
    fn accepts_the_right_token_only() {
        let (l, got) = start();
        let ok = r#"{"provider":{"appid":730},"map":{"name":"de_dust2"},"auth":{"token":"secret"}}"#;
        let bad = r#"{"provider":{"appid":730},"map":{"name":"de_dust2"},"auth":{"token":"secreT"}}"#;
        let none = r#"{"provider":{"appid":730},"map":{"name":"de_dust2"}}"#;
        assert!(send(l.port(), &post(ok, "")).starts_with("HTTP/1.1 200"));
        assert!(send(l.port(), &post(bad, "")).starts_with("HTTP/1.1 401"));
        assert!(send(l.port(), &post(none, "")).starts_with("HTTP/1.1 401"));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*got.lock().unwrap(), vec!["de_dust2", "wrong token", "wrong token"]);
    }

    #[test]
    fn refuses_browsers_gets_and_junk() {
        let (l, got) = start();
        let ok = r#"{"auth":{"token":"secret"}}"#;
        assert!(send(l.port(), &post(ok, "Origin: https://example.com\r\n")).starts_with("HTTP/1.1 403"));
        assert!(send(l.port(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n").starts_with("HTTP/1.1 405"));
        assert!(send(l.port(), &post("not json", "")).starts_with("HTTP/1.1 400"));
        assert!(send(l.port(), "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").starts_with("HTTP/1.1 411"));
        assert!(send(l.port(), "POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n").starts_with("HTTP/1.1 413"));
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn handles_several_requests_on_one_kept_alive_connection() {
        let (l, got) = start();
        let mut s = TcpStream::connect(("127.0.0.1", l.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        for map in ["de_nuke", "de_vertigo"] {
            let body = format!(r#"{{"map":{{"name":"{map}"}},"auth":{{"token":"secret"}}}}"#);
            write!(s, "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            let mut buf = [0u8; 256];
            let n = s.read(&mut buf).unwrap();
            let r = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(r.starts_with("HTTP/1.1 200"), "{r:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*got.lock().unwrap(), vec!["de_nuke", "de_vertigo"]);
    }

    #[test]
    fn only_listens_on_loopback_and_stops() {
        let (l, _) = start();
        let port = l.port();
        assert!(l.addr.ip().is_loopback());
        l.stop();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }
}
