//! One-off tournaments the operator runs from the admin page, next to the
//! weekly cup: a name, a start and an end, optional rules on who may enter
//! (one brain-size category, one main class) and prizes for rank bands
//! that can be gold, weapons or a trained troop.
//!
//! Troops are scored exactly like the weekly cup (`cup_score`: the game's
//! own seeded solo and squad raids), ranked within each brain-size
//! category. Prizes arrive as gifts the player claims in the game.

use crate::liveops::{Reward, EVENT_MESSAGE_MAX};
use crate::{CupRow, Invalid};
use godcore::classes::BaseClass;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TOURNAMENT_TITLE_MAX: usize = 60;
pub const TOURNAMENT_MESSAGE_MAX: usize = EVENT_MESSAGE_MAX;
/// Longest tournament, in days.
pub const TOURNAMENT_MAX_DAYS: i64 = 60;
/// Most prize bands in one tournament.
pub const TOURNAMENT_BANDS_MAX: usize = 10;
/// Ranks paid at most (a band can't reach past this).
pub const TOURNAMENT_RANK_MAX: u32 = 1000;
/// Gold one prize band may pay each player.
pub const TOURNAMENT_GOLD_MAX: i64 = 100_000;

/// Prize for every player ranked `from` to `to` (inclusive) in their
/// category.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrizeBand {
    pub from: u32,
    pub to: u32,
    pub reward: Reward,
}

/// Start or schedule a tournament.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewTournament {
    pub title: String,
    #[serde(default)]
    pub message: String,
    /// RFC 3339; empty or missing means now.
    #[serde(default)]
    pub starts_at: Option<String>,
    pub ends_at: String,
    /// Only troops of this brain-size category may enter. Missing: every
    /// category, each ranked on its own.
    #[serde(default)]
    pub category: Option<u8>,
    /// Only troops whose main class is this one may enter.
    #[serde(default)]
    pub class: Option<BaseClass>,
    pub prizes: Vec<PrizeBand>,
}

/// One prize band as players see it (a troop prize by name only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrizeLine {
    pub from: u32,
    pub to: u32,
    /// "500 gold, Crossbow".
    pub text: String,
    /// Gold part, for sorting and the standings.
    pub gold: i64,
}

impl PrizeLine {
    pub fn of(b: &PrizeBand) -> PrizeLine {
        PrizeLine {
            from: b.from,
            to: b.to,
            text: b.reward.describe(),
            gold: b.reward.gold,
        }
    }

    /// "1st", "2nd to 3rd", "4th to 10th".
    pub fn ranks(&self) -> String {
        if self.from == self.to {
            ordinal(self.from)
        } else {
            format!("{} to {}", ordinal(self.from), ordinal(self.to))
        }
    }
}

pub fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A tournament as the game and the admin page list it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TournamentInfo {
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub message: String,
    pub starts_at: String,
    pub ends_at: String,
    pub category: Option<u8>,
    pub class: Option<BaseClass>,
    pub prizes: Vec<PrizeLine>,
    /// Entries across all categories.
    pub entries: i64,
    /// Open for entries now.
    pub active: bool,
    /// Over; prizes are paid (as gifts) shortly after.
    pub finished: bool,
    /// Prizes have been handed out.
    pub paid: bool,
}

impl TournamentInfo {
    /// Who may enter: "Archers, up to 256 brains", or "Everyone".
    pub fn rules(&self) -> String {
        let mut p = Vec::new();
        if let Some(c) = self.class {
            p.push(format!("{c:?} troops only"));
        }
        if let Some(b) = self.category {
            p.push(format!("\"{}\" brains only", crate::bracket_name(b)));
        }
        if p.is_empty() {
            "Any attacker".into()
        } else {
            p.join(", ")
        }
    }

    /// The prize for `rank`, if any band covers it.
    pub fn prize_for(&self, rank: u32) -> Option<&PrizeLine> {
        self.prizes.iter().find(|p| p.from <= rank && rank <= p.to)
    }
}

/// One tournament's table in one category: `GET /v1/tournaments/{id}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TournamentStandings {
    pub info: TournamentInfo,
    /// The category `top` shows.
    pub category: u8,
    pub entries: u32,
    pub top: Vec<CupRow>,
    pub mine: Option<CupRow>,
}

/// Check a tournament's prize bands: in order, not overlapping, each
/// paying something.
pub fn check_bands(bands: &[PrizeBand]) -> Result<(), Invalid> {
    if bands.is_empty() {
        return Err(Invalid("give the tournament at least one prize".into()));
    }
    if bands.len() > TOURNAMENT_BANDS_MAX {
        return Err(Invalid(format!("at most {TOURNAMENT_BANDS_MAX} prize bands")));
    }
    let mut last = 0;
    for b in bands {
        if b.from < 1 || b.to < b.from || b.to > TOURNAMENT_RANK_MAX {
            return Err(Invalid(format!(
                "prize ranks must run from 1 up, at most {TOURNAMENT_RANK_MAX} ({} to {} isn't)",
                b.from, b.to
            )));
        }
        if b.from <= last {
            return Err(Invalid("prize bands must be in rank order and not overlap".into()));
        }
        last = b.to;
        if b.reward.is_empty() {
            return Err(Invalid(format!(
                "the prize for {} is empty",
                PrizeLine::of(b).ranks()
            )));
        }
        if !(0..=TOURNAMENT_GOLD_MAX).contains(&b.reward.gold) {
            return Err(Invalid(format!("prize gold must be 0 to {TOURNAMENT_GOLD_MAX}")));
        }
        if b.reward.weapons.len() > crate::REWARD_WEAPONS_MAX {
            return Err(Invalid("too many weapons in one prize".into()));
        }
    }
    Ok(())
}

/// Text of a tournament prize gift: "1st in Spring Clash".
pub fn prize_message(rank: u32, title: &str) -> String {
    format!("{} in {title}", ordinal(rank))
}

/// Summary of a whole prize table, for the admin log.
pub fn describe_bands(bands: &[PrizeBand]) -> String {
    bands
        .iter()
        .map(|b| {
            let l = PrizeLine::of(b);
            format!("{}: {}", l.ranks(), l.text)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(from: u32, to: u32, gold: i64) -> PrizeBand {
        PrizeBand {
            from,
            to,
            reward: Reward {
                gold,
                ..Default::default()
            },
        }
    }

    #[test]
    fn ordinals_read_well() {
        let got: Vec<_> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111].iter().map(|n| ordinal(*n)).collect();
        assert_eq!(got, ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st", "111th"]);
        assert_eq!(PrizeLine::of(&band(4, 10, 50)).ranks(), "4th to 10th");
    }

    #[test]
    fn bands_are_checked() {
        assert!(check_bands(&[band(1, 1, 500), band(2, 3, 200), band(4, 10, 50)]).is_ok());
        assert!(check_bands(&[]).is_err());
        assert!(check_bands(&[band(1, 3, 100), band(3, 5, 10)]).is_err(), "overlap");
        assert!(check_bands(&[band(2, 3, 100), band(1, 1, 10)]).is_err(), "order");
        assert!(check_bands(&[band(1, 1, 0)]).is_err(), "empty prize");
        assert!(check_bands(&[band(0, 1, 10)]).is_err());
        assert!(check_bands(&[band(1, 5000, 10)]).is_err());
        let w = PrizeBand {
            from: 1,
            to: 1,
            reward: Reward {
                weapons: vec![godcore::weapons::Weapon::Crossbow],
                ..Default::default()
            },
        };
        assert!(check_bands(&[w]).is_ok(), "a weapon alone is a prize");
    }
}
