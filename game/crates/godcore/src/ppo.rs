//! PPO with action masking, on the CPU, written from scratch.
//! Settings follow the tuned Python prototype (gamma 0.995, 10 epochs), with
//! 16 envs x 512 steps per update, which learned more reliably across seeds.

use crate::classes::ClassSpec;
use crate::env::{make_env, EpisodeInfo, Opponents, RlEnv};
use crate::nn::{Adam, Cache, Mlp};
use crate::rng::Rng;
use crate::sim::{N_ACTIONS, OBS_DIM};
use rayon::prelude::*;

pub const HIDDEN: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct PpoConfig {
    pub n_envs: usize,
    pub n_steps: usize,
    pub epochs: usize,
    pub minibatch: usize,
    pub lr: f32,
    pub gamma: f32,
    pub lambda: f32,
    pub clip: f32,
    pub ent_coef: f32,
    pub vf_coef: f32,
    pub max_grad_norm: f32,
}

impl Default for PpoConfig {
    fn default() -> Self {
        PpoConfig {
            n_envs: 16,
            n_steps: 512,
            epochs: 10,
            minibatch: 1024,
            lr: 3e-4,
            gamma: 0.995,
            lambda: 0.95,
            clip: 0.2,
            ent_coef: 0.01,
            vf_coef: 0.5,
            max_grad_norm: 0.5,
        }
    }
}

/// Neurons per hidden layer at each troop level. The brain grows when a
/// troop levels up (keeping what it learned), so stronger troops can learn
/// more but train a little slower.
pub fn hidden_for_level(level: u32) -> usize {
    match level {
        0..=2 => HIDDEN,
        3 => 96,
        4 => 128,
        _ => 160,
    }
}

/// Every hidden size a troop brain can have.
pub const HIDDEN_SIZES: [usize; 4] = [64, 96, 128, 160];

pub fn actor_dims(level: u32) -> Vec<usize> {
    let h = hidden_for_level(level);
    vec![OBS_DIM, h, h, N_ACTIONS]
}

pub fn critic_dims(level: u32) -> Vec<usize> {
    let h = hidden_for_level(level);
    vec![OBS_DIM, h, h, 1]
}

/// Whether `dims` is a valid actor shape for a troop of any level.
pub fn actor_dims_ok(dims: &[usize]) -> bool {
    dims.len() == 4 && dims[0] == OBS_DIM && dims[1] == dims[2] && HIDDEN_SIZES.contains(&dims[1]) && dims[3] == N_ACTIONS
}

/// Whether `dims` is the right actor shape for a troop of this level.
pub fn actor_dims_ok_for(dims: &[usize], level: u32) -> bool {
    dims == actor_dims(level).as_slice()
}

/// A fresh level 1 brain.
pub fn new_actor(rng: &mut Rng) -> Mlp {
    new_actor_for(1, rng)
}

pub fn new_critic(rng: &mut Rng) -> Mlp {
    new_critic_for(1, rng)
}

pub fn new_actor_for(level: u32, rng: &mut Rng) -> Mlp {
    Mlp::new(&actor_dims(level), 0.01, rng)
}

pub fn new_critic_for(level: u32, rng: &mut Rng) -> Mlp {
    Mlp::new(&critic_dims(level), 1.0, rng)
}

/// Masked log-softmax. Invalid actions get probability 0.
pub fn masked_log_probs(logits: &[f32], mask: &[bool; N_ACTIONS], out: &mut [f32; N_ACTIONS]) {
    let mut mx = f32::MIN;
    for a in 0..N_ACTIONS {
        if mask[a] && logits[a] > mx {
            mx = logits[a];
        }
    }
    let mut sum = 0.0;
    for a in 0..N_ACTIONS {
        if mask[a] {
            sum += (logits[a] - mx).exp();
        }
    }
    let ls = sum.ln();
    for a in 0..N_ACTIONS {
        out[a] = if mask[a] {
            logits[a] - mx - ls
        } else {
            f32::NEG_INFINITY
        };
    }
}

/// The highest-scoring legal action. Uses no exp/log, so battles replay
/// identically on every machine.
pub fn greedy_action(actor: &Mlp, obs: &[f32], mask: &[bool; N_ACTIONS], cache: &mut Cache) -> u8 {
    actor.forward(obs, cache);
    let logits = actor.output(cache);
    let mut best = 0usize;
    let mut best_v = f32::MIN;
    for a in 0..N_ACTIONS {
        if mask[a] && logits[a] > best_v {
            best_v = logits[a];
            best = a;
        }
    }
    best as u8
}

fn sample(logp: &[f32; N_ACTIONS], rng: &mut Rng) -> u8 {
    let u = rng.f32();
    let mut acc = 0.0;
    let mut last = 0;
    for a in 0..N_ACTIONS {
        if logp[a].is_finite() {
            acc += logp[a].exp();
            last = a;
            if u < acc {
                return a as u8;
            }
        }
    }
    last as u8
}

/// One PPO iteration's summary.
#[derive(Clone, Debug, Default)]
pub struct IterReport {
    pub steps_done: u64,
    pub episodes: Vec<EpisodeInfo>,
    pub policy_loss: f32,
    pub value_loss: f32,
    pub entropy: f32,
    pub seconds: f32,
}

struct Worker {
    env: Box<dyn RlEnv>,
    rng: Rng,
    cache_a: Cache,
    cache_c: Cache,
}

/// A run of one agent slot's transitions, ending in a terminal state or cut
/// off by the end of the rollout (then bootstrapped with `last_val`).
#[derive(Default)]
struct Segment {
    obs: Vec<f32>,
    mask: Vec<[bool; N_ACTIONS]>,
    act: Vec<u8>,
    logp: Vec<f32>,
    val: Vec<f32>,
    rew: Vec<f32>,
    terminal: bool,
    last_val: f32,
}

pub struct Trainer {
    pub cfg: PpoConfig,
    pub actor: Mlp,
    pub critic: Mlp,
    opt_a: Adam,
    opt_c: Adam,
    workers: Vec<Worker>,
    rng: Rng,
    pub steps_done: u64,
}

#[derive(Default)]
struct Grads {
    a: Vec<f32>,
    c: Vec<f32>,
    pl: f32,
    vl: f32,
    ent: f32,
}

impl Trainer {
    pub fn new(spec: &ClassSpec, actor: Mlp, critic: Mlp, seed: u64, cfg: PpoConfig) -> Self {
        Self::with_opponents(spec, actor, critic, seed, cfg, None)
    }

    /// Like `new`, but drills may pit the troop against these trained
    /// opponents (self-play against the player's own army).
    pub fn with_opponents(
        spec: &ClassSpec,
        actor: Mlp,
        critic: Mlp,
        seed: u64,
        cfg: PpoConfig,
        opponents: Option<Opponents>,
    ) -> Self {
        let mut rng = Rng::new(seed);
        let workers = (0..cfg.n_envs)
            .map(|_| {
                let env = make_env(spec, rng.next_u64(), opponents.clone());
                Worker {
                    env,
                    rng: Rng::new(rng.next_u64()),
                    cache_a: Cache::default(),
                    cache_c: Cache::default(),
                }
            })
            .collect();
        let (na, nc) = (actor.n_params(), critic.n_params());
        Trainer {
            cfg,
            actor,
            critic,
            opt_a: Adam::new(na),
            opt_c: Adam::new(nc),
            workers,
            rng,
            steps_done: 0,
        }
    }

    /// Collect one rollout from every env and run the PPO update on it.
    pub fn iterate(&mut self) -> IterReport {
        let t0 = std::time::Instant::now();
        let t_n = self.cfg.n_steps;
        let actor = &self.actor;
        let critic = &self.critic;

        // ---- Rollout: each env collects n_steps transitions in parallel,
        // from every agent slot that is alive.
        let per_env: Vec<(Vec<Segment>, Vec<EpisodeInfo>)> = self
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
                let mut count = 0;
                while count < t_n {
                    for k in 0..slots {
                        was_active[k] = w.env.active(k);
                        if !was_active[k] {
                            continue;
                        }
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
                        count += 1;
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
                            closed.push(seg);
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
                        critic.forward(&w.env.obs(k), &mut w.cache_c);
                        seg.last_val = critic.output(&w.cache_c)[0];
                    }
                    closed.push(seg);
                }
                (closed, eps)
            })
            .collect();

        // ---- Advantages (GAE) per segment, then flattened buffers.
        let mut obs = Vec::new();
        let mut masks = Vec::new();
        let mut acts = Vec::new();
        let mut old_logp = Vec::new();
        let mut adv = Vec::new();
        let mut rets = Vec::new();
        let mut episodes = Vec::new();
        for (segs, eps) in &per_env {
            for seg in segs {
                let len = seg.act.len();
                let mut a = vec![0.0f32; len];
                let mut gae = 0.0;
                for t in (0..len).rev() {
                    let (next_v, nonterm) = if t + 1 == len {
                        if seg.terminal {
                            (0.0, 0.0)
                        } else {
                            (seg.last_val, 1.0)
                        }
                    } else {
                        (seg.val[t + 1], 1.0)
                    };
                    let delta = seg.rew[t] + self.cfg.gamma * next_v * nonterm - seg.val[t];
                    gae = delta + self.cfg.gamma * self.cfg.lambda * nonterm * gae;
                    a[t] = gae;
                }
                for t in 0..len {
                    rets.push(a[t] + seg.val[t]);
                }
                adv.extend_from_slice(&a);
                obs.extend_from_slice(&seg.obs);
                masks.extend_from_slice(&seg.mask);
                acts.extend_from_slice(&seg.act);
                old_logp.extend_from_slice(&seg.logp);
            }
            episodes.extend_from_slice(eps);
        }
        let n = acts.len();

        // ---- PPO update.
        let mut idx: Vec<usize> = (0..n).collect();
        let (na, nc) = (self.actor.n_params(), self.critic.n_params());
        let cfg = self.cfg;
        let (mut pl_sum, mut vl_sum, mut ent_sum, mut batches) = (0.0, 0.0, 0.0, 0);
        for _ in 0..cfg.epochs {
            for i in (1..n).rev() {
                let j = self.rng.below(i + 1);
                idx.swap(i, j);
            }
            for mb in idx.chunks(cfg.minibatch) {
                let b = mb.len() as f32;
                let mean = mb.iter().map(|&i| adv[i]).sum::<f32>() / b;
                let var = mb.iter().map(|&i| (adv[i] - mean).powi(2)).sum::<f32>() / b;
                let std = var.sqrt() + 1e-8;
                let actor = &self.actor;
                let critic = &self.critic;
                // Fixed-size chunks summed in order: same result for any thread count.
                let parts: Vec<Grads> = mb
                    .par_chunks(128)
                    .map(|chunk| {
                        let mut g = Grads {
                            a: vec![0.0; na],
                            c: vec![0.0; nc],
                            ..Default::default()
                        };
                        let (mut ca, mut cc, mut scratch) =
                            (Cache::default(), Cache::default(), Vec::new());
                        let mut lp = [0.0f32; N_ACTIONS];
                        let mut d_logits = [0.0f32; N_ACTIONS];
                        for &i in chunk {
                            let o = &obs[i * OBS_DIM..(i + 1) * OBS_DIM];
                            let mask = &masks[i];
                            let a = acts[i] as usize;
                            let ad = (adv[i] - mean) / std;
                            actor.forward(o, &mut ca);
                            masked_log_probs(actor.output(&ca), mask, &mut lp);
                            let ratio = (lp[a] - old_logp[i]).exp();
                            let s1 = ratio * ad;
                            let clipped = ratio.clamp(1.0 - cfg.clip, 1.0 + cfg.clip);
                            let s2 = clipped * ad;
                            g.pl += -s1.min(s2);
                            let g_logp = if s1 <= s2 || (ratio - clipped).abs() < 1e-12 {
                                -ad * ratio
                            } else {
                                0.0
                            };
                            let mut h = 0.0;
                            for k in 0..N_ACTIONS {
                                if mask[k] {
                                    h -= lp[k].exp() * lp[k];
                                }
                            }
                            g.ent += h;
                            for k in 0..N_ACTIONS {
                                d_logits[k] = if mask[k] {
                                    let p = lp[k].exp();
                                    let ind = if k == a { 1.0 } else { 0.0 };
                                    (g_logp * (ind - p) + cfg.ent_coef * p * (lp[k] + h)) / b
                                } else {
                                    0.0
                                };
                            }
                            actor.backward(&ca, &d_logits, &mut g.a, &mut scratch);
                            critic.forward(o, &mut cc);
                            let v = critic.output(&cc)[0];
                            let diff = v - rets[i];
                            g.vl += diff * diff;
                            critic.backward(
                                &cc,
                                &[cfg.vf_coef * 2.0 * diff / b],
                                &mut g.c,
                                &mut scratch,
                            );
                        }
                        g
                    })
                    .collect();
                let mut ga = vec![0.0f32; na];
                let mut gc = vec![0.0f32; nc];
                for p in &parts {
                    for k in 0..na {
                        ga[k] += p.a[k];
                    }
                    for k in 0..nc {
                        gc[k] += p.c[k];
                    }
                    pl_sum += p.pl / b;
                    vl_sum += p.vl / b;
                    ent_sum += p.ent / b;
                }
                batches += 1;
                let norm = (ga.iter().chain(gc.iter()).map(|x| x * x).sum::<f32>()).sqrt();
                if norm > cfg.max_grad_norm {
                    let s = cfg.max_grad_norm / norm;
                    ga.iter_mut().chain(gc.iter_mut()).for_each(|x| *x *= s);
                }
                self.opt_a.step(&mut self.actor.params, &ga, cfg.lr);
                self.opt_c.step(&mut self.critic.params, &gc, cfg.lr);
            }
        }
        self.steps_done += n as u64;
        let bn = batches.max(1) as f32;
        IterReport {
            steps_done: self.steps_done,
            episodes,
            policy_loss: pl_sum / bn,
            value_loss: vl_sum / bn,
            entropy: ent_sum / bn,
            seconds: t0.elapsed().as_secs_f32(),
        }
    }
}
