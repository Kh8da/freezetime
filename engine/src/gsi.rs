//! The parts of a CS2 Game State Integration payload that freezetime reads.
//!
//! CS2 only sends the sections the cfg file asks for (see [`crate::cs2::cfg_text`]),
//! and leaves sections out when they don't apply: no `map` or `round` in the main
//! menu, no `round` during warmup on some servers. Everything is optional and
//! unknown fields are ignored, so a CS2 update that adds fields can't break parsing.

use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Payload {
    pub provider: Option<Provider>,
    pub map: Option<Map>,
    pub round: Option<Round>,
    pub player: Option<Player>,
    pub auth: Option<Auth>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Provider {
    pub name: String,
    #[serde(deserialize_with = "num")]
    pub appid: u32,
    /// SteamID64 of the person playing. `player.steamid` only matches it while
    /// you're alive: once you die, `player` is whoever you're spectating.
    pub steamid: String,
    #[serde(deserialize_with = "num")]
    pub timestamp: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Map {
    /// "competitive", "premier", "scrimcomp2v2" (Wingman), "casual", "deathmatch", ...
    pub mode: String,
    pub name: String,
    /// "warmup", "live", "intermission" (halftime) or "gameover".
    pub phase: String,
    #[serde(deserialize_with = "num")]
    pub round: u32,
    pub team_ct: Team,
    pub team_t: Team,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Team {
    #[serde(deserialize_with = "num")]
    pub score: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Round {
    /// "freezetime", "live" or "over".
    pub phase: String,
    pub win_team: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Player {
    pub steamid: String,
    pub name: String,
    /// "CT" or "T". Missing in the menu.
    pub team: Option<String>,
    /// "playing", "menu" or "textinput" (console or chat open).
    pub activity: String,
    pub state: Option<PlayerState>,
    pub match_stats: Option<MatchStats>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PlayerState {
    #[serde(deserialize_with = "num")]
    pub health: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MatchStats {
    #[serde(deserialize_with = "num")]
    pub kills: u32,
    #[serde(deserialize_with = "num")]
    pub assists: u32,
    #[serde(deserialize_with = "num")]
    pub deaths: u32,
    #[serde(deserialize_with = "num")]
    pub mvps: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Auth {
    pub token: String,
}

/// CS2 sends numbers as JSON numbers, but some community servers and older
/// builds have sent them as strings. Accept both, and treat anything else
/// (null, negative, garbage) as zero rather than dropping the whole payload.
fn num<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64> + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    let n = match v {
        serde_json::Value::Number(n) => n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    Ok(n.and_then(|n| T::try_from(n).ok()).unwrap_or_default())
}

pub fn parse(body: &[u8]) -> Result<Payload, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_captured_live_round_payload() {
        let p = parse(include_bytes!("../tests/fixtures/live_round.json")).unwrap();
        let provider = p.provider.unwrap();
        assert_eq!(provider.appid, 730);
        assert_eq!(provider.steamid, "76561198012345678");
        let map = p.map.unwrap();
        assert_eq!((map.mode.as_str(), map.name.as_str(), map.phase.as_str()), ("competitive", "de_mirage", "live"));
        assert_eq!((map.team_ct.score, map.team_t.score, map.round), (5, 3, 8));
        assert_eq!(p.round.unwrap().phase, "live");
        let player = p.player.unwrap();
        assert_eq!(player.team.as_deref(), Some("CT"));
        assert_eq!(player.state.unwrap().health, 100);
        assert_eq!(player.match_stats.unwrap(), MatchStats { kills: 9, assists: 2, deaths: 5, mvps: 2 });
        assert_eq!(p.auth.unwrap().token, "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn parses_a_main_menu_payload() {
        let p = parse(include_bytes!("../tests/fixtures/menu.json")).unwrap();
        assert!(p.map.is_none());
        assert!(p.round.is_none());
        assert_eq!(p.player.unwrap().activity, "menu");
    }

    #[test]
    fn tolerates_numbers_as_strings_and_odd_values() {
        let p = parse(br#"{"map":{"round":"7","team_ct":{"score":"3"},"team_t":{"score":null}},"player":{"state":{"health":-1}}}"#).unwrap();
        let map = p.map.unwrap();
        assert_eq!((map.round, map.team_ct.score, map.team_t.score), (7, 3, 0));
        assert_eq!(p.player.unwrap().state.unwrap().health, 0);
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse(b"hello").is_err());
    }
}
