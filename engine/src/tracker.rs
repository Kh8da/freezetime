//! Turns the stream of GSI payloads into what freezetime cares about: which phase
//! the game is in, whether you're alive in a live round, and when a match starts
//! and ends.

use crate::gsi::{MatchStats, Payload};
use serde::{Deserialize, Serialize};

/// Premier and Competitive. Only these count toward win rates and tilt numbers.
pub fn is_ranked(mode: &str) -> bool {
    matches!(mode, "competitive" | "premier")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Win,
    Loss,
    Draw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Main menu, loading, or nothing received yet.
    #[default]
    Menu,
    Warmup,
    Freezetime,
    Live,
    Over,
    Halftime,
    Gameover,
}

/// What the game looks like right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub phase: Phase,
    pub map: String,
    pub mode: String,
    pub round: u32,
    /// Your team's score and the other team's. None until freezetime has seen
    /// you on a team (for example while spectating someone else's match).
    pub score: Option<(u32, u32)>,
    /// None in the menu. Some(false) while dead and spectating a teammate.
    pub alive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub map: String,
    pub mode: String,
    pub outcome: Outcome,
    pub score: (u32, u32),
    pub stats: MatchStats,
    pub started: u64,
    pub ended: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    MatchStarted { map: String, mode: String },
    MatchEnded(Finished),
    /// Back in the menu, or on another map, before the match reached its end.
    /// Not recorded: you may reconnect, and the result arrives then.
    MatchLeft,
}

#[derive(Debug)]
struct Current {
    map: String,
    mode: String,
    started: u64,
    /// The last time `player` was you: your side and your scoreboard line.
    /// Kept because after you die, `player` describes someone else.
    own: Option<(String, MatchStats)>,
    finished: bool,
}

#[derive(Debug, Default)]
pub struct Tracker {
    current: Option<Current>,
    view: View,
}

impl Tracker {
    pub fn view(&self) -> &View {
        &self.view
    }

    /// The one moment music should be paused: the round is live and you're in it.
    pub fn in_live_round(&self) -> bool {
        self.view.phase == Phase::Live && self.view.alive == Some(true)
    }

    /// True between matches, which is when reminders are allowed.
    pub fn between_matches(&self) -> bool {
        matches!(self.view.phase, Phase::Menu | Phase::Gameover)
    }

    pub fn update(&mut self, p: &Payload, now: u64) -> Vec<Event> {
        let mut events = Vec::new();
        let me = p.provider.as_ref().map(|pr| pr.steamid.as_str()).unwrap_or("");
        let own = p.player.as_ref().filter(|pl| !me.is_empty() && pl.steamid == me);

        let Some(map) = &p.map else {
            if let Some(cur) = self.current.take() {
                if !cur.finished {
                    events.push(Event::MatchLeft);
                }
            }
            self.view = View::default();
            return events;
        };

        let new_match = match &self.current {
            None => true,
            Some(cur) => cur.map != map.name || (cur.finished && map.phase != "gameover"),
        };
        if new_match {
            if let Some(old) = self.current.take() {
                if !old.finished {
                    events.push(Event::MatchLeft);
                }
            }
            self.current = Some(Current {
                map: map.name.clone(),
                mode: map.mode.clone(),
                started: now,
                own: None,
                finished: false,
            });
            events.push(Event::MatchStarted { map: map.name.clone(), mode: map.mode.clone() });
        }
        let cur = self.current.as_mut().expect("set above");

        if let Some(pl) = own {
            if let (Some(team), Some(stats)) = (&pl.team, pl.match_stats) {
                cur.own = Some((team.clone(), stats));
            }
        }

        let score = cur.own.as_ref().map(|(team, _)| {
            if team.eq_ignore_ascii_case("T") {
                (map.team_t.score, map.team_ct.score)
            } else {
                (map.team_ct.score, map.team_t.score)
            }
        });

        if map.phase == "gameover" && !cur.finished {
            cur.finished = true;
            if let (Some((ours, theirs)), Some((_, stats))) = (score, &cur.own) {
                events.push(Event::MatchEnded(Finished {
                    map: cur.map.clone(),
                    mode: cur.mode.clone(),
                    outcome: match ours.cmp(&theirs) {
                        std::cmp::Ordering::Greater => Outcome::Win,
                        std::cmp::Ordering::Less => Outcome::Loss,
                        std::cmp::Ordering::Equal => Outcome::Draw,
                    },
                    score: (ours, theirs),
                    stats: *stats,
                    started: cur.started,
                    ended: now,
                }));
            }
        }

        let round_phase = p.round.as_ref().map(|r| r.phase.as_str()).unwrap_or("");
        let phase = match map.phase.as_str() {
            "gameover" => Phase::Gameover,
            "intermission" => Phase::Halftime,
            "warmup" => Phase::Warmup,
            _ => match round_phase {
                "live" => Phase::Live,
                "over" => Phase::Over,
                _ => Phase::Freezetime,
            },
        };
        let alive = match (own, &p.player) {
            // Health, not `activity`: activity reads "menu" whenever Esc is open.
            (Some(pl), _) => Some(pl.state.as_ref().is_some_and(|s| s.health > 0)),
            (None, Some(_)) => Some(false),
            (None, None) => None,
        };
        self.view = View {
            phase,
            map: map.name.clone(),
            mode: map.mode.clone(),
            round: map.round,
            score,
            alive,
        };
        events
    }
}
