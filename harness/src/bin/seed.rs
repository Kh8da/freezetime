//! Writes a believable `history.json` for looking at the UI with data in it:
//! about two months of evening sessions with a slump after losing streaks.
//!
//!   seed <app data folder>
//!
//! Refuses to overwrite an existing history.

use engine::stats::{History, MatchRecord, SessionRecord};
use engine::store::Store;
use engine::tracker::Outcome;
use std::path::PathBuf;

/// Small deterministic generator, so every run draws the same history.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const MAPS: &[(&str, u64)] = &[
    ("de_mirage", 58),
    ("de_inferno", 52),
    ("de_ancient", 50),
    ("de_nuke", 41),
    ("de_dust2", 55),
    ("de_anubis", 47),
    ("de_train", 44),
];

fn main() {
    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: seed <app data folder>");
        std::process::exit(2);
    });
    if dir.join("history.json").exists() {
        eprintln!("seed: {} already has a history.json; not touching it", dir.display());
        std::process::exit(1);
    }
    let mut rng = Rng(0x5eed_f00d_1234_5678);
    let mut h = History::default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let today = now - now % 86_400;
    for d in (1..60u64).rev() {
        if rng.below(10) < 6 {
            continue;
        }
        // Evenings, local-ish: 18:00 to 21:00 UTC start.
        let id = today - d * 86_400 + 18 * 3600 + rng.below(3 * 3600);
        let mut t = id + 120;
        let mut streak = 0;
        let count = 2 + rng.below(4);
        for _ in 0..count {
            let (map, base) = MAPS[rng.below(MAPS.len() as u64) as usize];
            // After two losses in a row the odds drop: the pattern tilt guard is for.
            let chance = if streak >= 2 { base - 16 } else { base };
            let roll = rng.below(100);
            let outcome = if roll < 4 { Outcome::Draw } else if roll < chance { Outcome::Win } else { Outcome::Loss };
            let other = 3 + rng.below(9) as u32;
            let score = match outcome {
                Outcome::Win => (13, other),
                Outcome::Loss => (other, 13),
                Outcome::Draw => (12, 12),
            };
            let rounds = (score.0 + score.1) as u64;
            let good = outcome == Outcome::Win;
            let kills = (10 + rng.below(12) + if good { 4 } else { 0 }) as u32;
            let deaths = (11 + rng.below(9) + if good { 0 } else { 3 }) as u32;
            let length = rounds * 105 + rng.below(240);
            h.matches.push(MatchRecord {
                session: id,
                map: map.into(),
                mode: if rng.below(3) == 0 { "competitive" } else { "premier" }.into(),
                outcome,
                score,
                kills,
                deaths,
                assists: (2 + rng.below(6)) as u32,
                mvps: (rng.below(5) + u64::from(good)) as u32,
                started: t,
                ended: t + length,
            });
            streak = if outcome == Outcome::Loss { streak + 1 } else { 0 };
            t += length + 180 + rng.below(600);
        }
        h.sessions.push(SessionRecord { id, ended: Some(t) });
    }
    Store::new(dir.clone()).save_history(&h).unwrap();
    println!("seed: wrote {} matches in {} sessions to {}", h.matches.len(), h.sessions.len(), dir.join("history.json").display());
}
