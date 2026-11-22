//! Human limits (sim version 10). Trained troops are people, not machines:
//! they run out of breath, wear down over a long fight, and slow down when
//! badly hurt. How much they can take depends on their class (a Paladin in
//! plate is not an Assassin), and training conditions the body: a troop
//! with more training has more stamina, gets it back faster, wears down
//! slower and reacts quicker.
//!
//! Skeleton guards, towers and scripted fighters don't tire.

use crate::classes::{BaseClass, ClassSpec};
use crate::rng::Rng;
use crate::weapons::Weapon;
use serde::{Deserialize, Serialize};

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
    /// Arrows carried (crossbows only, 0 otherwise).
    pub arrows: f32,
    /// Mana (fire staffs only, 0 otherwise).
    pub mana: f32,
    /// Stamina cost of each attack, from the weapon's weight.
    pub swing: f32,
    /// Born traits.
    pub traits: Traits,
}

/// Arrows a troop of each class carries, and mana (sim version 10).
fn quiver(c: BaseClass) -> f32 {
    match c {
        BaseClass::Archer => 90.0,
        BaseClass::Sentry => 80.0,
        _ => 60.0,
    }
}

fn mana_pool(c: BaseClass) -> f32 {
    match c {
        BaseClass::Mage | BaseClass::Warlock => 100.0,
        _ => 80.0,
    }
}

/// Conditioning adds up to this share of arrows and mana.
pub const SUPPLY_TRAINED: f32 = 0.25;
/// Mana a spell costs, a Fireball costs 3 times as much, and mana back per tick.
pub const SPELL_MANA: f32 = 3.0;
pub const MANA_REGEN: f32 = 0.3;
/// With no arrows (or no mana for a spell) a troop fights in close: this
/// reach and this share of its damage.
pub const MAKESHIFT_RANGE: f32 = 1.3;
pub const MAKESHIFT_DMG: f32 = 0.4;

/// How heavy a weapon is to swing: stamina per attack, times the base cost,
/// and armour weight it adds.
pub fn weapon_weight(w: Weapon) -> (f32, f32) {
    match w {
        Weapon::SwordShield => (1.0, 0.2),
        Weapon::Greataxe | Weapon::Greatsword => (1.3, 0.0),
        Weapon::Daggers => (0.7, 0.0),
        Weapon::Crossbow => (1.2, 0.0),
        Weapon::FireStaff | Weapon::HolyWand => (0.8, 0.0),
    }
}

/// What a recruit is born with (sim version 10). Every value is a
/// difference from the class's normal, evened out by what each trait is
/// worth to the class, so traits make troops different, not better. Troops recruited
/// before traits existed are average in everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Traits {
    /// Walking speed (+-8%).
    pub speed: f32,
    /// Damage (+-10%).
    pub strength: f32,
    /// Health (+-10%).
    pub toughness: f32,
    /// Stamina pool (+-15%).
    pub lungs: f32,
    /// Stamina recovery (+-15%).
    pub recovery: f32,
    /// Sight range (+-2 steps).
    pub eyesight: f32,
    /// Reaction time (+-1 tick, negative is quicker).
    pub reflexes: f32,
    /// Chance to miss a shot (+-5%, negative is steadier).
    pub aim: f32,
}

/// How far each trait can go from average.
const TRAIT_SPAN: [f32; 8] = [0.08, 0.10, 0.10, 0.15, 0.15, 2.0, 1.0, 0.05];
/// Drill win points each trait is worth at full span (melee, ranged,
/// caster, other): the average of two measurements, 600k-step troops under
/// the first v10 tuning and 1.2M-step troops under this one, 2,000 drills
/// each. Speed, eyesight and reflexes barely matter; strength does.
const TRAIT_VALUE: [[f32; 8]; 4] = [
    [0.3, 10.7, 7.6, 2.8, 1.2, 0.0, 0.1, 0.0],
    [1.3, 12.2, 3.9, 0.6, 1.8, 1.9, 0.0, 4.6],
    [0.0, 6.5, 4.4, 0.1, 0.1, 0.7, 0.4, 2.0],
    [0.5, 9.8, 5.3, 1.2, 1.0, 0.9, 0.2, 2.2],
];

fn trait_value(class: BaseClass) -> [f32; 8] {
    match class {
        BaseClass::Fighter | BaseClass::Paladin | BaseClass::Guard | BaseClass::Assassin => TRAIT_VALUE[0],
        BaseClass::Archer | BaseClass::Sentry | BaseClass::Hunter => TRAIT_VALUE[1],
        BaseClass::Mage | BaseClass::Warlock => TRAIT_VALUE[2],
        BaseClass::Healer | BaseClass::Keeper => TRAIT_VALUE[3],
    }
}

/// Names for a strong and a weak trait.
const TRAIT_WORDS: [(&str, &str); 8] = [
    ("Quick", "Slow"),
    ("Strong", "Weak arm"),
    ("Tough", "Frail"),
    ("Iron lungs", "Short of breath"),
    ("Quick to recover", "Slow to recover"),
    ("Sharp eyes", "Short-sighted"),
    ("Fast reflexes", "Slow reflexes"),
    ("Steady hands", "Shaky aim"),
];

impl Traits {
    /// The traits a recruit of this class with this seed is born with.
    /// Each trait is drawn, then the draw is evened out by what each trait
    /// is worth to the class (measured in drills: strength and toughness
    /// matter far more than eyesight), so a Strong, Tough recruit pays for
    /// it elsewhere and no roll is clearly best.
    pub fn roll_for(seed: u64, spec: &ClassSpec) -> Traits {
        let total: f32 = spec.parents.iter().map(|p| p.1).sum();
        let mut w = [0.0f32; 8];
        for &(class, share) in &spec.parents {
            let v = trait_value(class);
            for k in 0..8 {
                w[k] += v[k] * share / total;
            }
        }
        Self::roll_weighted(seed, w)
    }

    /// Born traits evened out for an average class (see `roll_for`).
    pub fn roll(seed: u64) -> Traits {
        Self::roll_weighted(seed, TRAIT_VALUE[3])
    }

    fn roll_weighted(seed: u64, w: [f32; 8]) -> Traits {
        let mut rng = Rng::new(seed ^ 0x7472_6169_7473);
        let mut v = [0.0f32; 8];
        for x in v.iter_mut() {
            *x = rng.range(-1.0, 1.0);
        }
        // Take off the part of the draw that makes the troop better or worse
        // overall, so what it is worth adds up to zero.
        let ww: f32 = w.iter().map(|x| x * x).sum();
        let vw: f32 = (0..8).map(|k| v[k] * w[k]).sum();
        for k in 0..8 {
            v[k] -= vw / ww * w[k];
        }
        // Shrink (rather than cut) so the worth still adds up to zero.
        let big = v.iter().map(|x| x.abs()).fold(1.0f32, f32::max);
        let mut out = [0.0f32; 8];
        for k in 0..8 {
            out[k] = v[k] / big * TRAIT_SPAN[k];
        }
        // For reflexes and aim, lower is better: flip so a positive draw is good.
        Traits {
            speed: out[0],
            strength: out[1],
            toughness: out[2],
            lungs: out[3],
            recovery: out[4],
            eyesight: out[5],
            reflexes: -out[6],
            aim: -out[7],
        }
    }

    /// What this troop's traits are worth to its class, in drill win
    /// points (0 for a fair roll).
    pub fn worth(&self, spec: &ClassSpec) -> f32 {
        let total: f32 = spec.parents.iter().map(|p| p.1).sum();
        let sc = self.scores();
        let mut sum = 0.0;
        for &(class, share) in &spec.parents {
            let v = trait_value(class);
            for k in 0..8 {
                sum += v[k] * sc[k] * share / total;
            }
        }
        sum
    }

    /// Each trait as a share of its span, positive meaning better.
    pub fn scores(&self) -> [f32; 8] {
        let v = [self.speed, self.strength, self.toughness, self.lungs, self.recovery, self.eyesight, -self.reflexes, -self.aim];
        let mut o = [0.0; 8];
        for k in 0..8 {
            o[k] = v[k] / TRAIT_SPAN[k];
        }
        o
    }

    /// The strongest and weakest traits in words, e.g. "Quick, Iron lungs;
    /// Short-sighted". Empty for an average troop.
    pub fn words(&self) -> String {
        let sc = self.scores();
        let mut idx: Vec<usize> = (0..8).filter(|&k| sc[k].abs() >= 0.35).collect();
        idx.sort_by(|&a, &b| sc[b].partial_cmp(&sc[a]).unwrap());
        let good: Vec<&str> = idx.iter().filter(|&&k| sc[k] > 0.0).take(2).map(|&k| TRAIT_WORDS[k].0).collect();
        let bad: Vec<&str> = idx.iter().rev().filter(|&&k| sc[k] < 0.0).take(1).map(|&k| TRAIT_WORDS[k].1).collect();
        match (good.is_empty(), bad.is_empty()) {
            (true, true) => String::new(),
            (false, true) => good.join(", "),
            (true, false) => bad.join(", "),
            (false, false) => format!("{}; {}", good.join(", "), bad.join(", ")),
        }
    }
}

/// Wear for an untrained body: every 20 stamina spent costs 1 of the pool.
/// Full conditioning halves it.
const WEAR: f32 = 0.05;

impl Body {
    /// The body of a class mix, blended by share, at a conditioning level.
    pub fn of(spec: &ClassSpec) -> Body {
        let c = spec.conditioning.clamp(0.0, 1.0);
        let at = |p: (f32, f32)| p.0 + (p.1 - p.0) * c;
        let t = spec.traits;
        let mut b = Body {
            stamina: 0.0,
            recovery: 0.0,
            load: 0.0,
            react: 0,
            wear: WEAR * (1.0 - 0.5 * c),
            arrows: 0.0,
            mana: 0.0,
            swing: 1.0,
            traits: t,
        };
        let (mut react, mut arrows, mut mana) = (0.0, 0.0, 0.0);
        let total: f32 = spec.parents.iter().map(|p| p.1).sum();
        for &(class, share) in &spec.parents {
            let s = class.body();
            let w = share / total;
            b.stamina += at(s.stamina) * w;
            b.recovery += at(s.recovery) * w;
            b.load += s.load * w;
            react += at(s.react) * w;
            arrows += quiver(class) * w;
            mana += mana_pool(class) * w;
        }
        let supply = 1.0 + SUPPLY_TRAINED * c;
        let weapon = spec.weapon();
        if weapon == Weapon::Crossbow {
            b.arrows = (arrows * supply).round();
        }
        if weapon == Weapon::FireStaff {
            b.mana = (mana * supply).round();
        }
        let (swing, load) = weapon_weight(weapon);
        b.swing = swing;
        b.load += load;
        b.stamina *= 1.0 + t.lungs;
        b.recovery *= 1.0 + t.recovery;
        b.react = (react + t.reflexes).round().max(1.0) as u32;
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

/// Tiredness that carries between raids (sim version 10, stage C). The
/// share of the pool a raid wore away comes back with the troop, up to this
/// much; then it rests off over `REST_SECS` of real time. Training, drills,
/// defending and wars ignore it.
pub const CARRY_MAX: f32 = 0.4;
/// Real seconds to rest off carried tiredness completely (4 hours).
pub const REST_SECS: u64 = 4 * 3600;

/// Hundredths, so the client and server agree exactly.
fn hundredths(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

/// What a troop that ended a raid with this worn ceiling carries home.
pub fn carried(pool: f32, max_stamina: f32) -> f32 {
    if pool <= 0.0 {
        return 0.0;
    }
    hundredths((1.0 - max_stamina / pool).clamp(0.0, CARRY_MAX))
}

/// What is left of `carried` after resting this many seconds.
pub fn rested(carried: f32, secs: u64) -> f32 {
    let left = 1.0 - (secs.min(REST_SECS) as f32) / REST_SECS as f32;
    hundredths(carried.clamp(0.0, CARRY_MAX) * left)
}

/// Seconds of rest until a troop carrying this much is fresh again.
pub fn rest_left(carried: f32, secs: u64) -> u64 {
    if carried <= 0.0 {
        return 0;
    }
    REST_SECS.saturating_sub(secs)
}

/// Below this share of health a troop is in pain.
pub const PAIN_AT: f32 = 0.5;
/// Speed and recovery lost at the edge of death.
pub const PAIN_SLOW: f32 = 0.25;

/// Injuries (sim version 10). Each hit on a trained troop lands on its legs,
/// arms or torso. Damage to the legs slows it, to the arms weakens and slows
/// its attacks, fully once that part has taken this share of its health.
pub const INJURY_FULL: f32 = 0.6;
/// Share of hits on the legs and on the arms (the rest hit the torso).
pub const HIT_LEGS: f32 = 0.3;
pub const HIT_ARMS: f32 = 0.3;
/// Speed left with ruined legs; damage left and cooldown with ruined arms.
pub const LAME_SPEED: f32 = 0.8;
pub const ARM_DMG: f32 = 0.85;
pub const ARM_COOLDOWN: f32 = 1.15;
/// A hit this big (share of health) opens a wound: bleeding this much a
/// tick, adding up to the most. Any heal stops it.
pub const WOUND: f32 = 0.15;
pub const BLEED: f32 = 0.1;
pub const BLEED_MAX: f32 = 0.3;
/// A heal mends injuries by this share of what it heals.
pub const MEND: f32 = 0.5;

/// How badly (0 to 1) a part that took this much damage is hurt.
pub fn injury(taken: f32) -> f32 {
    (taken / INJURY_FULL).clamp(0.0, 1.0)
}

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
