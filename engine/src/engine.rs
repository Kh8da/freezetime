//! The decisions: when music pauses and resumes, when a session starts and ends,
//! and when a tilt warning or break reminder goes out. Pure logic: time comes in
//! as an argument and effects go out as [`Action`]s, so all of it is testable
//! without a game, a clock or Windows.

use crate::gsi::Payload;
use crate::stats::{kd, History, MatchRecord, Recap, SessionRecord, Tally};
use crate::tracker::{is_ranked, Event, Outcome, Tracker, View};
use serde::{Deserialize, Serialize};

/// CS2 sends a heartbeat every 10 seconds, even in the menu.
pub const CONNECTED_SECS: u64 = 30;
/// No payload for this long means CS2 was closed, which ends the session.
pub const GONE_SECS: u64 = 5 * 60;
/// Sitting in the menu this long without starting a match also ends it.
pub const IDLE_SECS: u64 = 60 * 60;
/// Out of a match for this long counts as a break.
pub const BREAK_GAP_SECS: u64 = 15 * 60;
const MAX_MESSAGES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rules {
    /// Pause media while you're alive in a live round.
    pub music: bool,
    /// Warn after this many losses in a row. 0 turns it off.
    pub tilt_after: u32,
    /// Remind after this many minutes of play without a break. 0 turns it off.
    pub break_minutes: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { music: true, tilt_after: 3, break_minutes: 90 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Tilt,
    Break,
    Recap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: u64,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PauseMusic,
    ResumeMusic,
    Notify { title: String, body: String },
    SaveHistory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: u64,
    pub started: u64,
    pub tally: Tally,
    pub kd: String,
    /// Positive for wins in a row, negative for losses in a row.
    pub streak: i32,
    /// Newest first.
    pub matches: Vec<MatchRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub connected: bool,
    pub ever_connected: bool,
    pub view: View,
    pub ranked: bool,
    pub music_paused: bool,
    pub session: Option<SessionView>,
    pub messages: Vec<Message>,
    pub recap: Option<Recap>,
    /// CS2 is sending, but with an old token: it was started before the cfg changed.
    pub stale_token: bool,
}

struct Session {
    id: u64,
    /// Last match start or end, for the idle timeout.
    last_activity: u64,
    in_history: bool,
}

pub struct Engine {
    rules: Rules,
    tracker: Tracker,
    history: History,
    session: Option<Session>,
    last_payload: Option<u64>,
    last_rejected: Option<u64>,
    music_paused: bool,
    /// When the current stretch of play began, and when the next reminder is due.
    play_since: Option<u64>,
    next_break: u64,
    /// Last time a payload showed you on a map.
    last_in_match: u64,
    messages: Vec<Message>,
    next_message_id: u64,
    recap: Option<Recap>,
}

impl Engine {
    /// `now` is used to close a session left open by a previous run.
    pub fn new(mut history: History, rules: Rules, now: u64) -> (Engine, Vec<Action>) {
        let mut actions = Vec::new();
        let mut session = None;
        if let Some(open) = history.sessions.iter_mut().find(|s| s.ended.is_none()) {
            let last = history.matches.iter().filter(|m| m.session == open.id).map(|m| m.ended).max().unwrap_or(open.id);
            if now.saturating_sub(last) < IDLE_SECS {
                session = Some(Session { id: open.id, last_activity: last, in_history: true });
            } else {
                open.ended = Some(last);
                actions.push(Action::SaveHistory);
            }
        }
        let engine = Engine {
            rules,
            tracker: Tracker::default(),
            history,
            session,
            last_payload: None,
            last_rejected: None,
            music_paused: false,
            play_since: None,
            next_break: 0,
            last_in_match: 0,
            messages: Vec::new(),
            next_message_id: 1,
            recap: None,
        };
        (engine, actions)
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn rules(&self) -> Rules {
        self.rules
    }

    pub fn set_rules(&mut self, rules: Rules) -> Vec<Action> {
        self.rules = rules;
        let mut out = Vec::new();
        self.sync_music(&mut out);
        out
    }

    pub fn on_rejected(&mut self, now: u64) {
        self.last_rejected = Some(now);
    }

    pub fn on_payload(&mut self, p: &Payload, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        self.last_payload = Some(now);
        self.last_rejected = None;
        // Match and session times come from the game's own timestamp (the same
        // clock in normal use; it also lets a recorded match replay with its real
        // timing). Anything more than a day off is ignored.
        let now = p.provider.as_ref().map(|pr| pr.timestamp).filter(|t| t.abs_diff(now) < 86_400).unwrap_or(now);

        let events = self.tracker.update(p, now);
        if !self.tracker.view().map.is_empty() {
            self.last_in_match = now;
        }
        let mut notes: Vec<(Kind, String, String)> = Vec::new();

        for event in events {
            match event {
                Event::MatchStarted { .. } => {
                    self.start_session(now);
                    if self.play_since.is_none() {
                        self.play_since = Some(now);
                        self.next_break = now + self.rules.break_minutes as u64 * 60;
                    }
                }
                Event::MatchEnded(f) => {
                    if let Some(s) = &mut self.session {
                        s.last_activity = now;
                    }
                    if is_ranked(&f.mode) {
                        let session = self.ensure_session_in_history(now, &mut out);
                        self.history.matches.push(MatchRecord {
                            session,
                            map: f.map,
                            mode: f.mode,
                            outcome: f.outcome,
                            score: f.score,
                            kills: f.stats.kills,
                            deaths: f.stats.deaths,
                            assists: f.stats.assists,
                            mvps: f.stats.mvps,
                            started: f.started,
                            ended: f.ended,
                        });
                        out.push(Action::SaveHistory);
                        if f.outcome == Outcome::Loss {
                            if let Some(body) = self.history.tilt_message(session, self.rules.tilt_after) {
                                let streak = self.history.loss_streak(session);
                                notes.push((Kind::Tilt, format!("{streak} losses in a row"), body));
                            }
                        }
                    }
                    if let Some(n) = self.break_due(now) {
                        notes.push(n);
                    }
                }
                Event::MatchLeft => {
                    if let Some(s) = &mut self.session {
                        s.last_activity = now;
                    }
                    if let Some(n) = self.break_due(now) {
                        notes.push(n);
                    }
                }
            }
        }

        if !notes.is_empty() {
            // One notification per match end, even when a tilt warning and a
            // break reminder are both due.
            let title = notes[0].1.clone();
            let body = notes.iter().map(|n| n.2.as_str()).collect::<Vec<_>>().join(" ");
            for (kind, title, body) in notes {
                self.push_message(kind, title, body, now);
            }
            out.push(Action::Notify { title, body });
        }

        self.sync_music(&mut out);
        out
    }

    /// Called every so often with no payload, to notice CS2 closing and idle sessions.
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        let gone = self.last_payload.is_some_and(|t| now.saturating_sub(t) >= GONE_SECS);
        if gone && !self.tracker.view().map.is_empty() {
            // CS2 vanished mid-match (crash, alt-F4). Forget the match.
            self.tracker = Tracker::default();
        }
        if let Some(s) = &self.session {
            let idle = self.tracker.between_matches() && now.saturating_sub(s.last_activity) >= IDLE_SECS;
            if gone || idle {
                let end = if gone { self.last_payload.unwrap_or(now) } else { now };
                self.finish_session(end, &mut out);
            }
        }
        if self.play_since.is_some() && now.saturating_sub(self.last_in_match) >= BREAK_GAP_SECS {
            self.play_since = None;
        }
        if gone {
            self.sync_music(&mut out);
        }
        out
    }

    pub fn end_session(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        if self.session.is_some() {
            self.finish_session(now, &mut out);
        }
        out
    }

    pub fn dismiss(&mut self, id: u64) {
        self.messages.retain(|m| m.id != id);
    }

    pub fn dismiss_recap(&mut self) {
        self.recap = None;
        self.messages.retain(|m| m.kind != Kind::Recap);
    }

    pub fn live(&self, now: u64) -> Live {
        let connected = self.last_payload.is_some_and(|t| now.saturating_sub(t) < CONNECTED_SECS);
        let view = if connected { self.tracker.view().clone() } else { View::default() };
        let session = self.session.as_ref().map(|s| {
            let matches: Vec<MatchRecord> = self.history.session_matches(s.id).cloned().collect();
            let tally = Tally::of(&matches);
            let streak = match matches.last().map(|m| m.outcome) {
                Some(o @ (Outcome::Win | Outcome::Loss)) => {
                    let n = matches.iter().rev().take_while(|m| m.outcome == o).count() as i32;
                    if o == Outcome::Win { n } else { -n }
                }
                _ => 0,
            };
            SessionView {
                id: s.id,
                started: s.id,
                kd: kd(tally.kills, tally.deaths),
                tally,
                streak,
                matches: matches.into_iter().rev().collect(),
            }
        });
        Live {
            connected,
            ever_connected: self.last_payload.is_some(),
            ranked: is_ranked(&view.mode),
            view,
            music_paused: self.music_paused,
            session,
            messages: self.messages.iter().rev().cloned().collect(),
            recap: self.recap.clone(),
            stale_token: self.last_rejected.is_some_and(|t| now.saturating_sub(t) < CONNECTED_SECS),
        }
    }

    fn sync_music(&mut self, out: &mut Vec<Action>) {
        let connected = self.last_payload.is_some();
        let want = self.rules.music && connected && self.tracker.in_live_round();
        if want != self.music_paused {
            self.music_paused = want;
            out.push(if want { Action::PauseMusic } else { Action::ResumeMusic });
        }
    }

    fn start_session(&mut self, now: u64) {
        match &mut self.session {
            Some(s) => s.last_activity = now,
            None => {
                self.recap = None;
                self.messages.retain(|m| m.kind != Kind::Recap);
                self.session = Some(Session { id: now, last_activity: now, in_history: false });
            }
        }
    }

    /// Sessions only enter the history file once they have a ranked match.
    fn ensure_session_in_history(&mut self, now: u64, out: &mut Vec<Action>) -> u64 {
        if self.session.is_none() {
            self.start_session(now);
        }
        let s = self.session.as_mut().expect("set above");
        if !s.in_history {
            s.in_history = true;
            self.history.sessions.push(SessionRecord { id: s.id, ended: None });
            out.push(Action::SaveHistory);
        }
        s.id
    }

    fn finish_session(&mut self, end: u64, out: &mut Vec<Action>) {
        let Some(s) = self.session.take() else { return };
        if !s.in_history {
            return;
        }
        if let Some(rec) = self.history.sessions.iter_mut().find(|r| r.id == s.id) {
            rec.ended = Some(end);
        }
        out.push(Action::SaveHistory);
        let recap = self.history.recap(s.id);
        if recap.tally.matches == 0 {
            return;
        }
        let body = recap.summary();
        self.push_message(Kind::Recap, "Session recap".into(), body.clone(), end);
        out.push(Action::Notify { title: "Session recap".into(), body });
        self.recap = Some(recap);
    }

    fn break_due(&mut self, now: u64) -> Option<(Kind, String, String)> {
        let since = self.play_since?;
        if self.rules.break_minutes == 0 || now < self.next_break {
            return None;
        }
        self.next_break = now + self.rules.break_minutes as u64 * 60;
        let mins = (now - since) / 60;
        let played = match (mins / 60, mins % 60) {
            (0, m) => format!("{m} minutes"),
            (1, 0) => "1 hour".to_string(),
            (h, 0) => format!("{h} hours"),
            (1, m) => format!("1 hour {m} minutes"),
            (h, m) => format!("{h} hours {m} minutes"),
        };
        Some((
            Kind::Break,
            "Time for a break".into(),
            format!("You've played for {played} without a break. Step away for a few minutes before the next queue."),
        ))
    }

    fn push_message(&mut self, kind: Kind, title: String, body: String, at: u64) {
        // A newer message of the same kind replaces the old one.
        self.messages.retain(|m| m.kind != kind);
        self.messages.push(Message { id: self.next_message_id, kind, title, body, at });
        self.next_message_id += 1;
        if self.messages.len() > MAX_MESSAGES {
            self.messages.remove(0);
        }
    }
}

