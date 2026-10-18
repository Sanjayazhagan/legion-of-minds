//! A trained troop as the player owns it: class, level, brain and history.
//! Saved as one small JSON file, which is also what gets shared for wars.

use crate::classes::ClassSpec;
use crate::env::EpisodeInfo;
use crate::nn::Mlp;
use crate::ppo::{actor_dims, critic_dims, new_actor_for, new_critic_for};
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
}

/// Starting score for actions a grown brain has never tried (dodge, block,
/// skill): low enough to keep its old habits, high enough to get explored.
const NEW_ACTION_BIAS: f32 = -2.0;

impl Troop {
    pub fn new(id: u64, name: String, spec: ClassSpec, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
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
        }
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

    /// Copy move practice into the spec the sim reads.
    pub fn sync_moves(&mut self) {
        for m in Move::ALL {
            self.spec.move_practice[m as usize] = if self.spec.has_move(m) { self.move_practice(m) } else { 0.0 };
        }
    }

    /// Bring the brain up to the size and shape this level calls for,
    /// keeping everything it learned. Returns true if it changed.
    pub fn upgrade_brain(&mut self) -> bool {
        let mut rng = Rng::new(self.seed ^ 0x6272_6169_6e00 ^ self.level as u64);
        let mut changed = false;
        let target = actor_dims(self.level);
        if self.actor.dims != target {
            // Never shrink a brain that is already bigger.
            let dims: Vec<usize> = target.iter().zip(&self.actor.dims).map(|(t, o)| *t.max(o)).collect();
            if let Some(a) = self.actor.grow(&dims, NEW_ACTION_BIAS, &mut rng) {
                self.actor = a;
                changed = true;
            }
        }
        let target = critic_dims(self.level);
        if self.critic.dims != target && self.critic.dims.len() == target.len() {
            let dims: Vec<usize> = target.iter().zip(&self.critic.dims).map(|(t, o)| *t.max(o)).collect();
            if let Some(c) = self.critic.grow(&dims, 0.0, &mut rng) {
                self.critic = c;
                changed = true;
            }
        }
        changed
    }

    /// Level up (the caller pays) and grow the brain to match.
    pub fn level_up(&mut self) {
        self.level += 1;
        self.upgrade_brain();
    }

    pub fn cap(&self) -> u64 {
        level_cap(self.level)
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
