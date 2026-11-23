//! Anonymous play data, so the game can be improved from how people really
//! play it: where new players stop, how long training takes, which classes
//! win, crashes and slow frames.
//!
//! It is off until the player says yes (first launch, or Settings), and it
//! carries no account, name, email, troop brain or file: only a random id
//! made on this computer when the player opted in, which "Delete my play
//! data" in Settings throws away together with everything sent under it.
//! The server keeps no address with it, and forgets it after
//! [`PLAY_KEEP_DAYS`].
//!
//! [`PLAY_EVENTS`] is the full list of what the game may send. The server
//! refuses anything else, and Settings shows players this same list.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Most events in one upload.
pub const PLAY_BATCH_MAX: usize = 200;
/// Longest an event's details may be, as JSON text.
pub const PLAY_PROPS_MAX: usize = 1024;
/// Days the server keeps play data.
pub const PLAY_KEEP_DAYS: i64 = 180;

/// Every event the game may send, and what it carries, in words players
/// read in Settings.
pub const PLAY_EVENTS: &[(&str, &str)] = &[
    ("session_start", "The game opened: days since your first game, the operating system, CPU cores, graphics chip, what training runs on, and the biggest brain your PC can train."),
    ("session_end", "The game closed: minutes played, frames per second, and how often a frame was slow."),
    ("screen", "Which screen you opened (to see where new players stop)."),
    ("tour_step", "How far you got through the new player tour."),
    ("tour_end", "The tour finished or was skipped, and at which step."),
    ("battle_end", "A battle ended: offline raid, defence or online raid, stars, won or lost, ran out of time, the squad's classes, levels and brain sizes, and whether you watched a troop's thinking."),
    ("first_raid_won", "You won your first raid, and how many minutes after your first game."),
    ("training_start", "Training started: the troop's class, level and brain size, solo or team."),
    ("training_end", "Training finished: minutes it took, steps, drill win rate, whether the troop reached its plateau, brain size."),
    ("speed_test", "The PC speed test passed or failed, and the biggest brain it found."),
    ("stat_panel", "You opened a troop's stat panel."),
    ("sign_in", "Signing in worked or failed (Google, Discord), and the error code if it failed. Never your account."),
    ("find_opponent", "Find opponent found a base or came back empty, and the brain-size category."),
    ("cup_entry", "You entered the weekly cup, and the brain-size category."),
    ("online_error", "The server refused a request: its error code (for example, game update needed)."),
    ("crash", "The game crashed: the error message and the place in the game's code."),
    ("rating", "Your 1 to 5 rating after your first raid or first training."),
];

/// Is this one of the events the game may send?
pub fn play_event_known(name: &str) -> bool {
    PLAY_EVENTS.iter().any(|(n, _)| *n == name)
}

/// One thing that happened in the game.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayEvent {
    /// One of [`PLAY_EVENTS`].
    pub name: String,
    /// Seconds since this game session started.
    pub secs: u32,
    /// Its details (a JSON object, small).
    #[serde(default)]
    pub props: serde_json::Map<String, serde_json::Value>,
}

/// What the game uploads now and then.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayBatch {
    /// The random id made when the player opted in.
    pub install: Uuid,
    /// A random id for this run of the game.
    pub session: Uuid,
    pub game_version: String,
    pub sim_version: u32,
    /// "windows", "linux" or "macos".
    pub os: String,
    pub events: Vec<PlayEvent>,
}

/// Why a batch was refused.
pub fn check_play_batch(b: &PlayBatch) -> Result<(), String> {
    if b.events.is_empty() || b.events.len() > PLAY_BATCH_MAX {
        return Err(format!("send 1 to {PLAY_BATCH_MAX} events at a time"));
    }
    if b.game_version.len() > 20 || b.os.len() > 20 {
        return Err("game version or OS name too long".into());
    }
    for e in &b.events {
        if !play_event_known(&e.name) {
            return Err(format!("unknown event {:?}", e.name.chars().take(40).collect::<String>()));
        }
        let len = serde_json::to_string(&e.props).map_or(usize::MAX, |s| s.len());
        if len > PLAY_PROPS_MAX {
            return Err(format!("event {} has too many details", e.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(name: &str) -> PlayBatch {
        PlayBatch {
            install: Uuid::new_v4(),
            session: Uuid::new_v4(),
            game_version: "0.1.0".into(),
            sim_version: 11,
            os: "windows".into(),
            events: vec![PlayEvent { name: name.into(), secs: 3, props: Default::default() }],
        }
    }

    #[test]
    fn only_listed_events_pass() {
        assert!(check_play_batch(&batch("screen")).is_ok());
        assert!(check_play_batch(&batch("email")).is_err());
        let mut b = batch("screen");
        b.events[0].props.insert("x".into(), "y".repeat(PLAY_PROPS_MAX).into());
        assert!(check_play_batch(&b).is_err());
        b.events.clear();
        assert!(check_play_batch(&b).is_err());
    }
}
