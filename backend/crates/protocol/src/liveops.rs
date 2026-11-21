//! Live operations: events, gifts and tunable rewards that the game's
//! operator changes on the server without releasing a new game build.
//!
//! None of this touches battle rules. Events and settings only change how
//! much gold and how long a shield a finished battle pays out, so raids
//! re-run by the server give the same result as before.

use crate::{Season, User};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Gold multiplier, in percent, that an event may set. 100 is normal.
pub const EVENT_GOLD_PCT_MAX: i32 = 1000;
/// Flat gold an event may add to every won raid.
pub const EVENT_WIN_GOLD_MAX: i64 = 10_000;
/// Longest event, in days.
pub const EVENT_MAX_DAYS: i64 = 90;
pub const EVENT_TITLE_MAX: usize = 60;
pub const EVENT_MESSAGE_MAX: usize = 240;
/// Biggest single gift.
pub const GIFT_GOLD_MAX: i64 = 100_000;
pub const SEASON_NAME_MAX: usize = 40;

/// A time-limited event, as players see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveEvent {
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub message: String,
    pub starts_at: String,
    pub ends_at: String,
    /// Multiplier, in percent, on gold earned while it runs (100 = normal,
    /// 200 = double gold). Applies to raid loot and win bonuses and to war
    /// rewards. Loot taken from defenders is not multiplied: the extra is
    /// new gold.
    pub gold_pct: i32,
    /// Flat extra gold for every raid won with at least one star.
    pub win_gold: i64,
    /// Running now, as opposed to coming up.
    pub active: bool,
}

impl LiveEvent {
    /// Short description of what the event does, for the game and the
    /// admin page: "2x gold, +50 gold per win".
    pub fn perks(&self) -> String {
        let mut p = Vec::new();
        if self.gold_pct != 100 {
            if self.gold_pct % 100 == 0 {
                p.push(format!("{}x gold", self.gold_pct / 100));
            } else {
                p.push(format!("{}% gold", self.gold_pct));
            }
        }
        if self.win_gold > 0 {
            p.push(format!("+{} gold per win", self.win_gold));
        }
        p.join(", ")
    }
}

/// Gold the operator gave out, waiting for the player to claim it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gift {
    pub id: Uuid,
    pub gold: i64,
    #[serde(default)]
    pub message: String,
    pub expires_at: String,
}

/// What the game shows about live operations: `GET /v1/live`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiveView {
    /// Events running now, then upcoming ones (next 7 days), soonest first.
    pub events: Vec<LiveEvent>,
    /// Gifts this player hasn't claimed yet.
    pub gifts: Vec<Gift>,
    pub season: Option<Season>,
    /// All running events together: gold multiplier in percent.
    pub gold_pct: i32,
    /// All running events together: flat gold per won raid.
    pub win_gold: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GiftClaimed {
    pub gold: i64,
    pub user: User,
}

/// Running events combined: extras add up, so two "double gold" events at
/// once give triple gold.
pub fn combined_gold_pct(events: &[LiveEvent]) -> i32 {
    let extra: i32 = events
        .iter()
        .filter(|e| e.active)
        .map(|e| e.gold_pct - 100)
        .sum();
    (100 + extra).clamp(0, EVENT_GOLD_PCT_MAX * 2)
}

pub fn combined_win_gold(events: &[LiveEvent]) -> i64 {
    events.iter().filter(|e| e.active).map(|e| e.win_gold).sum()
}

/// `gold` scaled by a percent multiplier, rounded down.
pub fn scale_gold(gold: i64, pct: i32) -> i64 {
    gold.saturating_mul(pct.max(0) as i64) / 100
}

// ------------------------------------------------------------ admin side

/// Start or schedule an event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewEvent {
    pub title: String,
    #[serde(default)]
    pub message: String,
    /// RFC 3339; empty or missing means now.
    #[serde(default)]
    pub starts_at: Option<String>,
    pub ends_at: String,
    #[serde(default = "hundred")]
    pub gold_pct: i32,
    #[serde(default)]
    pub win_gold: i64,
}

fn hundred() -> i32 {
    100
}

/// Give gold to every player, or to one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewGift {
    pub gold: i64,
    #[serde(default)]
    pub message: String,
    /// A player's name or id. Missing means everyone.
    #[serde(default)]
    pub player: Option<String>,
    /// Days players have to claim it (default 14).
    #[serde(default)]
    pub days: Option<i64>,
}

/// Change the running season.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeasonEdit {
    #[serde(default)]
    pub name: Option<String>,
    /// New end time, RFC 3339.
    #[serde(default)]
    pub ends_at: Option<String>,
}

/// One reward or timing the operator can change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingView {
    pub key: String,
    pub group: String,
    pub label: String,
    pub help: String,
    pub value: i64,
    pub default: i64,
    pub min: i64,
    pub max: i64,
}

/// A gift in the admin list, with how many claimed it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminGift {
    pub id: Uuid,
    pub gold: i64,
    pub message: String,
    /// Player name, or empty for everyone.
    pub player: String,
    pub created_at: String,
    pub expires_at: String,
    pub claims: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminLogEntry {
    pub at: String,
    pub action: String,
    pub detail: String,
}

/// Everything the admin page shows: `GET /v1/admin/overview`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminOverview {
    pub season: Season,
    /// Running, upcoming and the last few finished events.
    pub events: Vec<LiveEvent>,
    pub gifts: Vec<AdminGift>,
    pub players: i64,
    /// Players who signed in during the last day.
    pub active_today: i64,
    pub log: Vec<AdminLogEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(active: bool, pct: i32, win: i64) -> LiveEvent {
        LiveEvent {
            id: Uuid::nil(),
            title: "t".into(),
            message: String::new(),
            starts_at: String::new(),
            ends_at: String::new(),
            gold_pct: pct,
            win_gold: win,
            active,
        }
    }

    #[test]
    fn events_add_up() {
        assert_eq!(combined_gold_pct(&[]), 100);
        let e = [ev(true, 200, 10), ev(true, 150, 5), ev(false, 500, 99)];
        assert_eq!(combined_gold_pct(&e), 250);
        assert_eq!(combined_win_gold(&e), 15);
        assert_eq!(scale_gold(41, 250), 102);
    }

    #[test]
    fn perks_read_well() {
        assert_eq!(ev(true, 200, 50).perks(), "2x gold, +50 gold per win");
        assert_eq!(ev(true, 150, 0).perks(), "150% gold");
        assert_eq!(ev(true, 100, 0).perks(), "");
    }
}
