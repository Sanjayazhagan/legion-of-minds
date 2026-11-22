//! A trained troop as the player owns it: class, level, brain and history.
//! Saved as one small JSON file, which is also what gets shared for wars.

use crate::body::{conditioning_of, Traits};
use crate::classes::{talents_for_level, ClassSpec};
use crate::env::EpisodeInfo;
use crate::nn::Mlp;
use crate::ppo::{
    actor_dims_h, brain_cap, critic_dims_h, hidden_for_level, new_actor_for, new_critic_for, HIDDEN_SIZES, LEVEL_BRAIN_MAX,
};
use crate::rng::Rng;
use crate::moves::{practice_of, Move};
use crate::weapons::{mastery_level, Weapon};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_LEVEL: u32 = 5;

/// Total training steps a troop can have at each level. Longer training makes
/// a better troop, but the cap grows slower than its cost.
pub fn level_cap(level: u32) -> u64 {
    match level {
        0 | 1 => 300_000,
        2 => 600_000,
        3 => 1_200_000,
        4 => 2_000_000,
        _ => 3_500_000,
    }
}

pub fn level_up_cost(level: u32) -> u32 {
    100 * level * level
}

/// Training steps in each half of the plateau check at a level: the score of
/// the last `window` steps is compared with the `window` before it.
pub fn plateau_window(level: u32) -> u64 {
    plateau_window_for(level_cap(level))
}

/// The plateau window for a training cap (see [`Troop::plateau_window`]).
pub fn plateau_window_for(cap: u64) -> u64 {
    (cap / 8).max(60_000)
}

/// A troop has plateaued when its drill score rose by less than this (on a
/// 0 to 1 scale, so 0.03 = 3 points) between the two halves of the check.
pub const PLATEAU_GAIN: f32 = 0.03;

/// A flat score only counts as a plateau once the troop has learned
/// something at this level: its score rose at least this much since the
/// level began (or it is already near the top). Before that, RL troops often
/// sit flat for a while before it clicks, and that isn't a full brain.
pub const PLATEAU_RISE: f32 = 0.10;

/// Has the troop learned all its current brain can hold? Levelling up (which
/// grows the brain) is only allowed once it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plateau {
    /// A troop from before the check existed: its next level-up skips it.
    Grandfathered,
    /// Not enough training at this level to judge yet.
    Warming { done: u64, need: u64 },
    /// Flat, but it hasn't learned anything yet at this level: often RL just
    /// hasn't clicked. `rise` is the score rise since the level began.
    Starting { rise: f32 },
    /// Still getting better. `gain` is the score rise between the halves.
    Improving { gain: f32 },
    /// Its score has gone flat: the brain is full.
    Reached { gain: f32 },
    /// It used every training step this level allows.
    AtCap,
}

impl Plateau {
    /// May the troop level up (as far as training goes)?
    pub fn ready(&self) -> bool {
        matches!(self, Plateau::Grandfathered | Plateau::Reached { .. } | Plateau::AtCap)
    }
}

/// One number for how well a troop does in drills: mostly wins, some survival.
pub fn drill_score(p: &CurvePoint) -> f32 {
    0.75 * p.win + 0.25 * (1.0 - p.died)
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct CurvePoint {
    pub steps: u64,
    pub win: f32,
    pub died: f32,
    pub timeout: f32,
    pub ret: f32,
}

/// Rolling averages of what the troop does in drills. Shown as sub-skills,
/// because learning comes in jumps and a flat win rate hides progress.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Skills {
    pub guard_share: f32,
    pub building_share: f32,
    pub healed: f32,
    pub survival: f32,
    pub win: f32,
    pub episodes: u32,
}

impl Skills {
    pub fn from_episodes(eps: &[EpisodeInfo]) -> Skills {
        if eps.is_empty() {
            return Skills::default();
        }
        let n = eps.len() as f32;
        Skills {
            guard_share: eps.iter().map(|e| e.guard_share).sum::<f32>() / n,
            building_share: eps.iter().map(|e| e.building_share).sum::<f32>() / n,
            healed: eps.iter().map(|e| e.healed).sum::<f32>() / n,
            survival: eps.iter().filter(|e| !e.died).count() as f32 / n,
            win: eps.iter().filter(|e| e.won).count() as f32 / n,
            episodes: eps.len() as u32,
        }
    }

    /// (label, learned?) pairs for the badge row.
    pub fn badges(&self, spec: &ClassSpec) -> Vec<(&'static str, bool)> {
        let mut v = Vec::new();
        if spec.role == crate::classes::Role::Defend {
            v.push(("Fights raiders", self.guard_share >= 0.4));
            v.push((
                "Protects buildings",
                self.episodes > 0 && self.building_share <= 0.5,
            ));
            if spec.can_heal() {
                v.push(("Heals allies", self.healed >= 30.0));
            }
            v.push(("Stays alive", self.survival >= 0.5));
            v.push(("Holds the base", self.win >= 0.5));
            return v;
        }
        if spec.dmg >= 5.0 {
            v.push(("Fights guards", self.guard_share >= 0.3));
            v.push(("Breaks buildings", self.building_share >= 0.3));
        }
        if spec.can_heal() {
            v.push(("Heals allies", self.healed >= 60.0));
        }
        v.push(("Stays alive", self.survival >= 0.7));
        v.push(("Wins drills", self.win >= 0.5));
        v
    }
}

/// Tiredness a troop carries between raids (stage C).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rest {
    /// Share of the stamina pool missing when the last raid ended.
    pub carried: f32,
    /// When that raid ended, in unix seconds.
    pub since: u64,
}

/// Unix seconds now (0 if the clock is before 1970).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Troop {
    pub id: u64,
    pub name: String,
    pub spec: ClassSpec,
    pub level: u32,
    pub steps: u64,
    pub actor: Mlp,
    pub critic: Mlp,
    pub curve: Vec<CurvePoint>,
    pub skills: Skills,
    pub seed: u64,
    #[serde(default)]
    pub sessions: u32,
    /// Training steps spent holding each weapon.
    #[serde(default)]
    pub mastery: BTreeMap<Weapon, u64>,
    /// Training steps of practice with each class move.
    #[serde(default)]
    pub move_xp: BTreeMap<Move, u64>,
    /// Neurons per layer the player picked. None for troops from before the
    /// choice existed: their brain grows with level as it used to.
    #[serde(default)]
    pub brain: Option<usize>,
    /// Training steps spent in team training (also counted in `steps`).
    #[serde(default)]
    pub team_steps: u64,
    /// Total steps when the troop reached its current level; the plateau
    /// check only looks at training since then. None for troops saved before
    /// the check existed: their next level-up skips it.
    #[serde(default)]
    pub level_from: Option<u64>,
    /// The plateau check passed at some point during this level. It stays
    /// passed until the next level-up, so a lucky stretch of drills afterwards
    /// can't take "ready to level up" away again.
    #[serde(default)]
    pub plateaued: bool,
    /// Born with traits rolled from its seed (rules v10). Troops recruited
    /// before traits existed are average in everything.
    #[serde(default)]
    pub born: bool,
    /// Tiredness carried home from the last raid and when it ended (unix
    /// seconds). Only this player's own record; online, the server keeps
    /// its own.
    #[serde(default)]
    pub rest: Rest,
    /// What its team came to mean by each signal (stage D), from the last
    /// team session it trained in; empty for none, "" where unknown.
    #[serde(default)]
    pub signals: Vec<String>,
}

/// Starting score for actions a grown brain has never tried (dodge, block,
/// skill): low enough to keep its old habits, high enough to get explored.
const NEW_ACTION_BIAS: f32 = -2.0;

impl Troop {
    pub fn new(id: u64, name: String, spec: ClassSpec, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let mut spec = spec;
        spec.talents = talents_for_level(1);
        Troop {
            id,
            name,
            spec,
            level: 1,
            steps: 0,
            actor: new_actor_for(1, &mut rng),
            critic: new_critic_for(1, &mut rng),
            curve: Vec::new(),
            skills: Skills::default(),
            seed,
            sessions: 0,
            mastery: BTreeMap::new(),
            move_xp: BTreeMap::new(),
            brain: None,
            team_steps: 0,
            level_from: Some(0),
            plateaued: false,
            born: true,
            rest: Rest::default(),
            signals: Vec::new(),
        }
        .synced()
    }

    fn synced(mut self) -> Self {
        self.sync_moves();
        self
    }

    /// How tired (share of the pool) this troop would start a raid at
    /// `now` (unix seconds).
    pub fn fatigue_at(&self, now: u64) -> f32 {
        crate::body::rested(self.rest.carried, now.saturating_sub(self.rest.since))
    }

    /// Seconds until this troop is fully rested.
    pub fn rest_left(&self, now: u64) -> u64 {
        crate::body::rest_left(self.rest.carried, now.saturating_sub(self.rest.since))
    }

    /// What this troop's team means by signal `k`, if it has learned one.
    pub fn signal_meaning(&self, k: u8) -> Option<&str> {
        self.signals.get(k as usize).map(String::as_str).filter(|m| !m.is_empty())
    }

    /// Record how tired a raid that ended at `now` left it.
    pub fn after_raid(&mut self, carried: f32, now: u64) {
        self.rest = Rest { carried, since: now };
    }

    /// What this troop was born with (average for troops from before traits).
    pub fn traits(&self) -> Traits {
        if self.born {
            Traits::roll_for(self.seed, &self.spec)
        } else {
            Traits::default()
        }
    }

    /// Neurons per layer the brain has now.
    pub fn hidden(&self) -> usize {
        self.actor.dims[1]
    }

    /// Neurons per layer the brain should have: the size for its level, or
    /// a bigger size picked back when players chose brain sizes.
    pub fn target_hidden(&self) -> usize {
        self.brain.unwrap_or(0).max(hidden_for_level(self.level))
    }

    /// Has it learned all its current brain can hold? See [`Plateau`].
    pub fn plateau(&self) -> Plateau {
        // At level 5 the brain can still grow past 256, so the check keeps
        // running until there is no bigger size.
        if (self.level >= MAX_LEVEL && self.next_brain().is_none()) || self.steps_left() == 0 {
            return Plateau::AtCap;
        }
        let Some(from) = self.level_from else { return Plateau::Grandfathered };
        let w = self.plateau_window();
        // Two windows to compare, plus a quarter window so the check can be
        // made twice in a row (see below).
        let need = 2 * w + w / 4;
        let done = self.steps.saturating_sub(from);
        if done < need || self.curve.is_empty() {
            return Plateau::Warming { done: done.min(need), need };
        }
        // The curve is thinned and team sessions add one point each, so
        // sample the score evenly through each half instead of counting points.
        let score_at = |s: u64| {
            let p = self.curve.iter().rev().find(|p| p.steps <= s).unwrap_or(&self.curve[0]);
            drill_score(p)
        };
        let mean = |a: u64| (1..=8).map(|k| score_at(a + w * k / 8)).sum::<f32>() / 8.0;
        let gain_at = |end: u64| mean(end - w) - mean(end - 2 * w);
        let now = mean(self.steps - w);
        // Flat now and a quarter window ago, so one lucky stretch of drills
        // doesn't flip the troop between "ready" and "still improving".
        let gain = gain_at(self.steps);
        let gain_then = gain_at(self.steps - w / 4);
        let rise = now.max(mean(self.steps - 2 * w)) - mean(from);
        if self.plateaued {
            Plateau::Reached { gain }
        } else if gain.max(gain_then) >= PLATEAU_GAIN {
            Plateau::Improving { gain: gain.max(gain_then) }
        } else if rise < PLATEAU_RISE && now < 0.85 {
            Plateau::Starting { rise }
        } else {
            Plateau::Reached { gain }
        }
    }

    /// Call after training adds to the curve: remembers a passed plateau
    /// check for the rest of the level.
    pub fn note_plateau(&mut self) {
        if matches!(self.plateau(), Plateau::Reached { .. }) {
            self.plateaued = true;
        }
    }

    /// The player picks a brain size. Brains can grow (keeping what they
    /// learned) but never shrink. Returns false if the size isn't allowed.
    pub fn set_brain(&mut self, h: usize) -> bool {
        if !HIDDEN_SIZES.contains(&h) || h < self.hidden() {
            return false;
        }
        self.brain = Some(h);
        self.upgrade_brain();
        true
    }

    /// The next bigger brain size, if there is one.
    pub fn next_brain(&self) -> Option<usize> {
        HIDDEN_SIZES.iter().copied().find(|&h| h > self.hidden())
    }

    /// Steps in each half of this troop's plateau check right now.
    pub fn plateau_window(&self) -> u64 {
        plateau_window_for(self.cap())
    }

    /// Can a level 5 troop grow its brain now? The caller also checks the
    /// player's PC can train the next size ([`Troop::next_brain`]) and pays
    /// [`crate::ppo::brain_grow_cost`].
    pub fn can_grow_brain(&self) -> bool {
        self.level >= MAX_LEVEL && self.next_brain().is_some() && self.plateau().ready()
    }

    /// Grow a level 5 troop's brain one size, keeping what it learned. Its
    /// training cap rises with it (see [`brain_cap`]) and the plateau check
    /// starts again. Returns the new size.
    pub fn grow_brain(&mut self) -> Option<usize> {
        if self.level < MAX_LEVEL {
            return None;
        }
        let h = self.next_brain()?;
        self.brain = Some(h);
        self.upgrade_brain();
        self.level_from = Some(self.steps);
        self.plateaued = false;
        Some(h)
    }

    pub fn weapon(&self) -> Weapon {
        self.spec.weapon()
    }

    /// Training steps with a weapon.
    pub fn mastery_steps(&self, w: Weapon) -> u64 {
        self.mastery.get(&w).copied().unwrap_or(0)
    }

    pub fn mastery_of(&self, w: Weapon) -> u8 {
        mastery_level(self.mastery_steps(w))
    }

    /// Switch weapons. Mastery of each weapon is kept.
    pub fn equip(&mut self, w: Weapon) {
        self.spec.weapon = Some(w);
        self.spec.mastery = self.mastery_of(w);
    }

    /// Record training: total steps, steps with the equipped weapon, and
    /// practice shared evenly across the troop's class moves.
    /// Returns the new weapon mastery level if it went up.
    pub fn add_training(&mut self, steps: u64) -> Option<u8> {
        let w = self.weapon();
        let before = self.mastery_of(w);
        self.steps += steps;
        *self.mastery.entry(w).or_insert(0) += steps;
        self.spec.mastery = self.mastery_of(w);
        self.practise_moves(steps);
        (self.spec.mastery > before).then_some(self.spec.mastery)
    }

    fn practise_moves(&mut self, steps: u64) {
        let kit = self.spec.moves();
        if !kit.is_empty() {
            for m in &kit {
                *self.move_xp.entry(*m).or_insert(0) += steps / kit.len() as u64;
            }
        }
        self.sync_moves();
    }

    /// Practice (0 to 1) with a move.
    pub fn move_practice(&self, m: Move) -> f32 {
        practice_of(self.move_xp.get(&m).copied().unwrap_or(0))
    }

    /// Copy move practice, the level's talent reach and the body's
    /// conditioning into the spec the sim reads.
    pub fn sync_moves(&mut self) {
        self.spec.talents = talents_for_level(self.level);
        self.spec.conditioning = conditioning_of(self.steps);
        self.spec.traits = self.traits();
        self.spec.move_tier = crate::human_moves::tier_for_brain(self.hidden());
        for m in Move::ALL {
            self.spec.move_practice[m as usize] = if self.spec.has_move(m) { self.move_practice(m) } else { 0.0 };
        }
    }

    /// Bring the brain up to the chosen size and the current input shape,
    /// keeping everything it learned. Returns true if it changed.
    pub fn upgrade_brain(&mut self) -> bool {
        let h = self.target_hidden();
        // Sizes past level 5 add their size to the seed so each growth gets
        // fresh neurons; the old sizes keep the seed they always had.
        let extra = if h > LEVEL_BRAIN_MAX { (h as u64) << 16 } else { 0 };
        let mut rng = Rng::new(self.seed ^ 0x6272_6169_6e00 ^ self.level as u64 ^ extra);
        let mut changed = false;
        let target = actor_dims_h(h);
        if self.actor.dims != target {
            // Never shrink a brain that is already bigger.
            let dims: Vec<usize> = target.iter().zip(&self.actor.dims).map(|(t, o)| *t.max(o)).collect();
            if let Some(a) = self.actor.grow(&dims, NEW_ACTION_BIAS, &mut rng) {
                self.actor = a;
                changed = true;
            }
        }
        let target = critic_dims_h(h);
        if self.critic.dims != target && self.critic.dims.len() == target.len() {
            let dims: Vec<usize> = target.iter().zip(&self.critic.dims).map(|(t, o)| *t.max(o)).collect();
            if let Some(c) = self.critic.grow(&dims, 0.0, &mut rng) {
                self.critic = c;
                changed = true;
            }
        }
        // An old pick the level has caught up with is just the level's size
        // now; keeping it would make the server think the brain doesn't match.
        self.brain = self.brain.filter(|&b| b > hidden_for_level(self.level));
        // The human moves the brain unlocks follow its size.
        self.spec.move_tier = crate::human_moves::tier_for_brain(self.hidden());
        changed
    }

    /// Level up (the caller pays and checks [`Troop::plateau`]). The brain
    /// grows to the size for the new level, keeping what it learned, and the
    /// plateau check starts again.
    pub fn level_up(&mut self) {
        self.level += 1;
        self.level_from = Some(self.steps);
        self.plateaued = false;
        self.sync_moves();
        self.upgrade_brain();
    }

    /// Total training steps allowed: by level, and at level 5 by brain size.
    pub fn cap(&self) -> u64 {
        if self.level >= MAX_LEVEL {
            level_cap(self.level).max(brain_cap(self.hidden()))
        } else {
            level_cap(self.level)
        }
    }

    pub fn steps_left(&self) -> u64 {
        self.cap().saturating_sub(self.steps)
    }

    pub fn save_json(&self) -> String {
        serde_json::to_string(self).expect("troop serialises")
    }

    /// Loads a troop, upgrading files from older versions: brains get the new
    /// inputs and moves, and past training counts as mastery of the troop's
    /// own weapon.
    pub fn load_json(s: &str) -> Result<Troop, String> {
        let mut t: Troop = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if t.mastery.is_empty() && t.steps > 0 {
            t.mastery.insert(t.weapon(), t.steps);
        }
        if t.move_xp.is_empty() && t.steps > 0 {
            let steps = t.steps;
            t.practise_moves(steps);
        }
        t.sync_moves();
        t.spec.mastery = t.mastery_of(t.weapon());
        t.upgrade_brain();
        Ok(t)
    }

    pub fn brain_params(&self) -> usize {
        self.actor.n_params()
    }
}

const NAMES: [&str; 40] = [
    "Akira", "Rin", "Haru", "Yuki", "Sora", "Kaito", "Mika", "Ren", "Hikari", "Daichi", "Aoi",
    "Kenji", "Natsu", "Yuna", "Riku", "Sakura", "Takeshi", "Emi", "Shin", "Hana", "Kaede", "Ryo",
    "Mei", "Jun", "Asuka", "Kira", "Nao", "Tora", "Suzu", "Hayate", "Ayame", "Kou", "Misaki",
    "Raiden", "Chiyo", "Itsuki", "Nami", "Sei", "Tsubasa", "Yoru",
];

pub fn random_name(rng: &mut Rng) -> String {
    NAMES[rng.below(NAMES.len())].to_string()
}
