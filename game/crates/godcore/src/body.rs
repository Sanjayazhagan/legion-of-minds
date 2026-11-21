//! Human limits (sim version 10). Trained troops are people, not machines:
//! they run out of breath, wear down over a long fight, and slow down when
//! badly hurt. How much they can take depends on their class (a Paladin in
//! plate is not an Assassin), and training conditions the body: a troop
//! with more training has more stamina, gets it back faster, wears down
//! slower and reacts quicker.
//!
//! Skeleton guards, towers and scripted fighters don't tire.

use crate::classes::{BaseClass, ClassSpec};

/// One class's body: (untrained, fully conditioned) for the numbers that
/// training improves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodySheet {
    /// Stamina pool.
    pub stamina: (f32, f32),
    /// Stamina back per tick when standing still.
    pub recovery: (f32, f32),
    /// Armour and gear weight: walking costs this much more.
    pub load: f32,
    /// Ticks between first seeing something and acting on it.
    pub react: (f32, f32),
}

impl BaseClass {
    pub fn body(self) -> BodySheet {
        let s = |stamina, recovery, load, react| BodySheet { stamina, recovery, load, react };
        match self {
            BaseClass::Fighter => s((100.0, 130.0), (1.0, 1.4), 1.1, (4.0, 3.0)),
            BaseClass::Archer => s((80.0, 110.0), (1.1, 1.5), 1.0, (4.0, 3.0)),
            BaseClass::Healer => s((70.0, 95.0), (1.2, 1.6), 1.0, (4.0, 4.0)),
            BaseClass::Mage => s((60.0, 85.0), (1.2, 1.6), 1.0, (4.0, 3.0)),
            BaseClass::Assassin => s((90.0, 120.0), (1.3, 1.8), 0.8, (3.0, 2.0)),
            BaseClass::Paladin => s((120.0, 150.0), (0.8, 1.1), 1.3, (5.0, 4.0)),
            BaseClass::Guard => s((120.0, 150.0), (0.9, 1.2), 1.3, (5.0, 4.0)),
            BaseClass::Sentry => s((80.0, 105.0), (1.1, 1.5), 1.0, (4.0, 3.0)),
            BaseClass::Keeper => s((75.0, 100.0), (1.2, 1.6), 1.0, (4.0, 4.0)),
            BaseClass::Warlock => s((65.0, 90.0), (1.2, 1.6), 1.0, (4.0, 3.0)),
            BaseClass::Hunter => s((110.0, 140.0), (1.2, 1.6), 0.9, (3.0, 2.0)),
        }
    }
}

/// Training steps for a fully conditioned body (a maxed level 5 troop).
pub const CONDITIONING_STEPS: u64 = 3_500_000;

/// Conditioning (0 to 1) after this much training. Fast at first, then
/// slower: about 0.3 at the level 1 cap, 0.6 at level 3, 1 at level 5.
pub fn conditioning_of(steps: u64) -> f32 {
    (steps.min(CONDITIONING_STEPS) as f32 / CONDITIONING_STEPS as f32).sqrt()
}

/// A troop's body as it enters a battle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub stamina: f32,
    pub recovery: f32,
    pub load: f32,
    pub react: u32,
    /// Share of stamina spent that comes off the pool for the rest of the battle.
    pub wear: f32,
}

/// Wear for an untrained body: every 20 stamina spent costs 1 of the pool.
/// Full conditioning halves it.
const WEAR: f32 = 0.05;

impl Body {
    /// The body of a class mix, blended by share, at a conditioning level.
    pub fn of(spec: &ClassSpec) -> Body {
        let c = spec.conditioning.clamp(0.0, 1.0);
        let at = |p: (f32, f32)| p.0 + (p.1 - p.0) * c;
        let mut b = Body { stamina: 0.0, recovery: 0.0, load: 0.0, react: 0, wear: WEAR * (1.0 - 0.5 * c) };
        let mut react = 0.0;
        let total: f32 = spec.parents.iter().map(|p| p.1).sum();
        for &(class, share) in &spec.parents {
            let s = class.body();
            let w = share / total;
            b.stamina += at(s.stamina) * w;
            b.recovery += at(s.recovery) * w;
            b.load += s.load * w;
            react += at(s.react) * w;
        }
        b.react = react.round().max(1.0) as u32;
        b
    }
}

// What things cost, in stamina.
/// Walking one tick (times the armour load).
pub const COST_WALK: f32 = 0.25;
/// A melee swing.
pub const COST_SWING: f32 = 3.0;
/// A shot, a spell or a heal.
pub const COST_SHOT: f32 = 1.5;
pub const COST_DODGE: f32 = 12.0;
pub const COST_BLOCK: f32 = 5.0;
/// Each tick the guard stays up.
pub const COST_HOLD: f32 = 0.2;
pub const COST_CHARGE: f32 = 15.0;
pub const COST_SKILL: f32 = 10.0;
/// Cloak, blink, snare and ward (concentration more than muscle).
pub const COST_FOCUS: f32 = 8.0;

/// Ticks without exertion before a troop standing still gets its breath
/// back fully. Between swings it barely recovers.
pub const BREATHER: u32 = 10;
/// Recovery while walking, and while fighting (or just after), as a share
/// of resting.
pub const REST_WALKING: f32 = 0.35;
pub const REST_FIGHTING: f32 = 0.2;

/// Below this share of its stamina pool a troop starts to tire.
pub const TIRED_AT: f32 = 0.5;
/// Fully tired (empty): speed, attack rate and damage left.
pub const TIRED_SPEED: f32 = 0.7;
pub const TIRED_COOLDOWN: f32 = 1.5;
pub const TIRED_DMG: f32 = 0.8;
/// Extra chance to miss a shot when fully tired.
pub const TIRED_MISS: f32 = 0.15;
/// Wear never takes more than this share of the pool.
pub const WEAR_FLOOR: f32 = 0.5;

/// Below this share of health a troop is in pain.
pub const PAIN_AT: f32 = 0.5;
/// Speed and recovery lost at the edge of death.
pub const PAIN_SLOW: f32 = 0.25;

/// How tired (0 fresh to 1 empty) a troop with this much stamina is.
pub fn tiredness(stamina: f32, pool: f32) -> f32 {
    if pool <= 0.0 {
        return 0.0;
    }
    ((TIRED_AT - stamina / pool) / TIRED_AT).clamp(0.0, 1.0)
}

/// How much pain (0 to 1) a troop with this much health is in.
pub fn pain(hp: f32, max_hp: f32) -> f32 {
    if max_hp <= 0.0 {
        return 0.0;
    }
    ((PAIN_AT - hp / max_hp) / PAIN_AT).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditioning_grows_with_training() {
        assert_eq!(conditioning_of(0), 0.0);
        assert!((conditioning_of(300_000) - 0.29).abs() < 0.01);
        assert!(conditioning_of(1_200_000) > 0.55);
        assert_eq!(conditioning_of(10_000_000), 1.0);
    }

    #[test]
    fn mixes_blend_their_bodies() {
        let mut s = ClassSpec::hybrid(BaseClass::Paladin, BaseClass::Assassin, 0.5);
        let b = Body::of(&s);
        assert!((b.stamina - 105.0).abs() < 1e-3);
        assert!((b.load - 1.05).abs() < 1e-3);
        s.conditioning = 1.0;
        let t = Body::of(&s);
        assert!(t.stamina > b.stamina && t.recovery > b.recovery && t.wear < b.wear);
        assert_eq!(t.react, 3);
    }

    #[test]
    fn tanks_carry_more_than_casters() {
        let p = Body::of(&BaseClass::Paladin.spec());
        let m = Body::of(&BaseClass::Mage.spec());
        assert!(p.stamina > m.stamina && p.load > m.load);
        assert!(p.recovery < m.recovery);
    }
}
