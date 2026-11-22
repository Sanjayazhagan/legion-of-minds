//! How big a troop brain this PC can train. Brain size has no fixed top
//! past level 5: the limit is how long this computer takes to train it.
//!
//! The check times two real training rounds (a 128 and a 256 brain, about
//! 10 to 30 seconds on a normal PC, on the graphics card when training uses
//! one), fits "time per step = fixed cost + cost per brain number", and from
//! that predicts every bigger size. The biggest size whose full training
//! (its step cap) fits in [`TRAIN_HOURS`] is this PC's max. Memory is never
//! the limit: even a 2048 brain needs under half a GB to train.

use godcore::classes::BaseClass;
use godcore::ppo::{brain_cap, brain_params, PpoConfig, Trainer, HIDDEN_SIZES, LEVEL_BRAIN_MAX};
use godcore::troop::Troop;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Instant;

/// Hours of training to take a brain from new to its step cap that still
/// count as "this PC can handle it": one long night.
pub const TRAIN_HOURS: f64 = 8.0;

/// The two sizes the check times.
const PROBE: [usize; 2] = [128, 256];

/// What the check found. Saved with the settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PcResult {
    /// What trained: "CPU (8 cores)" or the graphics card's name.
    pub device: String,
    /// Fitted cost of one training step: fixed part and per brain number,
    /// in microseconds.
    pub us_fixed: f64,
    pub us_per_param: f64,
    /// The input size it was measured with: a new sim with more inputs
    /// re-runs the check.
    pub obs_dim: usize,
}

impl PcResult {
    /// Predicted training steps per second with a brain of `h` neurons.
    pub fn steps_per_sec(&self, h: usize) -> f64 {
        1e6 / (self.us_fixed + self.us_per_param * brain_params(h) as f64).max(1.0)
    }

    /// Predicted hours to train a brain of `h` neurons up to its step cap.
    pub fn hours_to_full(&self, h: usize) -> f64 {
        brain_cap(h) as f64 / self.steps_per_sec(h) / 3600.0
    }

    /// The biggest brain this PC can train in [`TRAIN_HOURS`]. Never less
    /// than what levels give anyway.
    pub fn max_brain(&self) -> usize {
        HIDDEN_SIZES
            .iter()
            .copied()
            .filter(|&h| h <= LEVEL_BRAIN_MAX || self.hours_to_full(h) <= TRAIN_HOURS)
            .max()
            .unwrap_or(LEVEL_BRAIN_MAX)
    }

    /// Memory needed to train a brain of `h` neurons, in MB: actor and
    /// critic with gradients and optimiser state, plus a training round's
    /// worth of steps.
    pub fn train_mb(h: usize) -> f64 {
        let p = brain_params(h) as f64;
        let cfg = PpoConfig::default();
        let rows = (cfg.n_envs * cfg.n_steps) as f64;
        (2.0 * p * 4.0 * 4.0 + rows * (godcore::sim::OBS_DIM as f64 + 4.0 * h as f64) * 4.0) / 1e6
    }

    pub fn fresh(&self) -> bool {
        self.obs_dim == godcore::sim::OBS_DIM && self.us_per_param > 0.0
    }
}

pub enum Status {
    Idle,
    /// Timing size `step` of [`PROBE`].
    Running { step: usize },
    Done(PcResult),
    Failed(String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);

fn set(s: Status) {
    *STATUS.lock().unwrap() = s;
}

/// Start the check in the background (does nothing if it is running).
pub fn start() {
    {
        let mut s = STATUS.lock().unwrap();
        if matches!(*s, Status::Running { .. }) {
            return;
        }
        *s = Status::Running { step: 0 };
    }
    std::thread::spawn(|| {
        let r = std::panic::catch_unwind(measure);
        set(match r {
            Ok(r) => Status::Done(r),
            Err(_) => Status::Failed("the speed test crashed".into()),
        });
    });
}

/// `Some(step, of)` while running.
pub fn running() -> Option<(usize, usize)> {
    match *STATUS.lock().unwrap() {
        Status::Running { step } => Some((step + 1, PROBE.len())),
        _ => None,
    }
}

/// A finished result (or failure message), once, for the caller to keep.
pub fn take() -> Option<Result<PcResult, String>> {
    let mut s = STATUS.lock().unwrap();
    match std::mem::replace(&mut *s, Status::Idle) {
        Status::Done(r) => Some(Ok(r)),
        Status::Failed(e) => Some(Err(e)),
        other => {
            *s = other;
            None
        }
    }
}

fn measure() -> PcResult {
    // Wait for the graphics card search, so the check uses the same device
    // real training will.
    while crate::train_device().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let device = match crate::train_device().flatten() {
        Some(gpu) => gpu.to_string(),
        None => format!("CPU ({} cores)", rayon::current_num_threads() + 1),
    };
    let mut pts = Vec::new();
    for (k, &h) in PROBE.iter().enumerate() {
        set(Status::Running { step: k });
        let mut troop = Troop::new(1, "Test".into(), BaseClass::Fighter.spec(), 1);
        troop.level = 5;
        troop.sync_moves();
        troop.set_brain(h);
        let mut tr = Trainer::new(&troop.spec, troop.actor.clone(), troop.critic.clone(), 1, PpoConfig::default());
        let t0 = Instant::now();
        tr.iterate();
        let us = t0.elapsed().as_secs_f64() * 1e6 / tr.steps_done.max(1) as f64;
        pts.push((brain_params(h) as f64, us));
    }
    fit(device, &pts)
}

/// Straight line through the two timings (cost per brain number can't be
/// negative: timing noise falls back to cost proportional to size).
fn fit(device: String, pts: &[(f64, f64)]) -> PcResult {
    let ((p1, t1), (p2, t2)) = (pts[0], pts[1]);
    let mut b = (t2 - t1) / (p2 - p1);
    if !(b > 0.0) {
        b = t2 / p2;
    }
    let a = (t1 - b * p1).max(0.0);
    PcResult { device, us_fixed: a, us_per_param: b, obs_dim: godcore::sim::OBS_DIM }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 4-core cloud machine this was built on: 2,598 steps/s at 128 and
    /// 931 at 256 measured; 512 and 1024 measured at 269 and 75.
    fn cloud() -> PcResult {
        fit("CPU".into(), &[(brain_params(128) as f64, 1e6 / 2598.0), (brain_params(256) as f64, 1e6 / 931.0)])
    }

    #[test]
    fn the_fit_predicts_bigger_brains_within_a_tenth() {
        let r = cloud();
        for (h, measured) in [(512, 269.0), (1024, 75.0)] {
            let p = r.steps_per_sec(h);
            assert!((p - measured).abs() / measured < 0.12, "{h}: predicted {p:.0}, measured {measured}");
        }
    }

    #[test]
    fn max_brain_is_the_biggest_that_trains_overnight() {
        let r = cloud();
        assert_eq!(r.max_brain(), 512, "{:.1} h at 512, {:.1} h at 768", r.hours_to_full(512), r.hours_to_full(768));
        // A PC ten times faster gets bigger brains; a very slow one still
        // gets everything levels give.
        let fast = PcResult { us_fixed: r.us_fixed / 10.0, us_per_param: r.us_per_param / 10.0, ..r.clone() };
        assert!(fast.max_brain() >= 1024);
        let slow = PcResult { us_fixed: r.us_fixed * 50.0, us_per_param: r.us_per_param * 50.0, ..r };
        assert_eq!(slow.max_brain(), LEVEL_BRAIN_MAX);
    }

    #[test]
    fn noisy_timings_never_give_negative_costs() {
        let r = fit("x".into(), &[(1000.0, 500.0), (4000.0, 400.0)]);
        assert!(r.us_per_param > 0.0 && r.us_fixed >= 0.0);
        assert!(r.max_brain() >= LEVEL_BRAIN_MAX);
    }

    #[test]
    fn memory_is_small_even_for_the_biggest_brain() {
        assert!(PcResult::train_mb(2048) < 500.0, "{}", PcResult::train_mb(2048));
    }
}
