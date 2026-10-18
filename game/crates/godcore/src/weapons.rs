//! Weapons: each one changes how a troop fights and brings its own skill.
//! Troops get better with a weapon the longer they train holding it
//! (weapon mastery, levels 0 to 5).

use crate::classes::BaseClass;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Weapon {
    SwordShield,
    Greataxe,
    Greatsword,
    Daggers,
    Crossbow,
    FireStaff,
    HolyWand,
}

/// The special move a weapon unlocks (a cooldown ability, MOBA style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Skill {
    /// Heavy hit that stuns the target.
    ShieldBash,
    /// Spin hitting every enemy close by, buildings included.
    Whirlwind,
    /// Slam that hurts and stuns every enemy close by.
    GroundSlam,
    /// Blink next to a target and strike it hard.
    Shadowstep,
    /// Shoot the three nearest enemies in range at once.
    Volley,
    /// A big blast around the target.
    Fireball,
    /// Heal every ally nearby.
    HealingNova,
}

/// Steps of training with a weapon needed for each mastery level.
pub const MASTERY_STEPS: [u64; 6] = [0, 50_000, 150_000, 400_000, 900_000, 1_800_000];
pub const MAX_MASTERY: u8 = 5;

pub fn mastery_level(steps: u64) -> u8 {
    MASTERY_STEPS.iter().rposition(|&s| steps >= s).unwrap_or(0) as u8
}

/// What a weapon does to a troop's numbers.
#[derive(Clone, Copy, Debug)]
pub struct WeaponStats {
    pub dmg: f32,
    pub cooldown: f32,
    pub speed: f32,
    /// Attack range when the weapon's style differs from the class's
    /// (a fighter picking up a crossbow shoots; an archer with daggers stabs).
    pub range: f32,
    pub ranged: bool,
    /// Share of damage still taken while blocking.
    pub block_taken: f32,
    pub skill: Skill,
    pub skill_cd: u32,
    pub cost: u32,
}

impl Weapon {
    pub const ALL: [Weapon; 7] = [
        Weapon::SwordShield,
        Weapon::Greataxe,
        Weapon::Greatsword,
        Weapon::Daggers,
        Weapon::Crossbow,
        Weapon::FireStaff,
        Weapon::HolyWand,
    ];

    pub fn index(self) -> usize {
        Weapon::ALL.iter().position(|w| *w == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            Weapon::SwordShield => "Sword and shield",
            Weapon::Greataxe => "Greataxe",
            Weapon::Greatsword => "Greatsword",
            Weapon::Daggers => "Twin daggers",
            Weapon::Crossbow => "Crossbow",
            Weapon::FireStaff => "Fire staff",
            Weapon::HolyWand => "Holy wand",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Weapon::SwordShield => "Steady hits and the best block. Skill: Shield Bash stuns the target.",
            Weapon::Greataxe => "Big, slower swings. Skill: Whirlwind hits everything close by.",
            Weapon::Greatsword => "The heaviest hits. Skill: Ground Slam stuns everyone close by.",
            Weapon::Daggers => "Fast, light stabs and quicker feet. Skill: Shadowstep blinks in for a huge strike.",
            Weapon::Crossbow => "Shoots from range. Skill: Volley hits the three nearest enemies.",
            Weapon::FireStaff => "Spells from range. Skill: Fireball blasts a whole area.",
            Weapon::HolyWand => "Weak hits, stronger heals (or a small heal for anyone). Skill: Healing Nova.",
        }
    }

    pub fn skill_name(self) -> &'static str {
        match self.stats().skill {
            Skill::ShieldBash => "Shield Bash",
            Skill::Whirlwind => "Whirlwind",
            Skill::GroundSlam => "Ground Slam",
            Skill::Shadowstep => "Shadowstep",
            Skill::Volley => "Volley",
            Skill::Fireball => "Fireball",
            Skill::HealingNova => "Healing Nova",
        }
    }

    pub fn stats(self) -> WeaponStats {
        let base = WeaponStats {
            dmg: 1.0,
            cooldown: 1.0,
            speed: 1.0,
            range: 1.5,
            ranged: false,
            block_taken: 0.5,
            skill: Skill::ShieldBash,
            skill_cd: 90,
            cost: 0,
        };
        match self {
            Weapon::SwordShield => WeaponStats { block_taken: 0.15, skill: Skill::ShieldBash, skill_cd: 90, cost: 80, ..base },
            Weapon::Greataxe => WeaponStats { dmg: 1.25, cooldown: 1.2, range: 1.6, skill: Skill::Whirlwind, skill_cd: 100, cost: 100, ..base },
            Weapon::Greatsword => WeaponStats { dmg: 1.35, cooldown: 1.3, range: 1.7, skill: Skill::GroundSlam, skill_cd: 120, cost: 140, ..base },
            Weapon::Daggers => WeaponStats {
                dmg: 0.75,
                cooldown: 0.65,
                speed: 1.12,
                range: 1.3,
                skill: Skill::Shadowstep,
                skill_cd: 110,
                cost: 120,
                ..base
            },
            Weapon::Crossbow => WeaponStats { range: 5.0, ranged: true, skill: Skill::Volley, skill_cd: 90, cost: 100, ..base },
            Weapon::FireStaff => WeaponStats { range: 4.5, ranged: true, skill: Skill::Fireball, skill_cd: 120, cost: 140, ..base },
            Weapon::HolyWand => WeaponStats { dmg: 0.7, range: 4.0, ranged: true, skill: Skill::HealingNova, skill_cd: 120, cost: 120, ..base },
        }
    }

    /// The weapon a class starts with (free).
    pub fn default_for(c: BaseClass) -> Weapon {
        match c {
            BaseClass::Fighter | BaseClass::Hunter => Weapon::Greataxe,
            BaseClass::Archer | BaseClass::Sentry => Weapon::Crossbow,
            BaseClass::Healer | BaseClass::Keeper => Weapon::HolyWand,
            BaseClass::Guard => Weapon::SwordShield,
            BaseClass::Mage | BaseClass::Warlock => Weapon::FireStaff,
            BaseClass::Assassin => Weapon::Daggers,
            BaseClass::Paladin => Weapon::Greatsword,
        }
    }
}

/// Mastery bonuses: +5% damage per level, skills recharge 6% faster per
/// level, and from level 3 the skill itself is 30% stronger.
pub fn mastery_dmg(level: u8) -> f32 {
    1.0 + 0.05 * level as f32
}

pub fn mastery_skill_cd(level: u8) -> f32 {
    1.0 - 0.06 * level as f32
}

pub fn mastery_skill_power(level: u8) -> f32 {
    if level >= 3 {
        1.3
    } else {
        1.0
    }
}

pub const MASTERY_TITLES: [&str; 6] = ["Novice", "Trained", "Skilled", "Expert", "Master", "Grandmaster"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mastery_levels_follow_training_steps() {
        assert_eq!(mastery_level(0), 0);
        assert_eq!(mastery_level(49_999), 0);
        assert_eq!(mastery_level(50_000), 1);
        assert_eq!(mastery_level(1_000_000), 4);
        assert_eq!(mastery_level(10_000_000), MAX_MASTERY);
    }
}
