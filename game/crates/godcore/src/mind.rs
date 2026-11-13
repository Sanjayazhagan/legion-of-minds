//! "See what it's thinking": a read-only peek into one trained troop's head.
//!
//! Everything here only reads the world and runs the troop's brain once more
//! on the same inputs a battle would give it, so looking never changes what
//! happens. The choice marked `next` is exactly what `Battle::step` will make
//! the troop do on the next tick.

use crate::nn::{Cache, Mlp};
use crate::ppo::{greedy_action, masked_log_probs};
use crate::sim::{World, N_ACTIONS};

/// Something the troop has in its head about one enemy unit or building.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sighting {
    /// Unit index.
    pub j: usize,
    /// Where the troop thinks it is (for a memory, where it last saw it).
    pub x: f32,
    pub y: f32,
    /// 1 = in sight now; lower = an older memory, fading to 0.
    pub fresh: f32,
}

#[derive(Clone, Debug)]
pub struct Thought {
    pub unit: usize,
    /// Enemies and enemy buildings in sight right now.
    pub sees: Vec<Sighting>,
    /// In sight, but just spotted: it can't act on them for a moment yet.
    pub spotted: Vec<usize>,
    /// Out of sight, but remembered where they were.
    pub remembers: Vec<Sighting>,
    /// Where the nearest fight it can hear is.
    pub hears: Option<(f32, f32)>,
    /// Calls from teammates it has taken in: (kind, x, y they point at).
    pub calls: Vec<(u8, f32, f32)>,
    /// Its options with how strongly the brain leans to each, best first
    /// (adds up to 1 over every allowed option).
    pub choices: Vec<(u8, f32)>,
    /// What it will do next tick.
    pub next: u8,
    /// Who `next` lands on, if it aims at someone.
    pub target: Option<usize>,
}

/// Look inside troop `i`'s head. `actor` is its brain.
pub fn think(w: &World, i: usize, actor: &Mlp) -> Thought {
    let me = &w.units[i];
    let mut sees = Vec::new();
    let mut spotted = Vec::new();
    let mut remembers = Vec::new();
    for j in 0..w.units.len() {
        let u = &w.units[j];
        if j == i || !u.alive || u.team == me.team {
            continue;
        }
        let in_sight = w.sees(i, j);
        match w.belief(i, j) {
            Some((x, y, fresh)) if in_sight && fresh >= 1.0 => {
                sees.push(Sighting { j, x, y, fresh })
            }
            Some((x, y, fresh)) => remembers.push(Sighting { j, x, y, fresh }),
            None if in_sight => spotted.push(j),
            None => {}
        }
    }
    let mut cache = Cache::default();
    let obs = w.observe(i);
    let mask = w.mask(i);
    let next = greedy_action(actor, &obs, &mask, &mut cache);
    let mut lp = [0.0f32; N_ACTIONS];
    masked_log_probs(actor.output(&cache), &mask, &mut lp);
    let mut choices: Vec<(u8, f32)> = (0..N_ACTIONS)
        .filter(|&a| mask[a])
        .map(|a| (a as u8, lp[a].exp()))
        .collect();
    choices.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    Thought {
        unit: i,
        sees,
        spotted,
        remembers,
        hears: w.hears(i),
        calls: w.calls_heard(i),
        choices,
        next,
        target: w.aim(i, next),
    }
}
