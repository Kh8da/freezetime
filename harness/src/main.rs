//! End-to-end test: freezetime's real listener, engine and storage against
//! `fake-cs2`, which finds its address and token in the cfg file freezetime
//! installed, exactly as CS2 does.
//!
//! Everything happens in a throwaway folder (a fake Steam library and app data
//! folder), so this is safe to run on any machine, including your own.

use engine::cs2::{self, CfgStatus};
use engine::engine::{Live, Rules};
use engine::service::{now, Service, Sink};
use engine::stats::History;
use engine::store::Store;
use engine::tracker::Outcome;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static FAILS: AtomicU32 = AtomicU32::new(0);

fn check(ok: bool, msg: &str) {
    println!("{}  {}", if ok { "PASS" } else { "FAIL" }, msg);
    if !ok {
        FAILS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Records what the app would do: media keys, notifications, redraws.
struct Recorder {
    store: Store,
    log: Mutex<Vec<String>>,
    redraws: AtomicU32,
}

impl Sink for Recorder {
    fn pause_music(&self) {
        self.log.lock().unwrap().push("pause".into());
    }
    fn resume_music(&self) {
        self.log.lock().unwrap().push("resume".into());
    }
    fn notify(&self, title: &str, body: &str) {
        self.log.lock().unwrap().push(format!("notify {title}: {body}"));
    }
    fn save_history(&self, history: &History) {
        self.store.save_history(history).unwrap();
    }
    fn changed(&self, _live: &Live) {
        self.redraws.fetch_add(1, Ordering::SeqCst);
    }
}

impl Recorder {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.log.lock().unwrap())
    }
}

fn fake_steam(root: &Path) -> PathBuf {
    let steam = root.join("Steam");
    let lib = root.join("Library");
    fs::create_dir_all(steam.join("steamapps")).unwrap();
    fs::create_dir_all(lib.join("steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg")).unwrap();
    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    fs::write(
        steam.join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"730\"\t\t\"1\"\n\t\t}}\n\t}}\n}}\n",
            esc(&steam),
            esc(&lib)
        ),
    )
    .unwrap();
    fs::write(
        lib.join("steamapps/appmanifest_730.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"installdir\"\t\t\"Counter-Strike Global Offensive\"\n}\n",
    )
    .unwrap();
    steam
}

fn fake_cs2_exe() -> PathBuf {
    let me = std::env::current_exe().unwrap();
    me.with_file_name(format!("fake-cs2{}", std::env::consts::EXE_SUFFIX))
}

fn run_fake(cfg: &Path, scenario: &str, token: Option<&str>) -> String {
    let mut cmd = Command::new(fake_cs2_exe());
    cmd.arg("--cfg").arg(cfg).arg("--scenario").arg(scenario);
    if let Some(t) = token {
        cmd.arg("--token").arg(t);
    }
    let out = cmd.output().expect("couldn't start fake-cs2; build it with cargo build -p harness");
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        println!("      fake-cs2 failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    text
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// An address of this machine that isn't loopback, if it has one.
fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // No packet is sent: connecting a UDP socket only picks a route.
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

fn main() {
    let root = std::env::temp_dir().join(format!("freezetime-harness-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    println!("working in {}", root.display());

    // ---------- setup: find CS2 and install the cfg ----------

    let steam = fake_steam(&root);
    let store = Store::new(root.join("appdata"));
    let mut settings = store.settings();
    settings.port = free_port();
    store.save_settings(&settings).unwrap();

    let cfg_dir = cs2::find_cfg_dir(&[root.join("not-steam"), steam.clone()]);
    check(cfg_dir.is_some(), "finds CS2 through libraryfolders.vdf in a second library");
    let cfg_dir = cfg_dir.unwrap();
    check(cs2::status(&cfg_dir, settings.port, &settings.token) == CfgStatus::Missing, "cfg starts missing");
    let cfg = cs2::install(&cfg_dir, settings.port, &settings.token).unwrap();
    check(cs2::status(&cfg_dir, settings.port, &settings.token) == CfgStatus::Installed, "cfg installed");
    let names: Vec<_> = fs::read_dir(&cfg_dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    check(names == [cs2::CFG_NAME], "the cfg is the only file in CS2's folder");

    let rec = Arc::new(Recorder { store: Store::new(root.join("appdata")), log: Mutex::new(Vec::new()), redraws: AtomicU32::new(0) });
    let svc = Service::new(store.history(), settings.rules, rec.clone());
    let port = svc.listen(settings.port, &settings.token).unwrap();
    check(port == settings.port, "listening on the configured port");

    // ---------- only reachable from this machine ----------

    match lan_ip() {
        Some(ip) => check(
            TcpStream::connect_timeout(&(ip, port).into(), Duration::from_millis(500)).is_err(),
            &format!("not reachable on the network address {ip}"),
        ),
        None => println!("skip  no network address to test against"),
    }

    // ---------- one round: music ----------

    let out = run_fake(&cfg, "round", None);
    check(out.contains("0 refused"), &format!("fake CS2 accepted: {out}"));
    let log = rec.take();
    check(log == ["pause", "resume", "pause", "resume"], &format!("music: pause at live, resume at round end, pause, resume on death ({log:?})"));
    let live = svc.live();
    check(live.connected && live.ever_connected, "shows as connected");
    check(!live.music_paused, "music isn't left paused");

    // ---------- a session: one win, three losses ----------

    let out = run_fake(&cfg, "session", None);
    check(out.contains("0 refused"), &format!("fake CS2 accepted: {out}"));
    let log = rec.take();
    let pauses = log.iter().filter(|l| *l == "pause").count();
    let resumes = log.iter().filter(|l| *l == "resume").count();
    check(pauses > 80 && pauses == resumes, &format!("{pauses} pauses, {resumes} resumes over four matches"));
    let notes: Vec<&String> = log.iter().filter(|l| l.starts_with("notify")).collect();
    check(
        notes.len() == 1 && notes[0].starts_with("notify 3 losses in a row: You've lost 3 in a row."),
        &format!("one tilt warning, after the third loss ({notes:?})"),
    );

    let saved = Store::new(root.join("appdata")).history();
    let outcomes: Vec<(String, Outcome, (u32, u32))> =
        saved.matches.iter().map(|m| (m.map.clone(), m.outcome, m.score)).collect();
    // The single "round" scenario never finished a match, so only the session's four count.
    check(
        outcomes
            == [
                ("de_mirage".into(), Outcome::Win, (13, 9)),
                ("de_nuke".into(), Outcome::Loss, (10, 13)),
                ("de_inferno".into(), Outcome::Loss, (8, 13)),
                ("de_ancient".into(), Outcome::Loss, (11, 13)),
            ],
        &format!("history.json has the four results ({outcomes:?})"),
    );
    let live = svc.live();
    let session = live.session.clone().unwrap();
    check(session.streak == -3 && session.tally.matches == 4, "session shows 4 matches and a 3-loss streak");

    // ---------- an old token ----------

    let out = run_fake(&cfg, "match", Some("0000000000000000aaaaaaaaaaaaaaaa"));
    check(out.contains("0 accepted"), &format!("payloads with the wrong token are refused: {out}"));
    check(svc.live().stale_token, "the app says CS2 needs a restart");
    check(Store::new(root.join("appdata")).history().matches.len() == 4, "and nothing was recorded");
    check(rec.take().is_empty(), "and music wasn't touched");

    // ---------- the recap ----------

    svc.with(|e| e.end_session(now()));
    let log = rec.take();
    check(
        log.len() == 1 && log[0].starts_with("notify Session recap: 4 matches, 1-3, K/D "),
        &format!("ending the session sends the recap ({log:?})"),
    );
    let recap = svc.live().recap.unwrap();
    check(recap.best.as_deref() == Some("de_mirage"), "best map is the one you won");
    let saved = Store::new(root.join("appdata")).history();
    check(saved.sessions.len() == 1 && saved.sessions[0].ended.is_some(), "session saved as ended");

    // ---------- a restart picks up the history ----------

    svc.stop_listening();
    check(TcpStream::connect(("127.0.0.1", port)).is_err(), "listener stops");
    let rec2 = Arc::new(Recorder { store: Store::new(root.join("appdata")), log: Mutex::new(Vec::new()), redraws: AtomicU32::new(0) });
    let svc2 = Service::new(Store::new(root.join("appdata")).history(), Rules::default(), rec2.clone());
    svc2.listen(port, &settings.token).unwrap();
    run_fake(&cfg, "match", None);
    let saved = Store::new(root.join("appdata")).history();
    check(saved.matches.len() == 5 && saved.sessions.len() == 2, "a new session after a restart, history kept");
    check(rec2.redraws.load(Ordering::SeqCst) > 0, "the UI was told to redraw");

    // ---------- uninstall ----------

    check(cs2::remove(&cfg_dir).unwrap(), "removing the cfg");
    check(fs::read_dir(&cfg_dir).unwrap().count() == 0, "CS2's cfg folder is back to how it was");

    let _ = fs::remove_dir_all(&root);
    let fails = FAILS.load(Ordering::SeqCst);
    if fails > 0 {
        println!("\n{fails} failed");
        std::process::exit(1);
    }
    println!("\nall passed");
}
