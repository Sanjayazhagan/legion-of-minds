//! Human moves (sim version 11): the things a person can do in a fight that
//! troops couldn't before, unlocked as the brain grows. A bigger brain has
//! room to learn when each one is worth it.
//!
//! | Brain | Moves |
//! |---|---|
//! | 128+ | Sprint, Sneak, Back-step |
//! | 256+ | Strafe left/right, Shove, Crouch |
//! | 512+ | Jump, Parry, Feint |
//! | 1024+ | Grab and throw, Throw a rock |
//!
//! Every brain has an output for each of them from the start; a locked move
//! is simply never allowed (masked), so unlocking one changes nothing a
//! troop already does until it learns to use it.

use crate::sim::*;

/// Brain size each tier needs (tier 0 = no human moves).
pub const TIER_BRAIN: [usize; 5] = [0, 128, 256, 512, 1024];

/// The tier a brain of `h` neurons per layer unlocks.
pub fn tier_for_brain(h: usize) -> u8 {
    TIER_BRAIN.iter().rposition(|&b| h >= b).unwrap_or(0) as u8
}

/// (action, name, tier) for every human move, in action order.
pub const HUMAN_MOVES: [(u8, &str, u8); N_HUMAN] = [
    (ACT_SPRINT, "Sprint", 1),
    (ACT_SNEAK, "Sneak", 1),
    (ACT_BACKSTEP, "Back-step", 1),
    (ACT_STRAFE_L, "Strafe left", 2),
    (ACT_STRAFE_R, "Strafe right", 2),
    (ACT_SHOVE, "Shove", 2),
    (ACT_CROUCH, "Crouch", 2),
    (ACT_JUMP, "Jump", 3),
    (ACT_PARRY, "Parry", 3),
    (ACT_FEINT, "Feint", 3),
    (ACT_GRAB, "Grab and throw", 4),
    (ACT_ROCK, "Throw a rock", 4),
];

/// The tier action `a` needs (0 for actions that aren't human moves).
pub fn tier_of(a: u8) -> u8 {
    HUMAN_MOVES.iter().find(|m| m.0 == a).map_or(0, |m| m.2)
}

/// Names of the human moves a brain of `h` neurons has.
pub fn unlocked(h: usize) -> Vec<&'static str> {
    let t = tier_for_brain(h);
    HUMAN_MOVES.iter().filter(|m| m.2 <= t).map(|m| m.1).collect()
}

/// What the next tier adds, and the brain size it needs.
pub fn next_unlock(h: usize) -> Option<(usize, Vec<&'static str>)> {
    let t = tier_for_brain(h) + 1;
    let need = *TIER_BRAIN.get(t as usize)?;
    Some((need, HUMAN_MOVES.iter().filter(|m| m.2 == t).map(|m| m.1).collect()))
}

/// Does action `a` move the troop along the ground (for footsteps, aim and
/// getting breath back)?
pub fn is_step(a: u8) -> bool {
    matches!(a, 1..=8 | ACT_BACKSTEP | ACT_STRAFE_L | ACT_STRAFE_R)
}

// ---- Tuning

/// How long a pace lasts unless changed (ticks).
pub const PACE_TICKS: u32 = 30;
pub const SPRINT_SPEED: f32 = 1.6;
pub const SPRINT_EFFORT: f32 = 3.0;
pub const SPRINT_SOUND: f32 = 1.5;
pub const SNEAK_SPEED: f32 = 0.5;
pub const SNEAK_EFFORT: f32 = 0.6;
/// Sprinting stops below this share of the stamina pool.
pub const SPRINT_MIN_BREATH: f32 = 0.25;
pub const BACKSTEP_SPEED: f32 = 0.7;
pub const STRAFE_SPEED: f32 = 0.8;
/// Strafing circles an enemy this close.
pub const STRAFE_RANGE: f32 = 6.0;
/// Shove and grab reach (centre to centre).
pub const GRAPPLE_REACH: f32 = 1.8;
pub const SHOVE_DIST: f32 = 1.5;
pub const SHOVE_STUN: u32 = 3;
pub const SHOVE_CD: u32 = 20;
pub const COST_SHOVE: f32 = 6.0;
pub const CROUCH_TICKS: u32 = 20;
pub const CROUCH_SPEED: f32 = 0.5;
/// Extra chance a shot at a crouching troop misses.
pub const CROUCH_MISS: f32 = 0.3;
pub const JUMP_DIST: f32 = 2.2;
pub const JUMP_INVULN: u32 = 3;
pub const JUMP_CD: u32 = 25;
pub const COST_JUMP: f32 = 14.0;
pub const PARRY_TICKS: u32 = 4;
pub const PARRY_STUN: u32 = 8;
pub const PARRY_CD: u32 = 30;
/// A feinted troop that rolls, raises its guard, parries or jumps in the
/// next few ticks is caught off balance instead.
pub const FEINT_TICKS: u32 = 6;
pub const FEINT_STUN: u32 = 5;
pub const FEINT_CD: u32 = 20;
pub const COST_FEINT: f32 = 2.0;
pub const GRAB_THROW: f32 = 3.0;
pub const GRAB_STUN: u32 = 8;
pub const GRAB_DMG: f32 = 0.5;
pub const GRAB_CD: u32 = 60;
pub const COST_GRAB: f32 = 18.0;
pub const ROCK_RANGE: f32 = 8.0;
pub const ROCK_DMG: f32 = 3.0;
pub const ROCK_CD: u32 = 30;
pub const COST_ROCK: f32 = 3.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_brain_size() {
        assert_eq!([64, 96, 128, 192, 256, 384, 512, 768, 1024, 2048].map(tier_for_brain), [0, 0, 1, 1, 2, 2, 3, 3, 4, 4]);
        assert!(unlocked(64).is_empty());
        assert_eq!(unlocked(128), ["Sprint", "Sneak", "Back-step"]);
        assert_eq!(unlocked(1024).len(), N_HUMAN);
        assert_eq!(next_unlock(256).unwrap(), (512, vec!["Jump", "Parry", "Feint"]));
        assert_eq!(next_unlock(2048), None);
        for (k, m) in HUMAN_MOVES.iter().enumerate() {
            assert_eq!(m.0 as usize, ACT_SPRINT as usize + k);
        }
        assert_eq!(ACT_ROCK as usize + 1, N_ACTIONS);
    }
}
