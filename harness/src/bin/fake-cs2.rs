//! A stand-in for CS2's Game State Integration client. It reads a
//! `gamestate_integration_*.cfg` the way the game does, then posts a scripted
//! sequence of payloads to the address inside, one at a time over a kept-alive
//! connection, waiting for each response like CS2.
//!
//!   fake-cs2 --cfg <path to cfg> [--scenario session] [--pace 0.2] [--count 40] [--token other]
//!
//! `--pace` is seconds between payloads (0 sends as fast as the app answers).
//! `--count` stops after that many payloads, for example in the middle of a round.
//! `--token` overrides the token from the cfg, to act like a CS2 that was started
//! before the cfg changed.

#[path = "../scenarios.rs"]
mod scenarios;

use engine::vdf;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn fail(msg: &str) -> ! {
    eprintln!("fake-cs2: {msg}");
    std::process::exit(2);
}

/// "http://127.0.0.1:38127/" -> ("127.0.0.1:38127", "/")
fn split_uri(uri: &str) -> (String, String) {
    let rest = uri.strip_prefix("http://").unwrap_or_else(|| fail("uri must start with http://"));
    match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    }
}

struct Conn {
    addr: String,
    stream: Option<(TcpStream, BufReader<TcpStream>)>,
}

impl Conn {
    fn post(&mut self, path: &str, body: &str) -> std::io::Result<u16> {
        if self.stream.is_none() {
            let s = TcpStream::connect(&self.addr)?;
            s.set_read_timeout(Some(Duration::from_secs(5)))?;
            let r = BufReader::new(s.try_clone()?);
            self.stream = Some((s, r));
        }
        let (s, r) = self.stream.as_mut().unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Valve/Steam HTTP Client 1.0 (730)\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccept: text/html,*/*;q=0.9\r\nAccept-Charset: ISO-8859-1,utf-8,*;q=0.7\r\nCache-Control: no-cache\r\nPragma: no-cache\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        s.write_all(req.as_bytes())?;
        let mut status = String::new();
        r.read_line(&mut status)?;
        let code: u16 = status.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let mut close = false;
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            r.read_line(&mut line)?;
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if lower == "connection: close" {
                close = true;
            }
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut sink = vec![0; len];
        r.read_exact(&mut sink)?;
        if close {
            self.stream = None;
        }
        Ok(code)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cfg_path = arg(&args, "--cfg").unwrap_or_else(|| fail("--cfg <path> is required"));
    let scenario = arg(&args, "--scenario").unwrap_or_else(|| "session".into());
    let pace: f64 = arg(&args, "--pace").and_then(|p| p.parse().ok()).unwrap_or(0.0);

    let text = std::fs::read_to_string(&cfg_path).unwrap_or_else(|e| fail(&format!("can't read {cfg_path}: {e}")));
    let root = vdf::parse(&text).unwrap_or_else(|e| fail(&format!("cfg doesn't parse: {e}")));
    let (_, block) = root.entries.first().unwrap_or_else(|| fail("cfg is empty"));
    let vdf::Value::Obj(block) = block else { fail("cfg has no block") };
    let uri = block.get_str("uri").unwrap_or_else(|| fail("cfg has no uri"));
    let token = arg(&args, "--token")
        .or_else(|| block.get_obj("auth").and_then(|a| a.get_str("token")).map(String::from))
        .unwrap_or_default();
    let (addr, path) = split_uri(uri);

    // The scenario's clock is set so the last payload sent is stamped "now":
    // a three-hour session replays in seconds but reads as having just happened.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let unknown = || fail(&format!("unknown scenario {scenario}; try one of {:?}", scenarios::NAMES));
    let count: Option<usize> = arg(&args, "--count").and_then(|c| c.parse().ok());
    let probe = scenarios::build(&scenario, &token, 0).unwrap_or_else(unknown);
    let last = count.map(|n| n.clamp(1, probe.len())).unwrap_or(probe.len()) - 1;
    let span = engine::gsi::parse(probe[last].as_bytes()).ok().and_then(|p| p.provider).map(|p| p.timestamp).unwrap_or(0);
    let mut payloads = scenarios::build(&scenario, &token, now - span).unwrap_or_else(unknown);
    payloads.truncate(last + 1);

    let mut conn = Conn { addr, stream: None };
    let (mut ok, mut refused) = (0, 0);
    for body in &payloads {
        match conn.post(&path, body) {
            Ok(200) => ok += 1,
            Ok(code) => {
                refused += 1;
                conn.stream = None;
                if refused == 1 {
                    eprintln!("fake-cs2: app answered {code}");
                }
            }
            Err(e) => fail(&format!("couldn't reach {}: {e}", conn.addr)),
        }
        if pace > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(pace));
        }
    }
    println!("fake-cs2: sent {} payloads ({ok} accepted, {refused} refused)", payloads.len());
}
