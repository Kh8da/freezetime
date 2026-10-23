//! Runs the engine for real: the listener feeds it payloads, a slow ticker
//! notices CS2 closing, and a [`Sink`] carries out the effects (media keys,
//! notifications, saving, redrawing the UI).

use crate::engine::{Action, Engine, Live, Rules};
use crate::listener::{Incoming, Listener};
use crate::stats::History;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the ticker wakes. Only time-based things depend on it (CS2 gone,
/// idle sessions), so it can be slow; the CPU cost is one wake-up per interval.
const TICK: Duration = Duration::from_secs(15);

pub trait Sink: Send + Sync + 'static {
    fn pause_music(&self);
    fn resume_music(&self);
    fn notify(&self, title: &str, body: &str);
    fn save_history(&self, history: &History);
    /// The live state changed; redraw.
    fn changed(&self, live: &Live);
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

struct Inner {
    engine: Engine,
    last_live: Option<Live>,
}

pub struct Service {
    inner: Arc<Mutex<Inner>>,
    sink: Arc<dyn Sink>,
    listener: Mutex<Option<Listener>>,
}

fn apply(inner: &mut MutexGuard<Inner>, sink: &Arc<dyn Sink>, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::PauseMusic => sink.pause_music(),
            Action::ResumeMusic => sink.resume_music(),
            Action::Notify { title, body } => sink.notify(&title, &body),
            Action::SaveHistory => sink.save_history(inner.engine.history()),
        }
    }
    let live = inner.engine.live(now());
    if inner.last_live.as_ref() != Some(&live) {
        sink.changed(&live);
        inner.last_live = Some(live);
    }
}

impl Service {
    pub fn new(history: History, rules: Rules, sink: Arc<dyn Sink>) -> Arc<Service> {
        let (engine, actions) = Engine::new(history, rules, now());
        let svc = Arc::new(Service {
            inner: Arc::new(Mutex::new(Inner { engine, last_live: None })),
            sink,
            listener: Mutex::new(None),
        });
        apply(&mut svc.inner.lock().unwrap(), &svc.sink, actions);

        let weak = Arc::downgrade(&svc);
        std::thread::Builder::new()
            .name("ticker".into())
            .spawn(move || loop {
                std::thread::sleep(TICK);
                let Some(svc) = weak.upgrade() else { return };
                svc.with(|e| e.tick(now()));
            })
            .expect("couldn't start the ticker thread");
        svc
    }

    /// Runs `f` on the engine and carries out what it returns.
    pub fn with(&self, f: impl FnOnce(&mut Engine) -> Vec<Action>) {
        let mut inner = self.inner.lock().unwrap();
        let actions = f(&mut inner.engine);
        apply(&mut inner, &self.sink, actions);
    }

    pub fn live(&self) -> Live {
        self.inner.lock().unwrap().engine.live(now())
    }

    pub fn read<T>(&self, f: impl FnOnce(&Engine) -> T) -> T {
        f(&self.inner.lock().unwrap().engine)
    }

    /// (Re)starts listening, for example after the port or token changed.
    pub fn listen(&self, port: u16, token: &str) -> io::Result<u16> {
        let mut slot = self.listener.lock().unwrap();
        if let Some(old) = slot.take() {
            old.stop();
        }
        let inner = self.inner.clone();
        let sink = self.sink.clone();
        let listener = Listener::start(
            port,
            token,
            Arc::new(move |incoming| {
                let mut guard = inner.lock().unwrap();
                let actions = match incoming {
                    Incoming::Payload(p) => guard.engine.on_payload(&p, now()),
                    Incoming::WrongToken => {
                        guard.engine.on_rejected(now());
                        Vec::new()
                    }
                };
                apply(&mut guard, &sink, actions);
            }),
        )?;
        let port = listener.port();
        *slot = Some(listener);
        Ok(port)
    }

    pub fn stop_listening(&self) {
        if let Some(old) = self.listener.lock().unwrap().take() {
            old.stop();
        }
    }
}
