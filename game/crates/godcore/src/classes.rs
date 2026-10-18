//! Troop classes. A class is a stat block, a reward function and a mix of
//! training drills. A troop can blend any number of classes of one side by
//! percentage, and a training plan then reshapes its drills and rewards.

use crate::weapons::Weapon;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseClass {
    Fighter,
    Archer,
    Healer,
    Guard,
    Sentry,
    Keeper,
    Mage,
    Assassin,
    Paladin,
    Warlock,
    Hunter,
}

/// Which side a troop is trained for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Role {
    #[default]
    Attack,
    Defend,
}

const APPROACH: f32 = 0.5;
const FOLLOW: f32 = 0.3;

impl BaseClass {
    pub const ALL: [BaseClass; 6] = [
        BaseClass::Fighter,
        BaseClass::Archer,
        BaseClass::Healer,
        BaseClass::Mage,
        BaseClass::Assassin,
        BaseClass::Paladin,
    ];
    pub const DEFENDERS: [BaseClass; 5] = [
        BaseClass::Guard,
        BaseClass::Sentry,
        BaseClass::Keeper,
        BaseClass::Warlock,
        BaseClass::Hunter,
    ];

    pub fn role(self) -> Role {
        match self {
            BaseClass::Fighter
            | BaseClass::Archer
            | BaseClass::Healer
            | BaseClass::Mage
            | BaseClass::Assassin
            | BaseClass::Paladin => Role::Attack,
            _ => Role::Defend,
        }
    }

    /// Word used when this class flavours another in a blend's name.
    pub fn adjective(self) -> &'static str {
        match self {
            BaseClass::Fighter => "Iron",
            BaseClass::Archer => "Swift",
            BaseClass::Healer => "Holy",
            BaseClass::Mage => "Arcane",
            BaseClass::Assassin => "Shadow",
            BaseClass::Paladin => "Valiant",
            BaseClass::Guard => "Stalwart",
            BaseClass::Sentry => "Watchful",
            BaseClass::Keeper => "Sacred",
            BaseClass::Warlock => "Hexed",
            BaseClass::Hunter => "Wild",
        }
    }

    /// Word used when this class leads a blend.
    pub fn noun(self) -> &'static str {
        match self {
            BaseClass::Fighter => "Warrior",
            BaseClass::Archer => "Ranger",
            BaseClass::Healer => "Priest",
            BaseClass::Mage => "Sorcerer",
            BaseClass::Assassin => "Blade",
            BaseClass::Paladin => "Crusader",
            BaseClass::Guard => "Guardian",
            BaseClass::Sentry => "Marksman",
            BaseClass::Keeper => "Keeper",
            BaseClass::Warlock => "Warlock",
            BaseClass::Hunter => "Hunter",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BaseClass::Fighter => "Fighter",
            BaseClass::Archer => "Archer",
            BaseClass::Healer => "Healer",
            BaseClass::Guard => "Guard",
            BaseClass::Sentry => "Sentry",
            BaseClass::Keeper => "Keeper",
            BaseClass::Mage => "Mage",
            BaseClass::Assassin => "Assassin",
            BaseClass::Paladin => "Paladin",
            BaseClass::Warlock => "Warlock",
            BaseClass::Hunter => "Hunter",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            BaseClass::Fighter => "Tough melee bruiser. Learns to clear guards, then crush towers.",
            BaseClass::Archer => {
                "Ranged shooter that out-ranges guards. Learns to strike from a safe distance."
            }
            BaseClass::Healer => {
                "Support. Learns to keep the squad alive while staying out of fire."
            }
            BaseClass::Guard => "Defender. Melee wall that learns to intercept raiders before they reach your buildings.",
            BaseClass::Sentry => "Defender. Ranged shooter that learns to pick off raiders from behind the towers.",
            BaseClass::Keeper => "Defender. Shrine keeper that learns to heal the guards holding the line.",
            BaseClass::Mage => "Spell blasts hit everything near the target. Fragile, strong against crowds of guards.",
            BaseClass::Assassin => "Fast and deadly up close, but paper thin. Learns to pick its moment.",
            BaseClass::Paladin => "Slow armoured tank with a small heal. Soaks tower fire for the squad.",
            BaseClass::Warlock => "Defender. Curses splash onto groups of raiders from behind the walls.",
            BaseClass::Hunter => "Defender. Fast melee chaser that runs raiders down before they reach the core.",
        }
    }

    pub fn spec(self) -> ClassSpec {
        match self {
            BaseClass::Fighter => ClassSpec {
                name: "Fighter".into(),
                parents: vec![(BaseClass::Fighter, 1.0)],
                hp: 100.0,
                speed: 0.35,
                range: 1.5,
                dmg: 10.0,
                cooldown: 4,
                heal: 0.0,
                heal_range: 0.0,
                color: [0.86, 0.27, 0.25],
                weights: RewardWeights {
                    building_dmg: 2.0,
                    unit_dmg: 1.0,
                    heal: 0.0,
                    ally_death: 0.0,
                    approach: APPROACH,
                    ..RewardWeights::base()
                },
                drills: DrillMix::new(0.75, 0.25),
                role: Role::Attack,
                splash: 0.0,
                plan: None,
                weapon: None,
                mastery: 0,
                move_practice: [0.0; crate::moves::N_MOVES],
            },
            BaseClass::Archer => ClassSpec {
                name: "Archer".into(),
                parents: vec![(BaseClass::Archer, 1.0)],
                hp: 70.0,
                speed: 0.32,
                range: 5.5,
                dmg: 7.0,
                cooldown: 5,
                heal: 0.0,
                heal_range: 0.0,
                color: [0.30, 0.72, 0.36],
                weights: RewardWeights {
                    building_dmg: 2.0,
                    unit_dmg: 1.0,
                    heal: 0.0,
                    ally_death: 0.0,
                    approach: APPROACH,
                    ..RewardWeights::base()
                },
                drills: DrillMix::new(0.75, 0.25),
                role: Role::Attack,
                splash: 0.0,
                plan: None,
                weapon: None,
                mastery: 0,
                move_practice: [0.0; crate::moves::N_MOVES],
            },
            BaseClass::Healer => ClassSpec {
                name: "Healer".into(),
                parents: vec![(BaseClass::Healer, 1.0)],
                hp: 70.0,
                speed: 0.33,
                range: 3.0,
                dmg: 3.0,
                cooldown: 5,
                heal: 9.0,
                heal_range: 4.5,
                color: [0.98, 0.84, 0.35],
                weights: RewardWeights {
                    building_dmg: 0.6,
                    unit_dmg: 0.3,
                    heal: 1.5,
                    ally_death: -0.3,
                    follow: FOLLOW,
                    ..RewardWeights::base()
                },
                drills: DrillMix::new(0.0, 1.0),
                role: Role::Attack,
                splash: 0.0,
                plan: None,
                weapon: None,
                mastery: 0,
                move_practice: [0.0; crate::moves::N_MOVES],
            },
            BaseClass::Guard => ClassSpec {
                name: "Guard".into(),
                parents: vec![(BaseClass::Guard, 1.0)],
                hp: 90.0,
                speed: 0.32,
                range: 1.4,
                dmg: 8.0,
                cooldown: 5,
                heal: 0.0,
                heal_range: 0.0,
                color: [0.45, 0.55, 0.95],
                weights: RewardWeights::defend(1.0, 0.0, APPROACH),
                drills: DrillMix::new(0.0, 1.0),
                role: Role::Defend,
                splash: 0.0,
                plan: None,
                weapon: None,
                mastery: 0,
                move_practice: [0.0; crate::moves::N_MOVES],
            },
            BaseClass::Sentry => ClassSpec {
                name: "Sentry".into(),
                parents: vec![(BaseClass::Sentry, 1.0)],
                hp: 55.0,
                speed: 0.30,
                range: 5.0,
                dmg: 6.0,
                cooldown: 6,
                heal: 0.0,
                heal_range: 0.0,
                color: [0.35, 0.80, 0.85],
                weights: RewardWeights::defend(1.0, 0.0, 0.3),
                drills: DrillMix::new(0.0, 1.0),
                role: Role::Defend,
                splash: 0.0,
                plan: None,
                weapon: None,
                mastery: 0,
                move_practice: [0.0; crate::moves::N_MOVES],
            },
            BaseClass::Keeper => ClassSpec {
                name: "Keeper".into(),
                parents: vec![(BaseClass::Keeper, 1.0)],
                hp: 60.0,
                speed: 0.31,
                range: 3.0,
                dmg: 3.0,
                cooldown: 5,
                heal: 8.0,
                heal_range: 4.5,
                color: [0.80, 0.55, 0.95],
                weights: RewardWeights {
                    heal: 1.5,
                    follow: FOLLOW,
                    ..RewardWeights::defend(0.3, 0.0, 0.0)
                },
                drills: DrillMix::new(0.0, 1.0),
                role: Role::Defend,
                splash: 0.0,
                plan: None,
                weapon: None,
                mastery: 0,
                move_practice: [0.0; crate::moves::N_MOVES],
            },
            BaseClass::Mage => ClassSpec {
                name: "Mage".into(),
                parents: vec![(BaseClass::Mage, 1.0)],
                hp: 75.0,
                speed: 0.31,
                range: 5.0,
                dmg: 8.0,
                cooldown: 5,
                splash: 1.5,
                color: [0.35, 0.45, 1.0],
                weights: RewardWeights {
                    building_dmg: 2.0,
                    unit_dmg: 1.0,
                    approach: APPROACH,
                    ..RewardWeights::base()
                },
                drills: DrillMix::new(0.7, 0.3),
                ..ClassSpec::empty(Role::Attack)
            },
            BaseClass::Assassin => ClassSpec {
                name: "Assassin".into(),
                parents: vec![(BaseClass::Assassin, 1.0)],
                hp: 90.0,
                speed: 0.46,
                range: 1.3,
                dmg: 9.0,
                cooldown: 3,
                color: [0.42, 0.30, 0.55],
                weights: RewardWeights {
                    building_dmg: 1.6,
                    unit_dmg: 1.6,
                    approach: APPROACH,
                    ..RewardWeights::base()
                },
                drills: DrillMix::new(0.8, 0.2),
                ..ClassSpec::empty(Role::Attack)
            },
            BaseClass::Paladin => ClassSpec {
                name: "Paladin".into(),
                parents: vec![(BaseClass::Paladin, 1.0)],
                hp: 150.0,
                speed: 0.30,
                range: 1.5,
                dmg: 7.0,
                cooldown: 4,
                heal: 3.0,
                heal_range: 2.5,
                color: [0.95, 0.85, 0.55],
                weights: RewardWeights {
                    building_dmg: 1.5,
                    unit_dmg: 1.0,
                    heal: 1.0,
                    ally_death: -0.2,
                    approach: APPROACH,
                    ..RewardWeights::base()
                },
                drills: DrillMix::new(0.5, 0.5),
                ..ClassSpec::empty(Role::Attack)
            },
            BaseClass::Warlock => ClassSpec {
                name: "Warlock".into(),
                parents: vec![(BaseClass::Warlock, 1.0)],
                hp: 55.0,
                speed: 0.30,
                range: 4.0,
                dmg: 5.0,
                cooldown: 6,
                splash: 1.5,
                color: [0.85, 0.30, 0.55],
                weights: RewardWeights::defend(1.0, 0.0, 0.3),
                drills: DrillMix::new(0.0, 1.0),
                ..ClassSpec::empty(Role::Defend)
            },
            BaseClass::Hunter => ClassSpec {
                name: "Hunter".into(),
                parents: vec![(BaseClass::Hunter, 1.0)],
                hp: 80.0,
                speed: 0.42,
                range: 1.4,
                dmg: 7.0,
                cooldown: 4,
                color: [0.65, 0.50, 0.25],
                weights: RewardWeights::defend(1.2, 0.0, APPROACH),
                drills: DrillMix::new(0.0, 1.0),
                ..ClassSpec::empty(Role::Defend)
            },
        }
    }
}

/// Weights on the named reward signals. Damage and healing are normalised so
/// that a full drill's worth is about 1.0.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RewardWeights {
    pub building_dmg: f32,
    pub unit_dmg: f32,
    pub heal: f32,
    pub win: f32,
    pub death: f32,
    /// Running out the clock is worse than dying. This closes the prototype's
    /// "hide until timeout" exploit.
    pub timeout: f32,
    pub ally_death: f32,
    pub tick: f32,
    /// Shaping: reward for closing in on the nearest enemy (potential-based,
    /// so it speeds up learning without changing what the best play is).
    #[serde(default)]
    pub approach: f32,
    /// Shaping: reward for staying near the nearest ally.
    #[serde(default)]
    pub follow: f32,
}

impl RewardWeights {
    pub fn base() -> Self {
        RewardWeights {
            building_dmg: 1.0,
            unit_dmg: 1.0,
            heal: 0.0,
            win: 1.0,
            death: -1.0,
            timeout: -1.5,
            ally_death: 0.0,
            tick: -0.002,
            approach: 0.0,
            follow: 0.0,
        }
    }

    /// Defender rewards: `building_dmg` is the penalty weight for building HP
    /// lost, `unit_dmg` pays for damage to raiders, `win` for holding the base.
    pub fn defend(unit_dmg: f32, heal: f32, approach: f32) -> Self {
        RewardWeights {
            building_dmg: 2.0,
            unit_dmg,
            heal,
            win: 1.0,
            death: -0.5,
            timeout: 0.0,
            ally_death: -0.2,
            tick: 0.0,
            approach,
            follow: 0.0,
        }
    }

    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        RewardWeights {
            building_dmg: l(a.building_dmg, b.building_dmg),
            unit_dmg: l(a.unit_dmg, b.unit_dmg),
            heal: l(a.heal, b.heal),
            win: l(a.win, b.win),
            death: l(a.death, b.death),
            timeout: l(a.timeout, b.timeout),
            ally_death: l(a.ally_death, b.ally_death),
            tick: l(a.tick, b.tick),
            approach: l(a.approach, b.approach),
            follow: l(a.follow, b.follow),
        }
    }
}

/// How often each training drill is picked.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DrillMix {
    /// One troop against a tower and its guards.
    pub solo: f32,
    /// The troop plus two scripted fighters against a bigger base.
    pub squad: f32,
    /// Chance a drill faces the player's own trained troops instead of
    /// scripted enemies (when there are any).
    #[serde(default = "default_self_play")]
    pub self_play: f32,
}

fn default_self_play() -> f32 {
    crate::env::SELF_PLAY_CHANCE
}

impl DrillMix {
    pub fn new(solo: f32, squad: f32) -> Self {
        DrillMix { solo, squad, self_play: default_self_play() }
    }
}

/// The player's own training choices, all in whole percentages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainPlan {
    /// Attackers: share of solo raids; the rest are squad raids.
    pub solo: u32,
    /// Share of drills against the player's own trained troops.
    pub self_play: u32,
    /// How much each goal counts in the reward, summing to 100:
    /// buildings, enemy troops, healing, staying alive.
    pub focus: [u32; 4],
}

pub const FOCUS_NAMES_ATTACK: [&str; 4] = ["Break buildings", "Fight enemy troops", "Heal allies", "Stay alive"];
pub const FOCUS_NAMES_DEFEND: [&str; 4] = ["Protect buildings", "Fight raiders", "Heal allies", "Stay alive"];

impl TrainPlan {
    /// The plan a class mix would use on its own: its own drill mix and an
    /// even focus over the goals it can pursue.
    pub fn default_for(spec: &ClassSpec) -> TrainPlan {
        let total = (spec.drills.solo + spec.drills.squad).max(1e-6);
        let solo = (spec.drills.solo / total * 100.0).round() as u32;
        let focus = if spec.can_heal() { [25, 25, 25, 25] } else { [34, 33, 0, 33] };
        TrainPlan { solo, self_play: 50, focus }
    }

    /// Goals in play for this class (healing only if it can heal).
    pub fn active(spec: &ClassSpec) -> [bool; 4] {
        [true, true, spec.can_heal(), true]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassSpec {
    pub name: String,
    pub parents: Vec<(BaseClass, f32)>,
    pub hp: f32,
    pub speed: f32,
    pub range: f32,
    pub dmg: f32,
    pub cooldown: u32,
    pub heal: f32,
    pub heal_range: f32,
    pub color: [f32; 3],
    pub weights: RewardWeights,
    pub drills: DrillMix,
    #[serde(default)]
    pub role: Role,
    /// Attacks also hit enemies within this distance of the target (half damage).
    #[serde(default)]
    pub splash: f32,
    /// The player's training plan, if they set one (applied to `weights` and `drills`).
    #[serde(default)]
    pub plan: Option<TrainPlan>,
    /// Equipped weapon. None means the default weapon of the main class.
    #[serde(default)]
    pub weapon: Option<Weapon>,
    /// Mastery level (0 to 5) with the equipped weapon, set from the troop.
    #[serde(default)]
    pub mastery: u8,
    /// Practice (0 to 1) with each class move, set from the troop.
    #[serde(default)]
    pub move_practice: [f32; crate::moves::N_MOVES],
}

impl ClassSpec {
    fn empty(role: Role) -> ClassSpec {
        ClassSpec {
            name: String::new(),
            parents: Vec::new(),
            hp: 0.0,
            speed: 0.0,
            range: 0.0,
            dmg: 0.0,
            cooldown: 1,
            heal: 0.0,
            heal_range: 0.0,
            color: [1.0; 3],
            weights: RewardWeights::base(),
            drills: DrillMix::new(1.0, 0.0),
            role,
            splash: 0.0,
            plan: None,
            weapon: None,
            mastery: 0,
            move_practice: [0.0; crate::moves::N_MOVES],
        }
    }

    /// Blend any number of same-side classes. Shares are normalised, zero shares
    /// dropped. One class gives that class; two use the classic hybrid maths.
    pub fn blend(parts: &[(BaseClass, f32)]) -> ClassSpec {
        let mut parts: Vec<(BaseClass, f32)> = parts.iter().copied().filter(|p| p.1 > 0.0).collect();
        assert!(!parts.is_empty(), "a blend needs at least one class");
        // Biggest share first: it leads the name and the look.
        parts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let total: f32 = parts.iter().map(|p| p.1).sum();
        for p in &mut parts {
            p.1 /= total;
        }
        if parts.len() == 1 {
            return parts[0].0.spec();
        }
        if parts.len() == 2 {
            let mut s = ClassSpec::hybrid(parts[0].0, parts[1].0, parts[1].1);
            s.name = blend_name(&parts);
            return s;
        }
        let specs: Vec<ClassSpec> = parts.iter().map(|p| p.0.spec()).collect();
        let avg = |f: &dyn Fn(&ClassSpec) -> f32| -> f32 {
            specs.iter().zip(&parts).map(|(s, p)| f(s) * p.1).sum()
        };
        let healers: f32 = specs.iter().zip(&parts).filter(|(s, _)| s.heal_range > 0.0).map(|(_, p)| p.1).sum();
        let heal_range = if healers > 0.0 {
            specs.iter().zip(&parts).filter(|(s, _)| s.heal_range > 0.0).map(|(s, p)| s.heal_range * p.1).sum::<f32>() / healers
        } else {
            0.0
        };
        let splashers: f32 = specs.iter().zip(&parts).filter(|(s, _)| s.splash > 0.0).map(|(_, p)| p.1).sum();
        let w = |f: &dyn Fn(&RewardWeights) -> f32| -> f32 {
            specs.iter().zip(&parts).map(|(s, p)| f(&s.weights) * p.1).sum()
        };
        ClassSpec {
            name: blend_name(&parts),
            parents: parts.clone(),
            hp: avg(&|s| s.hp),
            speed: avg(&|s| s.speed),
            range: avg(&|s| s.range),
            dmg: avg(&|s| s.dmg),
            cooldown: avg(&|s| s.cooldown as f32).round().max(1.0) as u32,
            heal: avg(&|s| s.heal),
            heal_range,
            color: [avg(&|s| s.color[0]), avg(&|s| s.color[1]), avg(&|s| s.color[2])],
            weights: RewardWeights {
                building_dmg: w(&|r| r.building_dmg),
                unit_dmg: w(&|r| r.unit_dmg),
                heal: w(&|r| r.heal),
                win: w(&|r| r.win),
                death: w(&|r| r.death),
                timeout: w(&|r| r.timeout),
                ally_death: w(&|r| r.ally_death),
                tick: w(&|r| r.tick),
                approach: w(&|r| r.approach),
                follow: w(&|r| r.follow),
            },
            drills: DrillMix::new(avg(&|s| s.drills.solo), avg(&|s| s.drills.squad)),
            role: specs[0].role,
            // Splash needs a real share of a splash class; its reach scales with that share.
            splash: if splashers >= 0.2 { 1.5 * splashers.sqrt() } else { 0.0 },
            plan: None,
            weapon: None,
            mastery: 0,
            move_practice: [0.0; crate::moves::N_MOVES],
        }
    }

    /// A blend with the player's training plan applied on top.
    pub fn planned(parts: &[(BaseClass, f32)], plan: TrainPlan) -> ClassSpec {
        let mut s = ClassSpec::blend(parts);
        s.apply_plan(plan);
        s
    }

    /// Reshape drills and rewards. Focus is relative to an even split, so an
    /// untouched plan leaves the class's own rewards as they are.
    pub fn apply_plan(&mut self, plan: TrainPlan) {
        if self.role == Role::Attack {
            let solo = plan.solo.min(100) as f32 / 100.0;
            self.drills.solo = solo;
            self.drills.squad = 1.0 - solo;
        }
        self.drills.self_play = plan.self_play.min(100) as f32 / 100.0;
        let active = TrainPlan::active(self);
        let n = active.iter().filter(|a| **a).count() as f32;
        let even = 100.0 / n;
        let m = |i: usize| if active[i] { plan.focus[i] as f32 / even } else { 1.0 };
        self.weights.building_dmg *= m(0);
        self.weights.unit_dmg *= m(1);
        self.weights.heal *= m(2);
        self.weights.death *= m(3);
        self.weights.ally_death *= m(3);
        self.plan = Some(plan);
    }

    /// Blend two classes: `t` is how much of `b` goes in (0.5 = half and half).
    pub fn hybrid(a: BaseClass, b: BaseClass, t: f32) -> ClassSpec {
        let (sa, sb) = (a.spec(), b.spec());
        let l = |x: f32, y: f32| x + (y - x) * t;
        let name = match hybrid_name(a, b) {
            Some(n) if (t - 0.5).abs() < 0.01 => n.to_string(),
            _ => format!(
                "{}-{} {:.0}/{:.0}",
                a.name(),
                b.name(),
                (1.0 - t) * 100.0,
                t * 100.0
            ),
        };
        let mut color = [0.0; 3];
        for i in 0..3 {
            color[i] = l(sa.color[i], sb.color[i]);
        }
        ClassSpec {
            name,
            parents: vec![(a, 1.0 - t), (b, t)],
            hp: l(sa.hp, sb.hp),
            speed: l(sa.speed, sb.speed),
            range: l(sa.range, sb.range),
            dmg: l(sa.dmg, sb.dmg),
            cooldown: l(sa.cooldown as f32, sb.cooldown as f32).round() as u32,
            heal: l(sa.heal, sb.heal),
            heal_range: if sa.heal_range > 0.0 && sb.heal_range > 0.0 {
                l(sa.heal_range, sb.heal_range)
            } else {
                sa.heal_range.max(sb.heal_range)
            },
            color,
            weights: RewardWeights::lerp(&sa.weights, &sb.weights, t),
            drills: DrillMix::new(l(sa.drills.solo, sb.drills.solo), l(sa.drills.squad, sb.drills.squad)),
            role: sa.role,
            splash: if sa.splash > 0.0 && sb.splash > 0.0 {
                l(sa.splash, sb.splash)
            } else if (sa.splash > 0.0 && t <= 0.8) || (sb.splash > 0.0 && t >= 0.2) {
                1.5 * (if sa.splash > 0.0 { 1.0 - t } else { t }).sqrt()
            } else {
                0.0
            },
            plan: None,
            weapon: None,
            mastery: 0,
            move_practice: [0.0; crate::moves::N_MOVES],
        }
    }

    /// The class with the biggest share (the first one on a tie).
    pub fn main_class(&self) -> BaseClass {
        let mut best = self.parents[0];
        for p in &self.parents[1..] {
            if p.1 > best.1 {
                best = *p;
            }
        }
        best.0
    }

    /// The weapon this troop fights with.
    pub fn weapon(&self) -> Weapon {
        self.weapon.unwrap_or_else(|| Weapon::default_for(self.main_class()))
    }

    /// Copy the equipped weapon and mastery from another spec (after a re-blend).
    pub fn with_gear_of(mut self, other: &ClassSpec) -> ClassSpec {
        self.weapon = other.weapon;
        self.mastery = other.mastery;
        self.move_practice = other.move_practice;
        self
    }

    pub fn is_hybrid(&self) -> bool {
        self.parents.len() > 1
    }

    pub fn can_heal(&self) -> bool {
        self.heal > 0.0
    }

    pub fn describe(&self) -> String {
        let mut s = format!(
            "HP {:.0}  DMG {:.1}/{}t  range {:.1}  speed {:.2}",
            self.hp, self.dmg, self.cooldown, self.range, self.speed
        );
        if self.can_heal() {
            s += &format!("  heal {:.1} @ {:.1}", self.heal, self.heal_range);
        }
        if self.splash > 0.0 {
            s += &format!("  splash {:.1}", self.splash);
        }
        s
    }

    /// "Mage 50% · Healer 30% · Fighter 20%" for blends, empty for pure classes.
    pub fn mix_label(&self) -> String {
        if self.parents.len() < 2 {
            return String::new();
        }
        self.parents
            .iter()
            .map(|(c, w)| format!("{} {:.0}%", c.name(), w * 100.0))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Named 50/50 pairs keep their names; any other blend is "<flavour> <leader>",
/// e.g. Mage 60 + Healer 40 is a "Holy Sorcerer". Parts must be sorted, biggest first.
fn blend_name(parts: &[(BaseClass, f32)]) -> String {
    if parts.len() == 2 && (parts[0].1 - 0.5).abs() < 0.01 {
        if let Some(n) = hybrid_name(parts[0].0, parts[1].0) {
            return n.to_string();
        }
    }
    let base = format!("{} {}", parts[1].0.adjective(), parts[0].0.noun());
    if parts.len() >= 3 {
        format!("{} of {}", base, parts[2].0.adjective().to_string() + " Arts")
    } else {
        base
    }
}

pub fn hybrid_name(a: BaseClass, b: BaseClass) -> Option<&'static str> {
    use BaseClass::*;
    match (a, b) {
        (Fighter, Healer) | (Healer, Fighter) => Some("Battle Priest"),
        (Archer, Healer) | (Healer, Archer) => Some("Spirit Ranger"),
        (Fighter, Archer) | (Archer, Fighter) => Some("Spellblade"),
        (Guard, Sentry) | (Sentry, Guard) => Some("Bastion"),
        (Guard, Keeper) | (Keeper, Guard) => Some("Temple Knight"),
        (Sentry, Keeper) | (Keeper, Sentry) => Some("Oracle"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_weight_every_part() {
        let s = ClassSpec::blend(&[(BaseClass::Mage, 50.0), (BaseClass::Healer, 30.0), (BaseClass::Fighter, 20.0)]);
        let hp = 0.5 * BaseClass::Mage.spec().hp + 0.3 * BaseClass::Healer.spec().hp + 0.2 * BaseClass::Fighter.spec().hp;
        assert!((s.hp - hp).abs() < 1e-3);
        assert_eq!(s.parents[0].0, BaseClass::Mage);
        assert!(s.splash > 0.0 && s.can_heal());
        assert_eq!(ClassSpec::blend(&[(BaseClass::Archer, 100.0)]).name, "Archer");
    }

    #[test]
    fn an_even_plan_keeps_the_class_rewards() {
        let base = BaseClass::Healer.spec();
        let planned = ClassSpec::planned(&base.parents, TrainPlan::default_for(&base));
        assert!((planned.weights.heal - base.weights.heal).abs() < 1e-6);
        assert!((planned.weights.building_dmg - base.weights.building_dmg).abs() < 1e-6);
        let mut focus = TrainPlan::default_for(&base);
        focus.focus = [70, 10, 10, 10];
        let p = ClassSpec::planned(&base.parents, focus);
        assert!(p.weights.building_dmg > 2.0 * base.weights.building_dmg);
    }
}
