//! Whole matches, as sequences of real-shaped GSI payloads, through the tracker
//! and the engine. Time is the payload's own timestamp, so hours of play run in
//! milliseconds.

use engine::engine::{Action, Engine, Kind, Rules, GONE_SECS, IDLE_SECS};
use engine::gsi::{self, Payload};
use engine::sim::{Fate, Sim};
use engine::stats::{History, MatchRecord, SessionRecord};
use engine::tracker::{Event, Outcome, Phase, Tracker};

const T0: u64 = 1_761_200_000;

fn parse(raw: &str) -> Payload {
    gsi::parse(raw.as_bytes()).unwrap()
}

fn ts(p: &Payload) -> u64 {
    p.provider.as_ref().unwrap().timestamp
}

fn track(payloads: &[String]) -> (Tracker, Vec<Event>) {
    let mut t = Tracker::default();
    let mut events = Vec::new();
    for raw in payloads {
        let p = parse(raw);
        events.extend(t.update(&p, ts(&p)));
    }
    (t, events)
}

struct Step {
    phase: Phase,
    actions: Vec<Action>,
}

fn feed(e: &mut Engine, payloads: &[String]) -> Vec<Step> {
    payloads
        .iter()
        .map(|raw| {
            let p = parse(raw);
            let actions = e.on_payload(&p, ts(&p));
            Step { phase: e.live(ts(&p)).view.phase, actions }
        })
        .collect()
}

fn all(steps: &[Step]) -> Vec<Action> {
    steps.iter().flat_map(|s| s.actions.clone()).collect()
}

fn notes(steps: &[Step]) -> Vec<(String, String)> {
    all(steps)
        .into_iter()
        .filter_map(|a| match a {
            Action::Notify { title, body } => Some((title, body)),
            _ => None,
        })
        .collect()
}

fn fresh() -> Engine {
    Engine::new(History::default(), Rules::default(), T0).0
}

fn ended(events: &[Event]) -> Vec<engine::tracker::Finished> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::MatchEnded(f) => Some(f.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_won_match_is_recorded_with_your_own_stats() {
    let mut sim = Sim::new("t", T0);
    let payloads = sim.menu().match_("de_mirage", 13, 9).take();
    let (_, events) = track(&payloads);
    assert_eq!(events.iter().filter(|e| matches!(e, Event::MatchStarted { .. })).count(), 1);
    let f = ended(&events);
    assert_eq!(f.len(), 1);
    let f = &f[0];
    assert_eq!((f.map.as_str(), f.mode.as_str()), ("de_mirage", "competitive"));
    assert_eq!(f.outcome, Outcome::Win);
    assert_eq!(f.score, (13, 9));
    // 22 rounds, dying in every other one: 11 deaths. 16 kills in the rounds you
    // survived and 4 traded before dying.
    // The teammate's line (11/7) that `player` shows while you spectate is ignored,
    // and so are the 4 warmup kills.
    assert_eq!((f.stats.kills, f.stats.deaths, f.stats.assists), (20, 11, 6));
    assert!(events.last() == Some(&Event::MatchEnded(f.clone())), "leaving after gameover isn't a MatchLeft");
}

#[test]
fn score_follows_your_side_after_halftime() {
    // You end the match on T, where team_t holds your score.
    let mut sim = Sim::new("t", T0);
    let payloads = sim.join("de_nuke", "competitive").play_to(9, 13).take();
    let last = parse(payloads.last().unwrap());
    assert_eq!(last.player.as_ref().unwrap().team.as_deref(), Some("T"));
    assert_eq!(last.map.as_ref().unwrap().team_t.score, 9);
    let f = ended(&track(&payloads).1);
    assert_eq!((f[0].outcome, f[0].score), (Outcome::Loss, (9, 13)));
}

#[test]
fn twelve_all_is_a_draw() {
    let mut sim = Sim::new("t", T0);
    let f = ended(&track(&sim.join("de_ancient", "competitive").play_to(12, 12).take()).1);
    assert_eq!((f[0].outcome, f[0].score), (Outcome::Draw, (12, 12)));
}

#[test]
fn music_pauses_only_while_you_are_alive_in_a_live_round() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    sim.menu().join("de_inferno", "competitive");
    let warmup = feed(&mut e, &sim.take());
    assert!(all(&warmup).is_empty(), "nothing happens in the menu or warmup");

    sim.round(true, Fate::Survive);
    let survived = feed(&mut e, &sim.take());
    let phases: Vec<(Phase, Vec<Action>)> =
        survived.iter().filter(|s| !s.actions.is_empty()).map(|s| (s.phase, s.actions.clone())).collect();
    assert_eq!(
        phases,
        vec![(Phase::Live, vec![Action::PauseMusic]), (Phase::Over, vec![Action::ResumeMusic])]
    );

    sim.round(false, Fate::Die);
    let died = feed(&mut e, &sim.take());
    assert_eq!(all(&died), vec![Action::PauseMusic, Action::ResumeMusic]);
    // The resume comes on the payload where your health hits 0, mid-round.
    let resume_at = died.iter().position(|s| s.actions == vec![Action::ResumeMusic]).unwrap();
    assert_eq!(died[resume_at].phase, Phase::Live);
    // Spectating a teammate for the rest of the round doesn't pause again.
    assert!(died[resume_at + 1..].iter().all(|s| s.actions.is_empty()));
}

#[test]
fn a_full_match_pauses_once_per_round_and_ends_resumed() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    let steps = feed(&mut e, &sim.match_("de_overpass", 13, 7).take());
    let acts = all(&steps);
    let pauses = acts.iter().filter(|a| **a == Action::PauseMusic).count();
    let resumes = acts.iter().filter(|a| **a == Action::ResumeMusic).count();
    assert_eq!((pauses, resumes), (20, 20));
    // Strictly alternating, never two pauses in a row.
    let music: Vec<_> = acts.iter().filter(|a| matches!(a, Action::PauseMusic | Action::ResumeMusic)).collect();
    assert!(music.chunks(2).all(|c| c == [&Action::PauseMusic, &Action::ResumeMusic]));
    assert!(!e.live(sim.ts).music_paused);
}

#[test]
fn opening_the_esc_menu_mid_round_keeps_music_paused() {
    let mut e = fresh();
    let mut live: serde_json::Value = serde_json::from_str(include_str!("fixtures/live_round.json")).unwrap();
    let t = live["provider"]["timestamp"].as_u64().unwrap();
    assert_eq!(e.on_payload(&parse(&live.to_string()), t), vec![Action::PauseMusic]);
    live["player"]["activity"] = "menu".into();
    assert!(e.on_payload(&parse(&live.to_string()), t + 2).is_empty());
    live["player"]["activity"] = "textinput".into();
    assert!(e.on_payload(&parse(&live.to_string()), t + 4).is_empty());
    assert!(e.live(t + 4).music_paused);
}

#[test]
fn turning_music_control_off_mid_round_resumes_and_back_on_pauses() {
    let mut e = fresh();
    let live = include_str!("fixtures/live_round.json");
    let t = ts(&parse(live));
    assert_eq!(e.on_payload(&parse(live), t), vec![Action::PauseMusic]);
    let off = Rules { music: false, ..Rules::default() };
    assert_eq!(e.set_rules(off, t), vec![Action::ResumeMusic]);
    assert!(e.on_payload(&parse(live), t + 1).is_empty());
    assert_eq!(e.set_rules(Rules::default(), t + 2), vec![Action::PauseMusic]);
    // Turned on again long after CS2 went quiet: nothing to pause.
    e.set_rules(off, t + 3);
    assert!(e.set_rules(Rules::default(), t + 600).is_empty());
}

#[test]
fn cs2_closing_mid_round_resumes_music_quickly_and_ends_the_session_later() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    feed(&mut e, &sim.match_("de_dust2", 13, 4).take());
    sim.join("de_mirage", "competitive").round(true, Fate::Survive);
    let mut raw = sim.take();
    // Cut off in the middle of round 2's live phase.
    sim.round(true, Fate::Survive);
    let round2 = sim.take();
    raw.extend(round2.into_iter().take(2));
    feed(&mut e, &raw);
    let last = ts(&parse(raw.last().unwrap()));
    assert!(e.live(last).music_paused);
    assert!(e.tick(last + 15).is_empty(), "a gap shorter than a heartbeat changes nothing");
    // Half a minute without even a heartbeat: CS2 is gone, the music comes back.
    assert_eq!(e.tick(last + 30), vec![Action::ResumeMusic]);
    assert!(e.tick(last + 60).is_empty());
    let acts = e.tick(last + GONE_SECS);
    assert!(!acts.contains(&Action::ResumeMusic), "resumed once, not twice");
    assert!(acts.iter().any(|a| matches!(a, Action::Notify { title, .. } if title == "Session recap")));
    let live = e.live(last + GONE_SECS);
    assert!(!live.connected && live.session.is_none());
    let recap = live.recap.unwrap();
    // The unfinished match on Mirage isn't counted.
    assert_eq!((recap.tally.matches, recap.tally.wins), (1, 1));
    assert_eq!(e.history().sessions[0].ended, Some(last));
}

#[test]
fn leaving_mid_match_records_nothing_and_reconnecting_records_the_result() {
    let mut sim = Sim::new("t", T0);
    sim.join("de_vertigo", "competitive");
    for _ in 0..5 {
        sim.round(true, Fate::Survive);
    }
    sim.menu();
    // Reconnect: the scoreboard and your stats carry on from where they were.
    sim.join_back("de_vertigo");
    sim.play_to(13, 3).menu();
    let (_, events) = track(&sim.take());
    assert_eq!(events.iter().filter(|e| **e == Event::MatchLeft).count(), 1);
    let f = ended(&events);
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].outcome, f[0].score), (Outcome::Win, (13, 3)));
}

#[test]
fn starting_freezetime_mid_match_still_records_it() {
    let mut sim = Sim::new("t", T0);
    sim.join("de_train", "competitive");
    for _ in 0..10 {
        sim.round(true, Fate::Die);
    }
    sim.take(); // the app wasn't running yet
    sim.play_to(13, 6);
    let f = ended(&track(&sim.take()).1);
    assert_eq!((f[0].outcome, f[0].score), (Outcome::Win, (13, 6)));
}

#[test]
fn casual_pauses_music_but_isnt_recorded() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    sim.join("de_dust2", "casual").play_to(8, 3).menu();
    let steps = feed(&mut e, &sim.take());
    assert!(all(&steps).contains(&Action::PauseMusic));
    assert!(!all(&steps).contains(&Action::SaveHistory));
    assert!(e.history().matches.is_empty());
    // A session with no ranked matches leaves nothing behind.
    assert!(e.end_session(sim.ts).is_empty());
    assert!(e.history().sessions.is_empty());
}

#[test]
fn spectating_a_match_you_are_not_in_does_nothing() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    sim.join("de_mirage", "competitive").play_to(13, 11);
    let raw: Vec<String> = sim
        .take()
        .into_iter()
        .map(|r| {
            let mut v: serde_json::Value = serde_json::from_str(&r).unwrap();
            v["provider"]["steamid"] = "76561190000000000".into();
            v.to_string()
        })
        .collect();
    let steps = feed(&mut e, &raw);
    assert!(all(&steps).is_empty());
}

#[test]
fn three_losses_in_a_row_warns_with_your_numbers() {
    // History: 10 earlier sessions. After 3 straight losses you won 3 of 10.
    let mut h = History::default();
    for i in 0..10u64 {
        let id = T0 - 40 * 86_400 + i * 86_400;
        h.sessions.push(SessionRecord { id, ended: Some(id + 20_000) });
        let after = if i < 3 { Outcome::Win } else { Outcome::Loss };
        for (j, o) in [Outcome::Win, Outcome::Loss, Outcome::Loss, Outcome::Loss, after].into_iter().enumerate() {
            h.matches.push(MatchRecord {
                session: id,
                map: "de_mirage".into(),
                mode: "premier".into(),
                outcome: o,
                score: if o == Outcome::Win { (13, 10) } else { (10, 13) },
                kills: 17,
                deaths: 16,
                assists: 4,
                mvps: 2,
                started: id + j as u64 * 3000,
                ended: id + j as u64 * 3000 + 2400,
            });
        }
    }
    // Breaks off: this is about the tilt warning alone.
    let mut e = Engine::new(h, Rules { break_minutes: 0, ..Rules::default() }, T0).0;
    let mut sim = Sim::new("t", T0);
    sim.match_("de_anubis", 9, 13).match_("de_anubis", 11, 13);
    assert!(notes(&feed(&mut e, &sim.take())).is_empty(), "two losses: below the default threshold");
    sim.join("de_nuke", "competitive").play_to(7, 13);
    let steps = feed(&mut e, &sim.take());
    let n = notes(&steps);
    // Overall includes today's three losses.
    let overall = e.history().overall().win_rate().unwrap();
    assert_eq!(
        n,
        vec![(
            "3 losses in a row".to_string(),
            format!("You've lost 3 in a row. Your win rate after 3 losses in a row is 30% (10 matches), against {overall}% overall.")
        )]
    );
    // It arrives on the gameover payload: between matches, never mid-round.
    let at = steps.iter().find(|s| s.actions.iter().any(|a| matches!(a, Action::Notify { .. }))).unwrap();
    assert_eq!(at.phase, Phase::Gameover);
    let live = e.live(sim.ts);
    assert_eq!(live.session.as_ref().unwrap().streak, -3);
    assert_eq!(live.messages[0].kind, Kind::Tilt);
}

#[test]
fn break_reminders_come_between_matches_only() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    // Four matches back to back is well over 90 minutes.
    sim.menu();
    for map in ["de_mirage", "de_inferno", "de_nuke", "de_ancient"] {
        sim.match_(map, 13, 11);
    }
    let steps = feed(&mut e, &sim.take());
    let reminders: Vec<&Step> = steps
        .iter()
        .filter(|s| s.actions.iter().any(|a| matches!(a, Action::Notify { title, .. } if title == "Time for a break")))
        .collect();
    assert!(!reminders.is_empty());
    assert!(reminders.iter().all(|s| s.phase == Phase::Gameover));
    // Never more than one per 90 minutes of play.
    assert!(reminders.len() <= ((sim.ts - T0) / (90 * 60)) as usize);
    let (_, body) = notes(&steps).into_iter().find(|(t, _)| t == "Time for a break").unwrap();
    assert!(body.starts_with("You've played for 1 hour"), "{body}");
}

#[test]
fn a_real_break_resets_the_timer() {
    let mut e = Engine::new(History::default(), Rules { break_minutes: 60, ..Rules::default() }, T0).0;
    let mut sim = Sim::new("t", T0);
    sim.match_("de_mirage", 13, 11);
    feed(&mut e, &sim.take());
    // 20 minutes in the menu, then another long match.
    sim.ts += 20 * 60;
    e.tick(sim.ts);
    sim.match_("de_inferno", 13, 11);
    let n = notes(&feed(&mut e, &sim.take()));
    assert!(n.iter().all(|(t, _)| t != "Time for a break"), "{n:?}");
}

#[test]
fn idle_in_the_menu_for_an_hour_ends_the_session() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    sim.match_("de_mirage", 13, 5);
    feed(&mut e, &sim.take());
    // The idle clock starts when the match ends.
    let start = e.history().matches[0].ended;
    // CS2 still open, sending menu heartbeats every 10 seconds.
    while sim.ts + 10 < start + IDLE_SECS {
        sim.menu();
        feed(&mut e, &sim.take());
        let acts = e.tick(sim.ts);
        assert!(!acts.iter().any(|a| matches!(a, Action::Notify { .. })), "too early at {}", sim.ts - start);
    }
    let acts = e.tick(start + IDLE_SECS);
    assert!(acts.iter().any(|a| matches!(a, Action::Notify { title, .. } if title == "Session recap")), "{acts:?}");
}

#[test]
fn reopening_the_app_continues_a_recent_session_and_closes_an_old_one() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    sim.match_("de_mirage", 13, 5);
    feed(&mut e, &sim.take());
    let h = e.history().clone();
    assert_eq!(h.sessions[0].ended, None);

    let (e2, acts) = Engine::new(h.clone(), Rules::default(), sim.ts + 600);
    assert!(acts.is_empty());
    assert_eq!(e2.live(sim.ts + 600).session.unwrap().tally.matches, 1);

    let (e3, acts) = Engine::new(h, Rules::default(), sim.ts + IDLE_SECS * 3);
    assert_eq!(acts, vec![Action::SaveHistory]);
    assert!(e3.live(sim.ts).session.is_none());
    assert!(e3.history().sessions[0].ended.is_some());
}

#[test]
fn a_cs2_started_with_an_old_token_is_reported() {
    let mut e = fresh();
    e.on_rejected(T0);
    assert!(e.live(T0 + 5).stale_token);
    e.on_payload(&parse(include_str!("fixtures/menu.json")), T0 + 6);
    assert!(!e.live(T0 + 6).stale_token);
}

#[test]
fn a_tilt_warning_and_a_break_reminder_share_one_notification() {
    let mut e = fresh();
    let mut sim = Sim::new("t", T0);
    sim.match_("de_anubis", 9, 13).match_("de_anubis", 11, 13).match_("de_nuke", 7, 13);
    let n = notes(&feed(&mut e, &sim.take()));
    let last = n.last().unwrap();
    assert_eq!(last.0, "3 losses in a row");
    assert!(last.1.contains("You've lost 3 in a row.") && last.1.contains("without a break"), "{last:?}");
    // Both are kept as separate messages in the app.
    assert_eq!(e.live(sim.ts).messages.len(), 2);
}

#[test]
fn match_times_come_from_the_game_clock_unless_it_is_far_off() {
    let mut sim = Sim::new("t", T0);
    sim.match_("de_mirage", 13, 2);
    let raw = sim.take();
    // Processed an hour after the fact: times still come from the payloads.
    let mut e = fresh();
    for r in &raw {
        e.on_payload(&parse(r), T0 + 3600 + 10);
    }
    let m = &e.history().matches[0];
    assert!(m.started < T0 + 300 && m.ended > m.started + 15 * 60, "{m:?}");
    // A clock two days off is ignored in favour of the time it arrived.
    let mut e = fresh();
    let late = T0 + 2 * 86_400;
    for r in &raw {
        e.on_payload(&parse(r), late);
    }
    let m = &e.history().matches[0];
    assert_eq!((m.started, m.ended), (late, late));
}
