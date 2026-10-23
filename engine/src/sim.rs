//! Builds sequences of GSI payloads shaped like the ones CS2 sends, for the
//! unit tests and the fake CS2 in `harness`. The JSON carries the same sections,
//! field names and quirks as the real feed (`previously` blocks, `player`
//! switching to a teammate after you die, sides swapping at halftime), so the
//! code under test parses exactly what it would in the game.

use serde_json::{json, Value};

pub const STEAMID: &str = "76561198012345678";
const TEAMMATE: &str = "76561198087654321";
/// MR12: halftime after round 12, 13 rounds to win.
const HALF: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// You survive the round.
    Survive,
    /// You die partway through the live phase.
    Die,
}

pub struct Sim {
    token: String,
    pub ts: u64,
    map: String,
    mode: String,
    map_phase: &'static str,
    round_phase: &'static str,
    /// Rounds played so far; `map.round` in the feed.
    round: u32,
    ours: u32,
    theirs: u32,
    team: &'static str,
    health: u32,
    spectating: bool,
    kills: u32,
    deaths: u32,
    assists: u32,
    mvps: u32,
    previously: Option<Value>,
    out: Vec<String>,
}

impl Sim {
    pub fn new(token: &str, ts: u64) -> Sim {
        Sim {
            token: token.to_string(),
            ts,
            map: String::new(),
            mode: String::new(),
            map_phase: "",
            round_phase: "",
            round: 0,
            ours: 0,
            theirs: 0,
            team: "CT",
            health: 100,
            spectating: false,
            kills: 0,
            deaths: 0,
            assists: 0,
            mvps: 0,
            previously: None,
            out: Vec::new(),
        }
    }

    /// Everything sent so far, oldest first.
    pub fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.out)
    }

    fn advance(&mut self, secs: u64) {
        self.ts += secs;
    }

    fn provider(&self) -> Value {
        json!({
            "name": "Counter-Strike: Global Offensive",
            "appid": 730,
            "version": 14054,
            "steamid": STEAMID,
            "timestamp": self.ts
        })
    }

    fn team_obj(&self, side: &str) -> Value {
        let ours_side = self.team == side;
        json!({
            "score": if ours_side { self.ours } else { self.theirs },
            "consecutive_round_losses": 0,
            "timeouts_remaining": 1,
            "matches_won_this_series": 0
        })
    }

    fn emit(&mut self) {
        let mut root = serde_json::Map::new();
        root.insert("provider".into(), self.provider());
        if self.map.is_empty() {
            root.insert(
                "player".into(),
                json!({ "steamid": STEAMID, "name": "kh8da", "activity": "menu" }),
            );
        } else {
            root.insert(
                "map".into(),
                json!({
                    "mode": self.mode,
                    "name": self.map,
                    "phase": self.map_phase,
                    "round": self.round,
                    "team_ct": self.team_obj("CT"),
                    "team_t": self.team_obj("T"),
                    "num_matches_to_win_series": 0
                }),
            );
            if !self.round_phase.is_empty() {
                root.insert("round".into(), json!({ "phase": self.round_phase }));
            }
            let player = if self.spectating {
                json!({
                    "steamid": TEAMMATE,
                    "name": "teammate",
                    "observer_slot": 4,
                    "team": self.team,
                    "activity": "playing",
                    "state": { "health": 64, "armor": 100, "helmet": true, "flashed": 0, "smoked": 0,
                               "burning": 0, "money": 800, "round_kills": 1, "round_killhs": 0, "equip_value": 4700 },
                    "match_stats": { "kills": 11, "assists": 3, "deaths": 7, "mvps": 1, "score": 28 }
                })
            } else {
                json!({
                    "steamid": STEAMID,
                    "name": "kh8da",
                    "observer_slot": 3,
                    "team": self.team,
                    "activity": "playing",
                    "state": { "health": self.health, "armor": 100, "helmet": true, "flashed": 0, "smoked": 0,
                               "burning": 0, "money": 2350, "round_kills": 0, "round_killhs": 0, "equip_value": 5100 },
                    "match_stats": { "kills": self.kills, "assists": self.assists, "deaths": self.deaths,
                                     "mvps": self.mvps, "score": self.kills * 2 + self.assists }
                })
            };
            root.insert("player".into(), player);
        }
        if let Some(prev) = self.previously.take() {
            root.insert("previously".into(), prev);
        }
        root.insert("auth".into(), json!({ "token": self.token }));
        self.out.push(Value::Object(root).to_string());
    }

    /// Main menu, which CS2 also resends as a heartbeat while idle.
    pub fn menu(&mut self) -> &mut Self {
        self.map.clear();
        self.advance(10);
        self.emit();
        self
    }

    /// Loads into a map in warmup.
    pub fn join(&mut self, map: &str, mode: &str) -> &mut Self {
        self.map = map.to_string();
        self.mode = mode.to_string();
        self.map_phase = "warmup";
        self.round_phase = "";
        self.round = 0;
        self.ours = 0;
        self.theirs = 0;
        self.team = "CT";
        self.health = 100;
        self.spectating = false;
        (self.kills, self.deaths, self.assists, self.mvps) = (0, 0, 0, 0);
        self.advance(20);
        self.emit();
        // Warmup deathmatch: kills that the scoreboard resets when the match goes live.
        self.kills = 4;
        self.advance(30);
        self.emit();
        self.kills = 0;
        self
    }

    /// Reconnects to a match you left: the scoreboard and your stats carry on.
    pub fn join_back(&mut self, map: &str) -> &mut Self {
        self.map = map.to_string();
        self.map_phase = "live";
        self.round_phase = "freezetime";
        self.spectating = false;
        self.advance(30);
        self.emit();
        self
    }

    /// Plays one round from freezetime to the end-of-round delay.
    pub fn round(&mut self, won: bool, fate: Fate) -> &mut Self {
        let was = if self.round == 0 { "warmup" } else { self.round_phase };
        self.map_phase = "live";
        self.round_phase = "freezetime";
        self.health = 100;
        self.spectating = false;
        self.previously = Some(json!({ "round": { "phase": was } }));
        self.advance(1);
        self.emit();

        self.round_phase = "live";
        self.previously = Some(json!({ "round": { "phase": "freezetime" } }));
        self.advance(15);
        self.emit();

        // A mid-round update while alive (money spent, utility thrown).
        self.health = 76;
        self.advance(45);
        self.emit();

        if fate == Fate::Die {
            // Sometimes you trade a kill before going down.
            self.kills += u32::from(self.round.is_multiple_of(3));
            self.health = 0;
            self.deaths += 1;
            self.previously = Some(json!({ "player": { "state": { "health": 76 } } }));
            self.advance(25);
            self.emit();
            // Half a second later CS2 moves the camera, and `player` becomes the teammate.
            self.spectating = true;
            self.emit();
        } else {
            self.kills += 1 + u32::from(self.round % 4 == 2);
            self.assists += u32::from(self.round.is_multiple_of(4));
        }

        self.round_phase = "over";
        if won {
            self.ours += 1;
        } else {
            self.theirs += 1;
        }
        self.round += 1;
        self.previously = Some(json!({ "round": { "phase": "live" } }));
        self.advance(40);
        self.emit();

        if self.ours == 13 || self.theirs == 13 {
            return self;
        }
        if self.round == HALF {
            self.map_phase = "intermission";
            self.spectating = false;
            self.advance(5);
            self.emit();
            self.team = if self.team == "CT" { "T" } else { "CT" };
        }
        self
    }

    /// Plays rounds until the match ends `ours`-`theirs`. Dies in every other round.
    pub fn play_to(&mut self, ours: u32, theirs: u32) -> &mut Self {
        let mut i = 0;
        while self.ours < ours || self.theirs < theirs {
            // Interleave wins and losses so the score stays close.
            let want_win = self.ours < ours
                && (self.theirs >= theirs || (self.ours * theirs) <= (self.theirs * ours));
            let fate = if i % 2 == 1 { Fate::Die } else { Fate::Survive };
            self.round(want_win, fate);
            i += 1;
        }
        self.gameover()
    }

    /// The final scoreboard. `player` is you again: CS2 shows your own camera at the end.
    pub fn gameover(&mut self) -> &mut Self {
        self.map_phase = "gameover";
        self.round_phase = "over";
        self.spectating = false;
        self.mvps = self.round / 6;
        self.advance(5);
        self.emit();
        // CS2 keeps resending the final state until you leave.
        self.advance(10);
        self.emit();
        self
    }

    /// A whole match: join, play to the score, back to the menu.
    pub fn match_(&mut self, map: &str, ours: u32, theirs: u32) -> &mut Self {
        self.join(map, "competitive").play_to(ours, theirs).menu()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_payload_parses_and_carries_the_token() {
        let mut sim = Sim::new("tok", 1_000);
        let all = sim.match_("de_inferno", 13, 11).take();
        assert!(all.len() > 100);
        for raw in &all {
            let p = crate::gsi::parse(raw.as_bytes()).unwrap();
            assert_eq!(p.auth.unwrap().token, "tok");
        }
    }
}
