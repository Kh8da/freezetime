//! Match history and the numbers built from it: tilt odds, session recaps and
//! all-time totals. Only Premier and Competitive matches are recorded.

use crate::tracker::Outcome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fewer matches than this and a percentage says more about luck than about you.
pub const MIN_SAMPLE: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchRecord {
    pub session: u64,
    pub map: String,
    pub mode: String,
    pub outcome: Outcome,
    pub score: (u32, u32),
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub mvps: u32,
    pub started: u64,
    pub ended: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// Unix time the session started; doubles as its id.
    pub id: u64,
    pub ended: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct History {
    pub matches: Vec<MatchRecord>,
    pub sessions: Vec<SessionRecord>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub kills: u32,
    pub deaths: u32,
}

impl Tally {
    pub fn add(&mut self, m: &MatchRecord) {
        self.matches += 1;
        match m.outcome {
            Outcome::Win => self.wins += 1,
            Outcome::Loss => self.losses += 1,
            Outcome::Draw => self.draws += 1,
        }
        self.kills += m.kills;
        self.deaths += m.deaths;
    }

    pub fn of<'a>(matches: impl IntoIterator<Item = &'a MatchRecord>) -> Tally {
        let mut t = Tally::default();
        matches.into_iter().for_each(|m| t.add(m));
        t
    }

    /// Whole percent of matches won. Draws count as not won.
    pub fn win_rate(&self) -> Option<u32> {
        (self.matches > 0).then(|| (self.wins * 100 + self.matches / 2) / self.matches)
    }
}

/// Kills per death, as shown to the user: "1.24". With no deaths it's just the kills.
pub fn kd(kills: u32, deaths: u32) -> String {
    format!("{:.2}", kills as f64 / deaths.max(1) as f64)
}

impl History {
    pub fn session_matches(&self, session: u64) -> impl Iterator<Item = &MatchRecord> {
        self.matches.iter().filter(move |m| m.session == session)
    }

    /// Losses in a row at the end of this session. A win or a draw ends a streak,
    /// and so does a new session: yesterday's losses don't carry into today.
    pub fn loss_streak(&self, session: u64) -> u32 {
        let list: Vec<_> = self.session_matches(session).collect();
        list.iter().rev().take_while(|m| m.outcome == Outcome::Loss).count() as u32
    }

    /// How matches went right after at least `k` losses in a row in the same session.
    pub fn after_streak(&self, k: u32) -> Tally {
        let mut out = Tally::default();
        let mut streak: BTreeMap<u64, u32> = BTreeMap::new();
        for m in &self.matches {
            let s = streak.entry(m.session).or_insert(0);
            if *s >= k {
                out.add(m);
            }
            *s = if m.outcome == Outcome::Loss { *s + 1 } else { 0 };
        }
        out
    }

    pub fn overall(&self) -> Tally {
        Tally::of(&self.matches)
    }

    /// The warning shown after a loss, or None when the streak is below `threshold`.
    pub fn tilt_message(&self, session: u64, threshold: u32) -> Option<String> {
        let streak = self.loss_streak(session);
        if threshold == 0 || streak < threshold {
            return None;
        }
        let lead = format!("You've lost {streak} in a row.");
        let overall = self.overall();
        // Use the longest streak length you have enough history for, never below 2.
        let numbers = (2..=streak).rev().map(|k| (k, self.after_streak(k))).find(|(_, t)| t.matches >= MIN_SAMPLE);
        let overall_rate = overall.win_rate().filter(|_| overall.matches >= MIN_SAMPLE * 2);
        Some(match (numbers, overall_rate) {
            (Some((k, t)), Some(all)) => {
                let rate = t.win_rate().unwrap_or(0);
                format!(
                    "{lead} Your win rate after {k} losses in a row is {rate}% ({} matches), against {all}% overall.",
                    t.matches
                )
            }
            (Some((k, t)), None) => {
                format!("{lead} Your win rate after {k} losses in a row is {}% ({} matches).", t.win_rate().unwrap_or(0), t.matches)
            }
            (None, Some(all)) => format!(
                "{lead} There isn't enough history yet to know how you play after a streak like this. Overall you win {all}%."
            ),
            (None, None) => format!(
                "{lead} After a few more sessions, this is where you'll see your own win rate after streaks like this."
            ),
        })
    }

    /// Rows for the "your numbers" table: win rate after 1, 2, 3 and 4+ losses in a row.
    pub fn streak_table(&self) -> Vec<StreakRow> {
        (1..=4)
            .map(|k| {
                let t = self.after_streak(k);
                StreakRow { after: k, matches: t.matches, win_rate: t.win_rate().filter(|_| t.matches >= MIN_SAMPLE) }
            })
            .collect()
    }

    pub fn recap(&self, session: u64) -> Recap {
        let matches: Vec<MatchRecord> = self.session_matches(session).cloned().collect();
        let tally = Tally::of(&matches);
        let by_map = map_lines(&matches);
        // A best map needs a win and a worst map a loss: in an 0-3 session
        // nothing was "best". With one map there's nothing to compare.
        let (mut best, mut worst) = (None, None);
        if by_map.len() >= 2 {
            let b = &by_map[0];
            let w = &by_map[by_map.len() - 1];
            // Two maps that went exactly the same way have no best or worst.
            let same = (b.tally.wins, b.tally.losses) == (w.tally.wins, w.tally.losses) && b.kd == w.kd;
            if !same {
                best = (b.tally.wins > 0).then(|| b.map.clone());
                worst = (w.tally.losses > 0).then(|| w.map.clone());
            }
        }
        let first = matches.first().map(|m| m.started).unwrap_or(session);
        let last = matches.last().map(|m| m.ended).unwrap_or(first);
        Recap {
            session,
            started: first,
            ended: last,
            kd: kd(tally.kills, tally.deaths),
            tally,
            longest_loss_streak: longest_loss_streak(&matches),
            maps: by_map,
            best,
            worst,
            matches,
        }
    }

    /// Finished sessions with at least one match, newest first.
    pub fn past_sessions(&self) -> Vec<Recap> {
        let mut out: Vec<Recap> = self
            .sessions
            .iter()
            .filter(|s| s.ended.is_some())
            .map(|s| self.recap(s.id))
            .filter(|r| r.tally.matches > 0)
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.started));
        out
    }
}

/// Per-map totals, best first: net wins, then win rate, then K/D.
pub fn map_lines<'a>(matches: impl IntoIterator<Item = &'a MatchRecord>) -> Vec<MapLine> {
    let mut maps: BTreeMap<&str, Tally> = BTreeMap::new();
    for m in matches {
        maps.entry(m.map.as_str()).or_default().add(m);
    }
    let mut lines: Vec<MapLine> = maps
        .into_iter()
        .map(|(map, t)| MapLine { map: map.to_string(), tally: t, kd: kd(t.kills, t.deaths) })
        .collect();
    lines.sort_by(|a, b| {
        let key = |l: &MapLine| {
            (
                l.tally.wins as i64 - l.tally.losses as i64,
                l.tally.win_rate().unwrap_or(0),
                (l.tally.kills * 100) / l.tally.deaths.max(1),
            )
        };
        key(b).cmp(&key(a)).then_with(|| a.map.cmp(&b.map))
    });
    lines
}

fn longest_loss_streak(matches: &[MatchRecord]) -> u32 {
    let (mut best, mut cur) = (0, 0);
    for m in matches {
        cur = if m.outcome == Outcome::Loss { cur + 1 } else { 0 };
        best = best.max(cur);
    }
    best
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreakRow {
    pub after: u32,
    pub matches: u32,
    pub win_rate: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapLine {
    pub map: String,
    pub tally: Tally,
    pub kd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recap {
    pub session: u64,
    pub started: u64,
    pub ended: u64,
    pub tally: Tally,
    pub kd: String,
    pub longest_loss_streak: u32,
    pub maps: Vec<MapLine>,
    pub best: Option<String>,
    pub worst: Option<String>,
    pub matches: Vec<MatchRecord>,
}

impl Recap {
    /// One line for the notification when a session ends.
    pub fn summary(&self) -> String {
        let t = &self.tally;
        let mut s = format!(
            "{} {}, {}-{}{}, K/D {}.",
            t.matches,
            if t.matches == 1 { "match" } else { "matches" },
            t.wins,
            t.losses,
            if t.draws > 0 { format!("-{}", t.draws) } else { String::new() },
            self.kd
        );
        match (&self.best, &self.worst) {
            (Some(b), Some(w)) => s.push_str(&format!(" Best map {}, worst {}.", pretty_map(b), pretty_map(w))),
            (Some(b), None) => s.push_str(&format!(" Best map {}.", pretty_map(b))),
            (None, Some(w)) => s.push_str(&format!(" Worst map {}.", pretty_map(w))),
            (None, None) => {}
        }
        s
    }
}

/// "de_ancient" -> "Ancient". Workshop and unknown maps keep their file name.
pub fn pretty_map(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let short = base.strip_prefix("de_").or_else(|| base.strip_prefix("cs_")).unwrap_or(base);
    let mut c = short.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(session: u64, map: &str, outcome: Outcome, kills: u32, deaths: u32) -> MatchRecord {
        MatchRecord {
            session,
            map: map.into(),
            mode: "competitive".into(),
            outcome,
            score: match outcome {
                Outcome::Win => (13, 8),
                Outcome::Loss => (8, 13),
                Outcome::Draw => (12, 12),
            },
            kills,
            deaths,
            assists: 3,
            mvps: 2,
            started: session,
            ended: session + 2400,
        }
    }

    fn hist(sessions: &[&[Outcome]]) -> History {
        let mut h = History::default();
        for (i, outcomes) in sessions.iter().enumerate() {
            let id = 1_000_000 + i as u64 * 100_000;
            h.sessions.push(SessionRecord { id, ended: Some(id + 50_000) });
            for o in outcomes.iter() {
                h.matches.push(rec(id, "de_mirage", *o, 18, 16));
            }
        }
        h
    }

    use Outcome::{Draw as D, Loss as L, Win as W};

    #[test]
    fn streak_resets_on_win_draw_and_new_session() {
        let h = hist(&[&[L, L, L], &[W, L, L]]);
        assert_eq!(h.loss_streak(1_000_000), 3);
        assert_eq!(h.loss_streak(1_100_000), 2);
        let h = hist(&[&[L, L, D]]);
        assert_eq!(h.loss_streak(1_000_000), 0);
    }

    #[test]
    fn after_streak_counts_only_matches_that_followed_a_streak() {
        // Session 1: L L W   -> the W followed 2 losses.
        // Session 2: L L L L -> 3rd L followed 2, 4th followed 3.
        // Session 3: L W     -> streak doesn't carry over from session 2.
        let h = hist(&[&[L, L, W], &[L, L, L, L], &[L, W]]);
        let t2 = h.after_streak(2);
        assert_eq!((t2.matches, t2.wins), (3, 1));
        let t3 = h.after_streak(3);
        assert_eq!((t3.matches, t3.wins), (1, 0));
        let t1 = h.after_streak(1);
        assert_eq!((t1.matches, t1.wins), (6, 2));
    }

    #[test]
    fn tilt_message_uses_your_numbers_when_there_are_enough() {
        // 8 sessions of L L L then a result: after 3 losses you went 3 W, 5 L.
        let mut sessions: Vec<Vec<Outcome>> = Vec::new();
        for i in 0..8 {
            let mut s = vec![W, W, L, L, L];
            s.push(if i < 3 { W } else { L });
            sessions.push(s);
        }
        sessions.push(vec![L, L, L]);
        let refs: Vec<&[Outcome]> = sessions.iter().map(|v| v.as_slice()).collect();
        let h = hist(&refs);
        let msg = h.tilt_message(1_800_000, 3).unwrap();
        let after3 = h.after_streak(3);
        assert_eq!((after3.matches, after3.wins), (8, 3));
        assert_eq!(
            msg,
            format!(
                "You've lost 3 in a row. Your win rate after 3 losses in a row is 38% (8 matches), against {}% overall.",
                h.overall().win_rate().unwrap()
            )
        );
    }

    #[test]
    fn tilt_message_falls_back_to_shorter_streaks_then_admits_it_doesnt_know() {
        // Plenty of history after 2 losses, none after 4.
        let mut sessions: Vec<Vec<Outcome>> = (0..9).map(|_| vec![L, L, W]).collect();
        sessions.push(vec![L, L, L, L]);
        let refs: Vec<&[Outcome]> = sessions.iter().map(|v| v.as_slice()).collect();
        let h = hist(&refs);
        let msg = h.tilt_message(1_900_000, 3).unwrap();
        assert!(msg.starts_with("You've lost 4 in a row. Your win rate after 2 losses in a row is "), "{msg}");

        let h = hist(&[&[L, L, L]]);
        assert_eq!(
            h.tilt_message(1_000_000, 3).unwrap(),
            "You've lost 3 in a row. After a few more sessions, this is where you'll see your own win rate after streaks like this."
        );
        assert_eq!(h.tilt_message(1_000_000, 4), None);
        assert_eq!(h.tilt_message(1_000_000, 0), None);
    }

    #[test]
    fn recap_finds_best_and_worst_maps() {
        let id = 5_000;
        let mut h = History::default();
        h.sessions.push(SessionRecord { id, ended: Some(9_000) });
        h.matches.push(rec(id, "de_mirage", W, 25, 14));
        h.matches.push(rec(id, "de_nuke", L, 12, 19));
        h.matches.push(rec(id, "de_mirage", W, 20, 15));
        h.matches.push(rec(id, "de_inferno", L, 17, 18));
        h.matches.push(rec(id, "de_nuke", L, 9, 18));
        let r = h.recap(id);
        assert_eq!((r.tally.matches, r.tally.wins, r.tally.losses), (5, 2, 3));
        assert_eq!(r.kd, "0.99");
        assert_eq!(r.best.as_deref(), Some("de_mirage"));
        assert_eq!(r.worst.as_deref(), Some("de_nuke"));
        assert_eq!(r.longest_loss_streak, 2);
        assert_eq!(r.summary(), "5 matches, 2-3, K/D 0.99. Best map Mirage, worst Nuke.");
    }

    #[test]
    fn a_session_without_wins_has_no_best_map() {
        let id = 5_000;
        let mut h = History::default();
        h.matches.push(rec(id, "de_train", L, 11, 21));
        h.matches.push(rec(id, "de_nuke", L, 20, 18));
        h.matches.push(rec(id, "de_nuke", L, 19, 17));
        let r = h.recap(id);
        assert_eq!((r.best.as_deref(), r.worst.as_deref()), (None, Some("de_nuke")));
        assert_eq!(r.summary(), "3 matches, 0-3, K/D 0.89. Worst map Nuke.");
    }

    #[test]
    fn recap_with_one_map_has_no_best_or_worst() {
        let h = hist(&[&[W, L, D]]);
        let r = h.recap(1_000_000);
        assert_eq!((&r.best, &r.worst), (&None, &None));
        assert_eq!(r.summary(), "3 matches, 1-1-1, K/D 1.12.");
    }

    #[test]
    fn win_rate_rounds_and_handles_empty() {
        assert_eq!(Tally::default().win_rate(), None);
        let t = Tally { matches: 3, wins: 2, ..Tally::default() };
        assert_eq!(t.win_rate(), Some(67));
        assert_eq!(kd(10, 0), "10.00");
    }

    #[test]
    fn pretty_map_names() {
        assert_eq!(pretty_map("de_ancient"), "Ancient");
        assert_eq!(pretty_map("cs_office"), "Office");
        assert_eq!(pretty_map("workshop/123/aim_map"), "Aim_map");
    }
}
