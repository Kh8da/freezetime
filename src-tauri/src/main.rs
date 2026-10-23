#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod media;
mod notify;

use engine::cs2::{self, CfgStatus};
use engine::engine::{Live, Rules};
use engine::service::{now, Service, Sink};
use engine::stats::{kd, map_lines, History, MapLine, Recap, StreakRow, Tally};
use engine::store::{Settings, Store, DEFAULT_PORT};
use engine::tracker::Phase;
use media::Media;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;

const TRAY_ID: &str = "tray";

struct AppSink {
    app: AppHandle,
    media: Media,
    store: Store,
    sound: AtomicBool,
}

impl Sink for AppSink {
    fn pause_music(&self) {
        self.media.pause();
    }
    fn resume_music(&self) {
        self.media.resume();
    }
    fn notify(&self, title: &str, body: &str) {
        notify::show(title, body, self.sound.load(Ordering::Relaxed));
    }
    fn save_history(&self, history: &History) {
        if let Err(e) = self.store.save_history(history) {
            eprintln!("freezetime: {e}");
        }
    }
    fn changed(&self, live: &Live) {
        let _ = self.app.emit("live", live);
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_tooltip(Some(tray_text(live)));
        }
    }
}

fn tray_text(live: &Live) -> String {
    if !live.connected {
        return "freezetime: waiting for CS2".into();
    }
    let v = &live.view;
    let what = match v.phase {
        Phase::Menu => return "freezetime: CS2 is in the menu".into(),
        Phase::Warmup => "warmup",
        Phase::Freezetime => "freeze time",
        Phase::Live if v.alive == Some(false) => "dead, spectating",
        Phase::Live => "round live",
        Phase::Over => "round over",
        Phase::Halftime => "halftime",
        Phase::Gameover => "match over",
    };
    let score = v.score.map(|(a, b)| format!(" {a}:{b}")).unwrap_or_default();
    format!("freezetime: {}{score}, {what}", engine::stats::pretty_map(&v.map))
}

struct AppState {
    svc: Arc<Service>,
    sink: Arc<AppSink>,
    store: Store,
    settings: Mutex<Settings>,
    listen_error: Mutex<Option<String>>,
}

impl AppState {
    fn cfg_dir(&self) -> Option<PathBuf> {
        let picked = self.settings.lock().unwrap().cfg_dir.clone();
        if let Some(dir) = picked.map(PathBuf::from).filter(|d| d.is_dir()) {
            return Some(dir);
        }
        cs2::find_cfg_dir(&cs2::steam_candidates())
    }

    fn setup(&self) -> SetupDto {
        let s = self.settings.lock().unwrap().clone();
        let dir = self.cfg_dir();
        SetupDto {
            status: dir.as_ref().map(|d| cs2::status(d, s.port, &s.token)),
            cfg_path: dir.map(|d| d.join(cs2::CFG_NAME).display().to_string()),
            picked: s.cfg_dir.is_some(),
            listen_error: self.listen_error.lock().unwrap().clone(),
            port: s.port,
        }
    }

    fn listen(&self) {
        let (port, token) = {
            let s = self.settings.lock().unwrap();
            (s.port, s.token.clone())
        };
        *self.listen_error.lock().unwrap() = match self.svc.listen(port, &token) {
            Ok(_) => None,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                Some(format!("Port {port} is already used by another program."))
            }
            Err(e) => Some(format!("Couldn't listen on port {port}: {e}")),
        };
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupDto {
    /// None when CS2 couldn't be found.
    status: Option<CfgStatus>,
    cfg_path: Option<String>,
    /// The CS2 folder was picked by hand.
    picked: bool,
    listen_error: Option<String>,
    port: u16,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prefs {
    music: bool,
    tilt_after: u32,
    break_minutes: u32,
    sound: bool,
    autostart: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateDto {
    platform: &'static str,
    version: &'static str,
    live: Live,
    setup: SetupDto,
    prefs: Prefs,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryDto {
    sessions: Vec<Recap>,
    overall: Tally,
    kd: String,
    win_rate: Option<u32>,
    streaks: Vec<StreakRow>,
    maps: Vec<MapLine>,
}

#[tauri::command]
fn get_state(state: State<AppState>) -> StateDto {
    let s = state.settings.lock().unwrap().clone();
    StateDto {
        platform: std::env::consts::OS,
        version: env!("CARGO_PKG_VERSION"),
        live: state.svc.live(),
        setup: state.setup(),
        prefs: Prefs {
            music: s.rules.music,
            tilt_after: s.rules.tilt_after,
            break_minutes: s.rules.break_minutes,
            sound: s.sound,
            autostart: autostart::enabled(),
        },
    }
}

#[tauri::command]
fn get_history(state: State<AppState>) -> HistoryDto {
    state.svc.read(|e| {
        let h = e.history();
        let overall = h.overall();
        HistoryDto {
            sessions: h.past_sessions(),
            kd: kd(overall.kills, overall.deaths),
            win_rate: overall.win_rate(),
            overall,
            streaks: h.streak_table(),
            maps: map_lines(&h.matches),
        }
    })
}

#[tauri::command]
fn install_cfg(state: State<AppState>) -> Result<SetupDto, String> {
    let dir = state.cfg_dir().ok_or("couldn't find CS2. Pick its folder instead.")?;
    let (port, token) = {
        let s = state.settings.lock().unwrap();
        (s.port, s.token.clone())
    };
    cs2::install(&dir, port, &token).map_err(|e| format!("couldn't write the file: {e}"))?;
    Ok(state.setup())
}

#[tauri::command]
fn remove_cfg(state: State<AppState>) -> Result<SetupDto, String> {
    if let Some(dir) = state.cfg_dir() {
        cs2::remove(&dir).map_err(|e| format!("couldn't remove the file: {e}"))?;
    }
    Ok(state.setup())
}

#[tauri::command]
async fn pick_cs2_folder(app: AppHandle) -> Result<Option<SetupDto>, String> {
    let Some(picked) = app.dialog().file().set_title("Find your CS2 folder (Counter-Strike Global Offensive)").blocking_pick_folder() else {
        return Ok(None);
    };
    let picked = picked.into_path().map_err(|e| e.to_string())?;
    let dir = cs2::cfg_dir_from_pick(&picked).ok_or("that isn't a CS2 folder. Look for the one called Counter-Strike Global Offensive.")?;
    let state = app.state::<AppState>();
    {
        let mut s = state.settings.lock().unwrap();
        s.cfg_dir = Some(dir.display().to_string());
        state.store.save_settings(&s)?;
    }
    Ok(Some(state.setup()))
}

/// Moves to a free port after the default one turned out to be taken, and
/// rewrites the cfg so CS2 follows (after a restart).
#[tauri::command]
fn change_port(state: State<AppState>) -> Result<SetupDto, String> {
    let current = state.settings.lock().unwrap().port;
    let port = (DEFAULT_PORT..DEFAULT_PORT + 200)
        .chain(std::iter::once(0))
        .filter(|p| *p != current)
        .find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok().and_then(|l| l.local_addr().ok()).map(|a| a.port()))
        .ok_or("couldn't find a free port.")?;
    {
        let mut s = state.settings.lock().unwrap();
        s.port = port;
        state.store.save_settings(&s)?;
    }
    state.listen();
    if let Some(dir) = state.cfg_dir() {
        if cs2::status(&dir, current, &state.settings.lock().unwrap().token) != CfgStatus::Missing {
            let s = state.settings.lock().unwrap().clone();
            cs2::install(&dir, s.port, &s.token).map_err(|e| format!("couldn't update the file: {e}"))?;
        }
    }
    Ok(state.setup())
}

#[tauri::command]
fn set_prefs(app: AppHandle, state: State<AppState>, prefs: Prefs) -> Result<(), String> {
    let rules = Rules {
        music: prefs.music,
        tilt_after: prefs.tilt_after.min(10),
        break_minutes: prefs.break_minutes.min(600),
    };
    {
        let mut s = state.settings.lock().unwrap();
        s.rules = rules;
        s.sound = prefs.sound;
        state.store.save_settings(&s)?;
    }
    state.sink.sound.store(prefs.sound, Ordering::Relaxed);
    state.svc.with(|e| e.set_rules(rules, now()));
    sync_tray_music(&app, rules.music);
    if prefs.autostart != autostart::enabled() {
        autostart::set(prefs.autostart)?;
    }
    Ok(())
}

#[tauri::command]
fn end_session(state: State<AppState>) {
    state.svc.with(|e| e.end_session(now()));
}

#[tauri::command]
fn dismiss(state: State<AppState>, id: u64) {
    state.svc.with(|e| {
        e.dismiss(id);
        Vec::new()
    });
}

#[tauri::command]
fn dismiss_recap(state: State<AppState>) {
    state.svc.with(|e| {
        e.dismiss_recap();
        Vec::new()
    });
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    quit(&app);
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn quit(app: &AppHandle) {
    // Never leave music paused behind.
    let state = app.state::<AppState>();
    state.svc.stop_listening();
    state.sink.media.resume();
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.exit(0);
}

fn sync_tray_music(app: &AppHandle, on: bool) {
    if let Some(item) = app.try_state::<CheckMenuItem<tauri::Wry>>() {
        let _ = item.set_checked(on);
    }
}

fn toggle_music_from_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let rules = {
        let mut s = state.settings.lock().unwrap();
        s.rules.music = !s.rules.music;
        let _ = state.store.save_settings(&s);
        s.rules
    };
    state.svc.with(|e| e.set_rules(rules, now()));
    sync_tray_music(app, rules.music);
    let _ = app.emit("prefs-changed", ());
}

fn main() {
    let minimized = std::env::args().any(|a| a == "--minimized");
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let dir = app.path().app_data_dir()?;
            let store = Store::new(dir.clone());
            let settings = store.settings();
            notify::init(include_bytes!("../icons/128x128.png"), &dir);

            let sink = Arc::new(AppSink {
                app: app.handle().clone(),
                media: Media::start(),
                store: Store::new(dir.clone()),
                sound: AtomicBool::new(settings.sound),
            });
            let svc = Service::new(store.history(), settings.rules, sink.clone());
            let state = AppState {
                svc,
                sink,
                store,
                settings: Mutex::new(settings.clone()),
                listen_error: Mutex::new(None),
            };
            state.listen();
            // A cfg left from an earlier settings file (lost or reset) is brought up to date.
            if let Some(dir) = state.cfg_dir() {
                if cs2::status(&dir, settings.port, &settings.token) == CfgStatus::Outdated {
                    let _ = cs2::install(&dir, settings.port, &settings.token);
                }
            }
            app.manage(state);

            let open = MenuItem::with_id(app, "open", "Open freezetime", true, None::<&str>)?;
            let music = CheckMenuItem::with_id(app, "music", "Pause music during rounds", true, settings.rules.music, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &music, &PredefinedMenuItem::separator(app)?, &quit_item])?;
            app.manage(music);
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .tooltip("freezetime: waiting for CS2")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_window(app),
                    "music" => toggle_music_from_tray(app),
                    "quit" => quit(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_window(tray.app_handle());
                    }
                })
                .build(app)?;

            if !minimized {
                show_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing hides to the tray; freezetime has to keep running to do its job.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_history,
            install_cfg,
            remove_cfg,
            pick_cs2_folder,
            change_port,
            set_prefs,
            end_session,
            dismiss,
            dismiss_recap,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running freezetime");
}
