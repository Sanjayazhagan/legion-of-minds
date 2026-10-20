//! Training drills wrapped as RL environments.
//!
//! - `DrillEnv`: one attacking troop raids a base.
//! - `DefenseEnv`: three defenders, all driven by the policy being trained,
//!   hold a base against a raid party.
//!
//! Either side can face the player's own trained troops instead of scripted
//! ones (self-play): pass them in as an `Opponents` pool.

use crate::classes::{ClassSpec, Role};
use crate::nn::{Cache, Mlp};
use crate::ppo::greedy_action;
use crate::rng::Rng;
use crate::scenario::{add_raiders, build_defense_drill, build_drill, Drill, DEFENSE_SLOTS};
use crate::sim::{Control, Kind, Outcome, Team, World, N_ACTIONS, OBS_DIM, TOWER_HP};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What happened in one finished drill.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub won: bool,
    pub died: bool,
    pub timeout: bool,
    pub squad: bool,
    pub ret: f32,
    pub len: u32,
    /// Share of the enemy units' HP this troop (or defender team) removed.
    pub guard_share: f32,
    /// Attack: share of the building HP this troop removed.
    /// Defence: share of the base's building HP that was lost.
    pub building_share: f32,
    pub healed: f32,
    pub destruction: f32,
    #[serde(default)]
    pub vs_trained: bool,
    /// Weapon skills used, dodges made, hits evaded and damage blocked
    /// (per troop for defence teams).
    #[serde(default)]
    pub skills: f32,
    #[serde(default)]
    pub dodges: f32,
    #[serde(default)]
    pub evaded: f32,
    #[serde(default)]
    pub blocked: f32,
}

/// Trained troops the drill may use as opponents.
pub type Opponents = Arc<Vec<(ClassSpec, Mlp)>>;

/// Chance that a drill uses the opponent pool instead of scripted enemies.
pub const SELF_PLAY_CHANCE: f32 = 0.5;

/// Units in the world that are driven by an opponent's policy.
#[derive(Default)]
struct Npcs {
    units: Vec<(usize, usize)>,
    cache: Cache,
}

impl Npcs {
    fn act(&mut self, w: &World, pool: &Opponents, actions: &mut [u8]) {
        for &(i, p) in &self.units {
            if w.units[i].alive {
                actions[i] = greedy_action(&pool[p].1, &w.observe(i), &w.mask(i), &mut self.cache);
            }
        }
    }
}

/// Turn unit `i` into a trained troop of `spec`, keeping its position and team.
fn become_troop(w: &mut World, i: usize, spec: &ClassSpec) {
    let u = &mut w.units[i];
    u.kind = Kind::Troop;
    u.control = Control::External;
    u.max_hp = spec.hp;
    u.hp = spec.hp;
    u.speed = spec.speed;
    u.range = spec.range;
    u.dmg = spec.dmg;
    u.cd_max = spec.cooldown;
    u.heal = spec.heal;
    u.heal_range = spec.heal_range;
}

/// One step's result for every agent slot.
pub struct StepOut {
    pub rewards: Vec<f32>,
    /// Slot ended this step (died, or the drill ended).
    pub done: Vec<bool>,
    pub episode: Option<EpisodeInfo>,
}

/// A drill with one or more agent slots, all driven by the same policy.
pub trait RlEnv: Send {
    fn slots(&self) -> usize;
    fn active(&self, k: usize) -> bool;
    fn obs(&self, k: usize) -> [f32; OBS_DIM];
    fn mask(&self, k: usize) -> [bool; N_ACTIONS];
    fn act(&mut self, actions: &[u8]) -> StepOut;
}

pub fn make_env(spec: &ClassSpec, seed: u64, pool: Option<Opponents>) -> Box<dyn RlEnv> {
    match spec.role {
        Role::Attack => Box::new(DrillEnv::new(spec.clone(), seed).with_pool(pool)),
        Role::Defend => Box::new(DefenseEnv::new(spec.clone(), seed, pool)),
    }
}

// ------------------------------------------------------------------ attack

pub struct DrillEnv {
    pub spec: ClassSpec,
    pub world: World,
    pub drill: Drill,
    pub rng: Rng,
    pool: Option<Opponents>,
    npcs: Npcs,
    prev: crate::sim::UnitStats,
    prev_allies: usize,
    ret: f32,
    enemy_hp: f32,
    building_hp: f32,
}

impl DrillEnv {
    pub fn new(spec: ClassSpec, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let drill = Drill::pick(&spec, &mut rng);
        let world = build_drill(&spec, drill, &mut rng);
        let mut e = DrillEnv {
            spec,
            world,
            drill,
            rng,
            pool: None,
            npcs: Npcs::default(),
            prev: Default::default(),
            prev_allies: 0,
            ret: 0.0,
            enemy_hp: 0.0,
            building_hp: 0.0,
        };
        e.start();
        e
    }

    pub fn with_pool(mut self, pool: Option<Opponents>) -> Self {
        self.pool = pool.filter(|p| !p.is_empty());
        self.reset();
        self
    }

    /// A fixed drill for evaluation and replays (always scripted guards).
    pub fn fixed(spec: ClassSpec, drill: Drill, seed: u64) -> Self {
        let mut e = Self::new(spec, seed);
        let mut rng = Rng::new(seed);
        e.drill = drill;
        e.world = build_drill(&e.spec, drill, &mut rng);
        e.start();
        e
    }

    fn start(&mut self) {
        self.prev = Default::default();
        self.ret = 0.0;
        self.prev_allies = self.allies_alive();
        let team = self.world.units[0].team;
        self.enemy_hp = self
            .world
            .units
            .iter()
            .filter(|u| u.team != team && !u.kind.is_building())
            .map(|u| u.max_hp)
            .sum();
        self.building_hp = self
            .world
            .units
            .iter()
            .filter(|u| u.kind.is_building())
            .map(|u| u.max_hp)
            .sum();
    }

    pub fn reset(&mut self) {
        self.drill = Drill::pick(&self.spec, &mut self.rng);
        self.world = build_drill(&self.spec, self.drill, &mut self.rng);
        self.npcs.units.clear();
        if let Some(pool) = &self.pool {
            if self.rng.chance(self.spec.drills.self_play) {
                // The player's own defenders take the guards' places.
                for i in 0..self.world.units.len() {
                    if self.world.units[i].kind == Kind::Guard {
                        let p = self.rng.below(pool.len());
                        become_troop(&mut self.world, i, &pool[p].0);
                        self.npcs.units.push((i, p));
                    }
                }
            }
        }
        self.start();
    }

    fn allies_alive(&self) -> usize {
        self.world
            .units
            .iter()
            .skip(1)
            .filter(|u| u.alive && u.team == Team::Attack && u.kind == Kind::Troop)
            .count()
    }

    /// Shaping potential: closer to the nearest enemy and to an ally is better.
    fn potential(&self) -> f32 {
        let w = self.spec.weights;
        potential(&self.world, 0, w.approach, w.follow)
    }

    pub fn obs(&self) -> [f32; OBS_DIM] {
        self.world.observe(0)
    }

    pub fn mask(&self) -> [bool; N_ACTIONS] {
        self.world.mask(0)
    }

    /// Step the drill. Returns the reward and, if the drill ended, its summary
    /// (the env has already been reset in that case).
    pub fn step(&mut self, action: u8) -> (f32, Option<EpisodeInfo>) {
        let phi0 = self.potential();
        let mut actions = vec![0u8; self.world.units.len()];
        actions[0] = action;
        if let Some(pool) = &self.pool {
            self.npcs.act(&self.world, pool, &mut actions);
        }
        self.world.step(&actions);
        let w = self.spec.weights;
        let me = &self.world.units[0];
        let s = me.stats;
        let d_b = s.building_dmg - self.prev.building_dmg;
        let d_u = s.unit_dmg - self.prev.unit_dmg;
        let d_h = s.heal - self.prev.heal;
        self.prev = s;
        let allies = self.allies_alive();
        let lost = self.prev_allies.saturating_sub(allies);
        self.prev_allies = allies;

        let mut r = w.building_dmg * d_b / TOWER_HP
            + w.unit_dmg * d_u / 120.0
            + w.heal * d_h / 100.0
            + w.ally_death * lost as f32
            + w.tick;
        let outcome = self.world.outcome();
        if outcome == Outcome::Running && self.world.units[0].alive {
            r += 0.995 * self.potential() - phi0;
        }
        let won = outcome == Outcome::Victory;
        let died = !me.alive && !won;
        let timeout = !won && !died && outcome == Outcome::Timeout;
        let wiped = outcome == Outcome::Wiped;
        if won {
            r += w.win;
        } else if died || wiped {
            r += w.death;
        } else if timeout {
            r += w.timeout;
        }
        self.ret += r;
        if won || died || timeout || wiped {
            let info = EpisodeInfo {
                won,
                died: died || wiped,
                timeout,
                squad: self.drill == Drill::Squad,
                ret: self.ret,
                len: self.world.tick,
                guard_share: if self.enemy_hp > 0.0 {
                    s.unit_dmg / self.enemy_hp
                } else {
                    0.0
                },
                building_share: if self.building_hp > 0.0 {
                    s.building_dmg / self.building_hp
                } else {
                    0.0
                },
                healed: s.heal,
                destruction: self.world.destruction(),
                vs_trained: !self.npcs.units.is_empty(),
                skills: s.skills as f32,
                dodges: s.dodges as f32,
                evaded: s.evaded as f32,
                blocked: s.blocked,
            };
            self.reset();
            return (r, Some(info));
        }
        (r, None)
    }
}

impl RlEnv for DrillEnv {
    fn slots(&self) -> usize {
        1
    }
    fn active(&self, _k: usize) -> bool {
        true
    }
    fn obs(&self, _k: usize) -> [f32; OBS_DIM] {
        self.world.observe(0)
    }
    fn mask(&self, _k: usize) -> [bool; N_ACTIONS] {
        self.world.mask(0)
    }
    fn act(&mut self, actions: &[u8]) -> StepOut {
        let (r, ep) = self.step(actions[0]);
        StepOut {
            rewards: vec![r],
            done: vec![ep.is_some()],
            episode: ep,
        }
    }
}

/// Potential for shaping: distance to the nearest enemy (attackers) or to
/// their post (defenders), and to the nearest ally.
pub(crate) fn potential(w: &World, i: usize, approach: f32, follow: f32) -> f32 {
    if approach == 0.0 && follow == 0.0 {
        return 0.0;
    }
    let me = &w.units[i];
    let (mut enemy, mut ally) = (f32::MAX, f32::MAX);
    for (j, u) in w.units.iter().enumerate() {
        if j == i || !u.alive {
            continue;
        }
        if u.team == me.team {
            if !u.kind.is_building() {
                ally = ally.min(me.dist(u));
            }
        } else if me.team == Team::Attack {
            let d = me.dist(u) - if u.kind.is_building() { u.radius } else { 0.0 };
            enemy = enemy.min(d);
        }
        // Defenders are pulled toward their post instead: within 4 of a
        // building they protect, where the towers cover them.
        if me.team == Team::Defend && u.team == me.team && u.kind.is_building() {
            enemy = enemy.min((me.dist(u) - 4.0).max(0.0));
        }
    }
    let mut p = 0.0;
    if enemy < f32::MAX {
        p -= approach * enemy / 10.0;
    }
    if ally < f32::MAX {
        p -= follow * (ally - 2.0).max(0.0) / 10.0;
    }
    p
}

// ----------------------------------------------------------------- defence

pub struct DefenseEnv {
    pub spec: ClassSpec,
    pub world: World,
    rng: Rng,
    pool: Option<Opponents>,
    npcs: Npcs,
    prev: Vec<crate::sim::UnitStats>,
    alive: Vec<bool>,
    building_left: f32,
    building_hp: f32,
    raider_hp: f32,
    ret: f32,
}

impl DefenseEnv {
    pub fn new(spec: ClassSpec, seed: u64, pool: Option<Opponents>) -> Self {
        let mut e = DefenseEnv {
            spec,
            world: World::new(1),
            rng: Rng::new(seed),
            pool: pool.filter(|p| !p.is_empty()),
            npcs: Npcs::default(),
            prev: Vec::new(),
            alive: Vec::new(),
            building_left: 0.0,
            building_hp: 0.0,
            raider_hp: 0.0,
            ret: 0.0,
        };
        e.reset();
        e
    }

    /// A fixed drill for evaluation and replays (scripted raiders).
    pub fn fixed(spec: ClassSpec, seed: u64) -> Self {
        let mut e = DefenseEnv::new(spec, seed, None);
        e.rng = Rng::new(seed);
        e.reset();
        e
    }

    pub fn reset(&mut self) {
        let (mut w, from) = build_defense_drill(&self.spec, &mut self.rng);
        self.npcs.units.clear();
        let vs_pool = self.pool.is_some() && self.rng.chance(self.spec.drills.self_play);
        if vs_pool {
            let pool = self.pool.clone().unwrap();
            let n = 2 + self.rng.below(2);
            for _ in 0..n {
                let p = self.rng.below(pool.len());
                let (ux, uy) = self.rng.unit_vec();
                let i = w.add_troop(
                    &pool[p].0,
                    from.0 + ux * 1.2,
                    from.1 + uy * 1.2,
                    Control::External,
                    usize::MAX,
                );
                self.npcs.units.push((i, p));
            }
        } else {
            let n = 2 + self.rng.below(2);
            add_raiders(&mut w, n, from, 1.0, &mut self.rng);
        }
        self.world = w;
        self.prev = vec![Default::default(); DEFENSE_SLOTS];
        self.alive = vec![true; DEFENSE_SLOTS];
        self.building_hp = self
            .world
            .units
            .iter()
            .filter(|u| u.kind.is_building())
            .map(|u| u.max_hp)
            .sum();
        self.building_left = self.building_hp;
        self.raider_hp = self
            .world
            .units
            .iter()
            .filter(|u| u.team == Team::Attack)
            .map(|u| u.max_hp)
            .sum();
        self.ret = 0.0;
    }

    fn buildings_left(&self) -> f32 {
        self.world
            .units
            .iter()
            .filter(|u| u.kind.is_building())
            .map(|u| u.hp)
            .sum()
    }
}

impl RlEnv for DefenseEnv {
    fn slots(&self) -> usize {
        DEFENSE_SLOTS
    }
    fn active(&self, k: usize) -> bool {
        self.world.units[k].alive
    }
    fn obs(&self, k: usize) -> [f32; OBS_DIM] {
        self.world.observe(k)
    }
    fn mask(&self, k: usize) -> [bool; N_ACTIONS] {
        self.world.mask(k)
    }

    fn act(&mut self, slot_actions: &[u8]) -> StepOut {
        let w = self.spec.weights;
        let n = DEFENSE_SLOTS;
        let phi0: Vec<f32> = (0..n)
            .map(|k| potential(&self.world, k, w.approach, w.follow))
            .collect();
        let mut actions = vec![0u8; self.world.units.len()];
        actions[..n].copy_from_slice(&slot_actions[..n]);
        if let Some(pool) = &self.pool {
            self.npcs.act(&self.world, pool, &mut actions);
        }
        self.world.step(&actions);
        let left = self.buildings_left();
        let lost = (self.building_left - left) / self.building_hp;
        self.building_left = left;
        let deaths = (0..n)
            .filter(|&k| self.alive[k] && !self.world.units[k].alive)
            .count();

        let mut rewards = vec![0.0; n];
        let mut done = vec![false; n];
        let outcome = self.world.outcome();
        let fallen = outcome == Outcome::Victory;
        let held = matches!(outcome, Outcome::Wiped | Outcome::Timeout);
        for k in 0..n {
            if !self.alive[k] {
                continue;
            }
            let u = &self.world.units[k];
            let s = u.stats;
            let d_u = s.unit_dmg - self.prev[k].unit_dmg;
            let d_h = s.heal - self.prev[k].heal;
            self.prev[k] = s;
            let mut r =
                w.unit_dmg * d_u / 100.0 + w.heal * d_h / 100.0 - w.building_dmg * lost + w.tick;
            let died_now = !u.alive;
            r += w.ally_death * (deaths - died_now as usize) as f32;
            if died_now {
                r += w.death;
                done[k] = true;
            } else if outcome == Outcome::Running {
                r += 0.995 * potential(&self.world, k, w.approach, w.follow) - phi0[k];
            }
            if held && !died_now {
                r += w.win;
            } else if fallen {
                r -= w.win;
            }
            rewards[k] = r;
            self.ret += r / n as f32;
        }
        for k in 0..n {
            self.alive[k] = self.world.units[k].alive;
        }
        // With every defender down, the raid plays out without them.
        let mut outcome = outcome;
        if outcome == Outcome::Running && !self.alive.iter().any(|&a| a) {
            let mut idle = vec![0u8; self.world.units.len()];
            while self.world.outcome() == Outcome::Running {
                if let Some(pool) = &self.pool {
                    self.npcs.act(&self.world, pool, &mut idle);
                }
                self.world.step(&idle);
            }
            outcome = self.world.outcome();
        }
        let mut episode = None;
        if outcome != Outcome::Running {
            for d in done.iter_mut() {
                *d = true;
            }
            let dmg: f32 = (0..n).map(|k| self.world.units[k].stats.unit_dmg).sum();
            let healed: f32 = (0..n).map(|k| self.world.units[k].stats.heal).sum();
            let survivors = self.alive.iter().filter(|&&a| a).count();
            let per = |f: &dyn Fn(&crate::sim::UnitStats) -> f32| -> f32 {
                (0..n).map(|k| f(&self.world.units[k].stats)).sum::<f32>() / n as f32
            };
            episode = Some(EpisodeInfo {
                won: matches!(outcome, Outcome::Wiped | Outcome::Timeout),
                died: survivors * 2 < n,
                timeout: outcome == Outcome::Timeout,
                squad: true,
                ret: self.ret,
                len: self.world.tick,
                guard_share: if self.raider_hp > 0.0 {
                    dmg / self.raider_hp
                } else {
                    0.0
                },
                building_share: self.world.destruction(),
                healed: healed / n as f32,
                destruction: self.world.destruction(),
                vs_trained: !self.npcs.units.is_empty(),
                skills: per(&|s| s.skills as f32),
                dodges: per(&|s| s.dodges as f32),
                evaded: per(&|s| s.evaded as f32),
                blocked: per(&|s| s.blocked),
            });
            self.reset();
        }
        StepOut {
            rewards,
            done,
            episode,
        }
    }
}
