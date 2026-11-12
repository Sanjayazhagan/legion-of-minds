//! Leagues, seasons, clans, troops lent to clanmates, and clan wars.

use crate::{bad, finite, Invalid, Placement, PublicPlayer, RaidResult};
use godcore::scenario::can_deploy;
use godcore::troop::Troop;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use uuid::Uuid;

// ---------------------------------------------------------------- leagues

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum League {
    Bronze,
    Silver,
    Gold,
    Crystal,
    Master,
    Champion,
    Legend,
}

pub const LEAGUES: [League; 7] = [
    League::Bronze,
    League::Silver,
    League::Gold,
    League::Crystal,
    League::Master,
    League::Champion,
    League::Legend,
];

impl League {
    /// Trophies needed to be in this league.
    pub fn floor(self) -> i32 {
        match self {
            League::Bronze => 0,
            League::Silver => 400,
            League::Gold => 800,
            League::Crystal => 1200,
            League::Master => 1600,
            League::Champion => 2000,
            League::Legend => 2600,
        }
    }

    pub fn of(trophies: i32) -> League {
        LEAGUES
            .iter()
            .rev()
            .copied()
            .find(|l| trophies >= l.floor())
            .unwrap_or(League::Bronze)
    }

    pub fn next(self) -> Option<League> {
        LEAGUES.get(self as usize + 1).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            League::Bronze => "Bronze",
            League::Silver => "Silver",
            League::Gold => "Gold",
            League::Crystal => "Crystal",
            League::Master => "Master",
            League::Champion => "Champion",
            League::Legend => "Legend",
        }
    }

    /// Extra online gold per star for a raid won in this league.
    pub fn bonus_per_star(self) -> i64 {
        [0, 5, 10, 20, 30, 45, 60][self as usize]
    }

    pub fn win_bonus(self, stars: u32) -> i64 {
        self.bonus_per_star() * stars.min(3) as i64
    }

    /// Online gold paid to everyone who ends a season in this league.
    pub fn season_reward(self) -> i64 {
        [50, 150, 300, 600, 1000, 1600, 2500][self as usize]
    }
}

/// At the end of a season, trophies above this are halved, so everyone
/// climbs again but the best players start ahead.
pub const SEASON_FLOOR: i32 = 800;

pub fn season_reset(trophies: i32) -> i32 {
    if trophies > SEASON_FLOOR {
        SEASON_FLOOR + (trophies - SEASON_FLOOR) / 2
    } else {
        trophies
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Season {
    pub number: i32,
    pub started_at: String,
    pub ends_at: String,
}

/// Where a player finished a season.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeasonResult {
    pub season: i32,
    pub trophies: i32,
    pub league: League,
    /// 1 is the top of the leaderboard.
    pub rank: i64,
    pub reward: i64,
}

// ---------------------------------------------------------------- clans

pub const CLAN_MAX_MEMBERS: usize = 30;
pub const CLAN_NAME_MIN: usize = 3;
pub const CLAN_NAME_MAX: usize = 24;
pub const CLAN_DESC_MAX: usize = 120;
pub const CHAT_MAX: usize = 200;
/// Troops each member can lend to the clan at once.
pub const MAX_LOANS: usize = 2;
/// Borrowed troops one raid can take along, inside the army limit.
pub const MAX_BORROWED: usize = 2;
/// Attacks each member gets in a war.
pub const WAR_ATTACKS: u32 = 2;
/// Bases per side in a war, at most.
pub const WAR_MAX_SIZE: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClanRole {
    Leader,
    Elder,
    Member,
}

impl ClanRole {
    pub fn key(self) -> &'static str {
        match self {
            ClanRole::Leader => "leader",
            ClanRole::Elder => "elder",
            ClanRole::Member => "member",
        }
    }

    pub fn parse(s: &str) -> ClanRole {
        match s {
            "leader" => ClanRole::Leader,
            "elder" => ClanRole::Elder,
            _ => ClanRole::Member,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ClanRole::Leader => "Leader",
            ClanRole::Elder => "Elder",
            ClanRole::Member => "Member",
        }
    }

    /// Leaders and elders run wars and can remove members.
    pub fn officer(self) -> bool {
        self != ClanRole::Member
    }
}

/// The player's own clan, on their profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClanTag {
    pub id: Uuid,
    pub name: String,
    pub role: ClanRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClanSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// Anyone can join an open clan.
    pub open: bool,
    pub members: u32,
    /// All members' trophies added up.
    pub trophies: i64,
    pub war_wins: i32,
    pub war_losses: i32,
    pub war_draws: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClanMember {
    pub player: PublicPlayer,
    pub role: ClanRole,
    /// Troops this member lends to the clan right now.
    pub lending: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClanView {
    pub clan: ClanSummary,
    pub members: Vec<ClanMember>,
    /// The clan is waiting for a war opponent.
    pub war_searching: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateClan {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub open: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatPost {
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    /// None once the writer has deleted their account.
    pub user_id: Option<Uuid>,
    pub from: String,
    pub body: String,
    pub at: String,
}

/// Troops to lend to the clan. Replaces what you lent before.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lend {
    pub troops: Vec<Troop>,
}

/// A troop a clanmate lends.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Loan {
    pub id: Uuid,
    pub owner: PublicPlayer,
    pub troop: Troop,
}

/// A borrowed troop in a raid: which loan, and where it starts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Borrowed {
    pub loan_id: Uuid,
    pub x: f32,
    pub y: f32,
}

/// A clan or chat text: trimmed, control characters dropped, length checked.
pub fn clean_text(s: &str, min: usize, max: usize, what: &str) -> Result<String, Invalid> {
    let t: String = s
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let n = t.chars().count();
    if n < min || n > max {
        return bad(format!("{what} must be {min} to {max} characters"));
    }
    Ok(t)
}

/// The full army for a raid: your own troops, then borrowed ones, in that
/// order on both machines so the battle comes out the same.
pub fn with_borrowed(
    own: Vec<Placement>,
    borrowed: &[Borrowed],
    troop_of: impl Fn(Uuid) -> Option<Troop>,
) -> Result<Vec<Placement>, Invalid> {
    if borrowed.len() > MAX_BORROWED {
        return bad(format!("take at most {MAX_BORROWED} borrowed troops"));
    }
    if own.len() + borrowed.len() > crate::MAX_ARMY {
        return bad(format!("send at most {} troops in all", crate::MAX_ARMY));
    }
    let mut army = own;
    let mut seen = Vec::new();
    for b in borrowed {
        if seen.contains(&b.loan_id) {
            return bad("each borrowed troop can go once");
        }
        seen.push(b.loan_id);
        if !finite(b.x, b.y) || !can_deploy(b.x, b.y) {
            return bad("troops must be placed outside the red zone");
        }
        let troop = troop_of(b.loan_id)
            .ok_or_else(|| Invalid("that troop isn't lent to your clan any more".into()))?;
        if army.iter().any(|p| p.troop.id == troop.id && p.troop.name == troop.name) {
            return bad("that troop is already in the army");
        }
        army.push(Placement {
            troop,
            x: b.x,
            y: b.y,
        });
    }
    if army.is_empty() {
        return bad("send at least one troop");
    }
    Ok(army)
}

// ---------------------------------------------------------------- wars

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarState {
    /// Bases are locked in; attacks open at `battle_at`.
    Preparation,
    Battle,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarResult {
    Won,
    Lost,
    Draw,
}

impl WarResult {
    /// Online gold each member who was in the war gets.
    pub fn reward(self) -> i64 {
        match self {
            WarResult::Won => 500,
            WarResult::Draw => 250,
            WarResult::Lost => 100,
        }
    }
}

/// Stars first, then average destruction on the enemy's bases.
pub fn war_order(stars_a: u32, dest_a: f32, stars_b: u32, dest_b: f32) -> Ordering {
    stars_a
        .cmp(&stars_b)
        .then(dest_a.partial_cmp(&dest_b).unwrap_or(Ordering::Equal))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarBase {
    pub player: PublicPlayer,
    /// 1 is the strongest base on its side.
    pub position: u32,
    pub town_hall: u32,
    /// Best stars the other side has taken from this base.
    pub best_stars: u32,
    pub best_destruction: f32,
    /// Attacks this player has used.
    pub attacks_used: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarSide {
    pub clan_id: Uuid,
    pub name: String,
    /// Stars this side has taken from the other.
    pub stars: u32,
    /// Average best destruction on the other side's bases.
    pub destruction: f32,
    pub bases: Vec<WarBase>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarView {
    pub id: Uuid,
    pub state: WarState,
    pub battle_at: String,
    pub ends_at: String,
    pub us: WarSide,
    pub them: WarSide,
    /// False when you joined the clan after the war began.
    pub in_war: bool,
    pub my_attacks_left: u32,
    pub result: Option<WarResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarSearch {
    pub searching: bool,
    /// Set when an opponent was found straight away.
    pub war: Option<WarView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WarOutcome {
    pub attack_id: Uuid,
    pub result: RaidResult,
    /// Stars this attack added for the clan.
    pub new_stars: u32,
    pub client_matched: Option<bool>,
    pub war: WarView,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leagues_follow_trophies() {
        assert_eq!(League::of(0), League::Bronze);
        assert_eq!(League::of(399), League::Bronze);
        assert_eq!(League::of(400), League::Silver);
        assert_eq!(League::of(2599), League::Champion);
        assert_eq!(League::of(9000), League::Legend);
        assert_eq!(League::Bronze.next(), Some(League::Silver));
        assert_eq!(League::Legend.next(), None);
        for w in LEAGUES.windows(2) {
            assert!(w[0].floor() < w[1].floor());
            assert!(w[0].season_reward() < w[1].season_reward());
        }
        assert_eq!(League::Gold.win_bonus(3), 30);
        assert_eq!(League::Bronze.win_bonus(3), 0);
    }

    #[test]
    fn season_reset_halves_the_top() {
        assert_eq!(season_reset(500), 500);
        assert_eq!(season_reset(800), 800);
        assert_eq!(season_reset(1000), 900);
        assert_eq!(season_reset(2801), 1800);
    }

    #[test]
    fn clan_text_is_cleaned() {
        assert_eq!(clean_text("  Iron\t Wolves ", 3, 24, "name").unwrap(), "Iron Wolves");
        assert!(clean_text("ab", 3, 24, "name").is_err());
        assert!(clean_text(&"x".repeat(25), 3, 24, "name").is_err());
    }

    #[test]
    fn war_order_uses_destruction_on_a_tie() {
        assert_eq!(war_order(5, 0.1, 4, 0.9), Ordering::Greater);
        assert_eq!(war_order(5, 0.5, 5, 0.6), Ordering::Less);
        assert_eq!(war_order(5, 0.5, 5, 0.5), Ordering::Equal);
    }
}
