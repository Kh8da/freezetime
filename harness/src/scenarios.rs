use engine::sim::{Fate, Sim};

pub const NAMES: &[&str] = &["session", "round", "match", "menu"];

/// Payload sequences the fake CS2 can play.
pub fn build(name: &str, token: &str, start: u64) -> Option<Vec<String>> {
    let mut sim = Sim::new(token, start);
    match name {
        // A win, then three losses: enough for a tilt warning at the default setting.
        "session" => {
            sim.menu()
                .match_("de_mirage", 13, 9)
                .match_("de_nuke", 10, 13)
                .match_("de_inferno", 8, 13)
                .match_("de_ancient", 11, 13);
        }
        // One live round where you die, then the next freezetime.
        "round" => {
            sim.menu().join("de_dust2", "competitive").round(true, Fate::Survive).round(false, Fate::Die);
        }
        "match" => {
            sim.menu().match_("de_anubis", 13, 11);
        }
        "menu" => {
            sim.menu();
        }
        _ => return None,
    }
    Some(sim.take())
}
