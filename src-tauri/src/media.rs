//! Pausing and resuming whatever is playing, through Windows' own media session
//! controls: the same ones behind the volume flyout and the media keys. Works
//! for Spotify, browsers, foobar2000 and anything else that shows up there,
//! without an account or an API key.
//!
//! Only sessions that were playing are paused, and only those are resumed
//! afterwards. Anything you paused yourself stays paused.

use std::sync::mpsc::{channel, Sender};

enum Cmd {
    Pause,
    Resume,
}

pub struct Media {
    tx: Sender<Cmd>,
}

impl Media {
    /// Media calls can take a moment, so they run on their own thread and the
    /// game feed never waits for them.
    pub fn start() -> Media {
        let (tx, rx) = channel::<Cmd>();
        std::thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                let mut paused: Vec<String> = Vec::new();
                for cmd in rx {
                    match cmd {
                        Cmd::Pause => match imp::pause_playing() {
                            Ok(ids) => paused.extend(ids),
                            Err(e) => eprintln!("freezetime: couldn't pause media: {e}"),
                        },
                        Cmd::Resume => {
                            let ids = std::mem::take(&mut paused);
                            if !ids.is_empty() {
                                if let Err(e) = imp::resume(&ids) {
                                    eprintln!("freezetime: couldn't resume media: {e}");
                                }
                            }
                        }
                    }
                }
            })
            .expect("couldn't start the media thread");
        Media { tx }
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }
}

#[cfg(windows)]
mod imp {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::core::Result;

    fn manager() -> Result<Manager> {
        Manager::RequestAsync()?.join()
    }

    /// Pauses every session that is playing. Returns the apps it paused.
    pub fn pause_playing() -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for s in manager()?.GetSessions()? {
            if s.GetPlaybackInfo()?.PlaybackStatus()? != Status::Playing {
                continue;
            }
            if s.TryPauseAsync()?.join().unwrap_or(false) {
                ids.push(s.SourceAppUserModelId()?.to_string());
            }
        }
        Ok(ids)
    }

    /// Resumes the sessions from `ids` that are still paused.
    pub fn resume(ids: &[String]) -> Result<()> {
        for s in manager()?.GetSessions()? {
            let id = s.SourceAppUserModelId()?.to_string();
            if ids.contains(&id) && s.GetPlaybackInfo()?.PlaybackStatus()? == Status::Paused {
                let _ = s.TryPlayAsync()?.join();
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    // Windows first. On other systems the rules still run and the UI still shows
    // the state, but nothing is sent to a player.
    pub fn pause_playing() -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    pub fn resume(_ids: &[String]) -> Result<(), String> {
        Ok(())
    }
}
