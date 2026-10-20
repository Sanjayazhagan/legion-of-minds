//! Team training: several different troops, each with its own brain, train
//! together in one drill. Each troop is rewarded partly for its own play and
//! partly for how the whole team did, so covering a teammate pays off, and
//! each sees its nearest teammates' roles and what they are doing.

use crate::classes::{ClassSpec, Role};
use crate::env::{EpisodeInfo, Opponents};
use crate::nn::{Adam, Cache, Mlp};
use crate::ppo::{masked_log_probs, ppo_update, sample, Batch, PpoConfig, Segment};
use crate::rng::Rng;
use crate::scenario::{add_raiders, build_team_defense, build_team_drill, MAX_TEAM, MIN_TEAM};
use crate::sim::{Control, Kind, Outcome, Team, World, N_ACTIONS, TOWER_HP};
use rayon::prelude::*;

/// How much of each troop's reward is the team's average reward instead of
/// its own (0 = selfish, 1 = only the team counts).
pub const TEAM_SPIRIT: f32 = 0.5;

/// Why a set of troops can't train as a team.
pub fn team_problem(team: &[ClassSpec]) -> Option<&'static str> {
    if team.len() < MIN_TEAM {
        Some("Pick at least 2 troops.")
    } else if team.len() > MAX_TEAM {
        Some("A team has at most 5 troops.")
    } else if team.iter().any(|s| s.role != team[0].role) {
        Some("A team is all attackers or all defenders.")
    } else {
        None
    }
}

/// One team drill, attack or defence, with one agent slot per team member.
pub struct TeamEnv {
    pub team: Vec<ClassSpec>,
    pub world: World,
    pub rng: Rng,
    pool: Option<Opponents>,
    npcs: Vec<(usize, usize)>,
    cache: Cache,
    prev: Vec<crate::sim::UnitStats>,
    alive: Vec<bool>,
    building_left: f32,
    building_hp: f32,
    enemy_hp: f32,
    ret: f32,
}

impl TeamEnv {
    pub fn new(team: Vec<ClassSpec>, seed: u64, pool: Option<Opponents>) -> Self {
        let mut e = TeamEnv {
            team,
            world: World::new(1),
            rng: Rng::new(seed),
            pool: pool.filter(|p| !p.is_empty()),
            npcs: Vec::new(),
            cache: Cache::default(),
            prev: Vec::new(),
            alive: Vec::new(),
            building_left: 0.0,
            building_hp: 0.0,
            enemy_hp: 0.0,
            ret: 0.0,
        };
        e.reset();
        e
    }

    fn defending(&self) -> bool {
        self.team[0].role == Role::Defend
    }

    /// Share of drills against the player's own trained troops.
    fn self_play(&self) -> f32 {
        self.team.iter().map(|s| s.drills.self_play).sum::<f32>() / self.team.len() as f32
    }

    pub fn reset(&mut self) {
        self.npcs.clear();
        let n = self.team.len();
        let vs_pool = self.pool.is_some() && self.rng.chance(self.self_play());
        if self.defending() {
            let (mut w, from) = build_team_defense(&self.team, &mut self.rng);
            let raiders = (n - 1 + self.rng.below(2)).max(2);
            if vs_pool {
                let pool = self.pool.clone().unwrap();
                for _ in 0..raiders {
                    let p = self.rng.below(pool.len());
                    let (ux, uy) = self.rng.unit_vec();
                    let i = w.add_troop(&pool[p].0, from.0 + ux * 1.2, from.1 + uy * 1.2, Control::External, usize::MAX);
                    self.npcs.push((i, p));
                }
            } else {
                add_raiders(&mut w, raiders, from, 1.0, &mut self.rng);
            }
            self.world = w;
        } else {
            let mut w = build_team_drill(&self.team, &mut self.rng);
            if vs_pool {
                // The player's own defenders take the guards' places.
                let pool = self.pool.clone().unwrap();
                for i in 0..w.units.len() {
                    if w.units[i].kind == Kind::Guard {
                        let p = self.rng.below(pool.len());
                        let (x, y) = (w.units[i].x, w.units[i].y);
                        w.units[i].alive = false;
                        w.units[i].hp = 0.0;
                        let j = w.add_troop_on(Team::Defend, &pool[p].0, x, y, Control::External, usize::MAX);
                        self.npcs.push((j, p));
                    }
                }
            }
            self.world = w;
        }
        self.prev = vec![Default::default(); n];
        self.alive = vec![true; n];
        self.building_hp = self.world.units.iter().filter(|u| u.kind.is_building()).map(|u| u.max_hp).sum();
        self.building_left = self.building_hp;
        let enemy = if self.defending() { Team::Attack } else { Team::Defend };
        self.enemy_hp = self
            .world
            .units
            .iter()
            .filter(|u| u.alive && u.team == enemy && !u.kind.is_building())
            .map(|u| u.max_hp)
            .sum();
        self.ret = 0.0;
    }

    pub fn slots(&self) -> usize {
        self.team.len()
    }

    pub fn active(&self, k: usize) -> bool {
        self.world.units[k].alive
    }

    pub fn obs(&self, k: usize) -> [f32; crate::sim::OBS_DIM] {
        self.world.observe(k)
    }

    pub fn mask(&self, k: usize) -> [bool; N_ACTIONS] {
        self.world.mask(k)
    }

    fn buildings_left(&self) -> f32 {
        self.world.units.iter().filter(|u| u.kind.is_building()).map(|u| u.hp).sum()
    }

    fn npcs_act(&mut self, actions: &mut [u8]) {
        if let Some(pool) = &self.pool {
            for &(i, p) in &self.npcs {
                if self.world.units[i].alive {
                    actions[i] = crate::ppo::greedy_action(&pool[p].1, &self.world.observe(i), &self.world.mask(i), &mut self.cache);
                }
            }
        }
    }

    /// Step every member. Each member's reward is its own share (its class
    /// and plan weights) blended with the team's average.
    pub fn act(&mut self, slot_actions: &[u8]) -> crate::env::StepOut {
        let n = self.team.len();
        let defend = self.defending();
        let phi0: Vec<f32> = (0..n)
            .map(|k| {
                let w = self.team[k].weights;
                crate::env::potential(&self.world, k, w.approach, w.follow)
            })
            .collect();
        let mut actions = vec![0u8; self.world.units.len()];
        actions[..n].copy_from_slice(&slot_actions[..n]);
        self.npcs_act(&mut actions);
        self.world.step(&actions);
        let left = self.buildings_left();
        let lost = (self.building_left - left) / self.building_hp.max(1.0);
        self.building_left = left;
        let deaths = (0..n).filter(|&k| self.alive[k] && !self.world.units[k].alive).count();
        let mut outcome = self.world.outcome();
        let members_left = (0..n).any(|k| self.world.units[k].alive);
        if !defend && outcome == Outcome::Running && !members_left {
            outcome = Outcome::Wiped;
        }
        // With every defender down, the raid plays out without them.
        if defend && outcome == Outcome::Running && !members_left {
            let mut idle = vec![0u8; self.world.units.len()];
            while self.world.outcome() == Outcome::Running {
                self.npcs_act(&mut idle);
                self.world.step(&idle);
            }
            outcome = self.world.outcome();
        }
        let over = outcome != Outcome::Running;

        let mut own = vec![0.0f32; n];
        let mut stepped = vec![false; n];
        let mut done = vec![false; n];
        for k in 0..n {
            if !self.alive[k] {
                continue;
            }
            stepped[k] = true;
            let w = self.team[k].weights;
            let u = &self.world.units[k];
            let s = u.stats;
            let d_b = s.building_dmg - self.prev[k].building_dmg;
            let d_u = s.unit_dmg - self.prev[k].unit_dmg;
            let d_h = s.heal - self.prev[k].heal;
            self.prev[k] = s;
            let died_now = !u.alive;
            let others_died = (deaths - died_now as usize) as f32;
            let mut r = if defend {
                w.unit_dmg * d_u / 100.0 + w.heal * d_h / 100.0 - w.building_dmg * lost + w.tick
            } else {
                w.building_dmg * d_b / TOWER_HP + w.unit_dmg * d_u / 120.0 + w.heal * d_h / 100.0 + w.tick
            };
            r += w.ally_death * others_died;
            if died_now {
                r += w.death;
                done[k] = true;
            } else if !over {
                r += 0.995 * crate::env::potential(&self.world, k, w.approach, w.follow) - phi0[k];
            }
            if over && !died_now {
                r += match (defend, outcome) {
                    (true, Outcome::Victory) => -w.win,
                    (true, _) => w.win,
                    (false, Outcome::Victory) => w.win,
                    (false, Outcome::Timeout) => w.timeout,
                    (false, _) => w.death,
                };
            }
            own[k] = r;
        }
        let m = stepped.iter().filter(|&&s| s).count().max(1) as f32;
        let team_avg = own.iter().sum::<f32>() / m;
        let rewards: Vec<f32> = (0..n)
            .map(|k| if stepped[k] { (1.0 - TEAM_SPIRIT) * own[k] + TEAM_SPIRIT * team_avg } else { 0.0 })
            .collect();
        self.ret += team_avg;
        for k in 0..n {
            self.alive[k] = self.world.units[k].alive;
        }
        let mut episode = None;
        if over {
            done.iter_mut().for_each(|d| *d = true);
            let survivors = self.alive.iter().filter(|&&a| a).count();
            let per = |f: &dyn Fn(&crate::sim::UnitStats) -> f32| -> f32 {
                (0..n).map(|k| f(&self.world.units[k].stats)).sum::<f32>() / n as f32
            };
            let dmg: f32 = (0..n).map(|k| self.world.units[k].stats.unit_dmg).sum();
            let won = if defend { outcome != Outcome::Victory } else { outcome == Outcome::Victory };
            episode = Some(EpisodeInfo {
                won,
                died: survivors * 2 < n,
                timeout: outcome == Outcome::Timeout,
                squad: true,
                ret: self.ret,
                len: self.world.tick,
                guard_share: if self.enemy_hp > 0.0 { dmg / self.enemy_hp } else { 0.0 },
                building_share: self.world.destruction(),
                healed: per(&|s| s.heal),
                destruction: self.world.destruction(),
                vs_trained: !self.npcs.is_empty(),
                skills: per(&|s| s.skills as f32),
                dodges: per(&|s| s.dodges as f32),
                evaded: per(&|s| s.evaded as f32),
                blocked: per(&|s| s.blocked),
            });
            self.reset();
        }
        crate::env::StepOut { rewards, done, episode }
    }
}

/// One team member's brain and its optimiser state.
pub struct Member {
    pub actor: Mlp,
    pub critic: Mlp,
    opt_a: Adam,
    opt_c: Adam,
    /// Training steps this member has taken so far in this session.
    pub steps: u64,
}

struct TeamWorker {
    env: TeamEnv,
    rng: Rng,
    cache_a: Cache,
    cache_c: Cache,
}

/// What one team training round produced.
#[derive(Clone, Debug, Default)]
pub struct TeamReport {
    pub episodes: Vec<EpisodeInfo>,
    /// Training steps each member took this round.
    pub steps: Vec<u64>,
    pub seconds: f32,
}

/// PPO for a team: one rollout of the shared drills, then each member's
/// brain is updated on its own experience.
pub struct TeamTrainer {
    pub cfg: PpoConfig,
    pub members: Vec<Member>,
    workers: Vec<TeamWorker>,
    rng: Rng,
}

impl TeamTrainer {
    pub fn new(team: Vec<(ClassSpec, Mlp, Mlp)>, seed: u64, cfg: PpoConfig, pool: Option<Opponents>) -> Self {
        let specs: Vec<ClassSpec> = team.iter().map(|t| t.0.clone()).collect();
        let mut rng = Rng::new(seed);
        // The same number of decisions per round as solo training, shared
        // across the team's drills.
        let workers = (0..cfg.n_envs)
            .map(|_| TeamWorker {
                env: TeamEnv::new(specs.clone(), rng.next_u64(), pool.clone()),
                rng: Rng::new(rng.next_u64()),
                cache_a: Cache::default(),
                cache_c: Cache::default(),
            })
            .collect();
        let members = team
            .into_iter()
            .map(|(_, actor, critic)| {
                let (na, nc) = (actor.n_params(), critic.n_params());
                Member { actor, critic, opt_a: Adam::new(na), opt_c: Adam::new(nc), steps: 0 }
            })
            .collect();
        TeamTrainer { cfg, members, workers, rng }
    }

    pub fn iterate(&mut self) -> TeamReport {
        let t0 = std::time::Instant::now();
        let t_n = self.cfg.n_steps;
        let members = &self.members;
        let nm = members.len();

        // ---- Rollout: every drill collects n_steps decisions per member slot.
        let per_env: Vec<(Vec<(usize, Segment)>, Vec<EpisodeInfo>)> = self
            .workers
            .par_iter_mut()
            .map(|w| {
                let slots = w.env.slots();
                let mut open: Vec<Segment> = (0..slots).map(|_| Segment::default()).collect();
                let mut closed = Vec::new();
                let mut eps = Vec::new();
                let mut lp = [0.0f32; N_ACTIONS];
                let mut acts = vec![0u8; slots];
                let mut was_active = vec![false; slots];
                for _ in 0..t_n {
                    for k in 0..slots {
                        was_active[k] = w.env.active(k);
                        if !was_active[k] {
                            continue;
                        }
                        let (actor, critic) = (&members[k].actor, &members[k].critic);
                        let o = w.env.obs(k);
                        let mask = w.env.mask(k);
                        actor.forward(&o, &mut w.cache_a);
                        masked_log_probs(actor.output(&w.cache_a), &mask, &mut lp);
                        let a = sample(&lp, &mut w.rng);
                        critic.forward(&o, &mut w.cache_c);
                        let seg = &mut open[k];
                        seg.obs.extend_from_slice(&o);
                        seg.mask.push(mask);
                        seg.act.push(a);
                        seg.logp.push(lp[a as usize]);
                        seg.val.push(w.cache_c.acts.last().unwrap()[0]);
                        acts[k] = a;
                    }
                    let out = w.env.act(&acts);
                    for k in 0..slots {
                        if !was_active[k] {
                            continue;
                        }
                        open[k].rew.push(out.rewards[k]);
                        if out.done[k] {
                            let mut seg = std::mem::take(&mut open[k]);
                            seg.terminal = true;
                            closed.push((k, seg));
                        }
                    }
                    if let Some(e) = out.episode {
                        eps.push(e);
                    }
                }
                for (k, mut seg) in open.into_iter().enumerate() {
                    if seg.act.is_empty() {
                        continue;
                    }
                    if w.env.active(k) {
                        let critic = &members[k].critic;
                        critic.forward(&w.env.obs(k), &mut w.cache_c);
                        seg.last_val = critic.output(&w.cache_c)[0];
                    }
                    closed.push((k, seg));
                }
                (closed, eps)
            })
            .collect();

        // ---- Each member learns from its own decisions.
        let mut episodes = Vec::new();
        for (_, eps) in &per_env {
            episodes.extend_from_slice(eps);
        }
        let mut steps = vec![0u64; nm];
        let cfg = self.cfg;
        for (k, m) in self.members.iter_mut().enumerate() {
            let segs = per_env.iter().flat_map(|(s, _)| s.iter()).filter(|(i, _)| *i == k).map(|(_, s)| s);
            let batch = Batch::from_segments(segs, &cfg);
            if batch.acts.is_empty() {
                continue;
            }
            steps[k] = batch.acts.len() as u64;
            m.steps += steps[k];
            ppo_update(&cfg, &mut m.actor, &mut m.critic, &mut m.opt_a, &mut m.opt_c, &mut self.rng, &batch);
        }
        TeamReport { episodes, steps, seconds: t0.elapsed().as_secs_f32() }
    }
}

impl TeamTrainer {
    /// A trainer for these troops, each starting from its own brain.
    pub fn for_troops(troops: &[crate::troop::Troop], seed: u64, cfg: PpoConfig, pool: Option<Opponents>) -> Self {
        let team = troops.iter().map(|t| (t.spec.clone(), t.actor.clone(), t.critic.clone())).collect();
        Self::new(team, seed, cfg, pool)
    }

    /// Give the trained brains back to the troops. Each troop's own decisions
    /// count as its training (towards its level cap, weapon mastery and move
    /// practice). Returns any new weapon mastery level per troop.
    pub fn finish(self, troops: &mut [crate::troop::Troop], recent: &[EpisodeInfo]) -> Vec<Option<u8>> {
        let skills = crate::troop::Skills::from_episodes(recent);
        self.members
            .into_iter()
            .zip(troops.iter_mut())
            .map(|(m, t)| {
                t.actor = m.actor;
                t.critic = m.critic;
                t.sessions += 1;
                t.team_steps += m.steps;
                let up = t.add_training(m.steps);
                t.curve.push(crate::troop::CurvePoint { steps: t.steps, win: skills.win, died: 1.0 - skills.survival, timeout: 0.0, ret: 0.0 });
                up
            })
            .collect()
    }
}
