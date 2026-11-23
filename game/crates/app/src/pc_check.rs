//! How big a troop brain this PC can handle. Brain size, and with it the
//! highest level a troop can reach, has no fixed top: the only limits are
//! this computer's.
//!
//! The check times two real training rounds (a 128 and a 256 brain, about
//! 10 to 30 seconds on a normal PC, on the graphics card when training uses
//! one), fits "time per step = fixed cost + cost per brain number", and from
//! that predicts every bigger size. It also times a big brain thinking (what
//! battles need) and reads how much memory the PC has. A size fits this PC
//! when all three hold:
//!
//! - Training: a full training of it (its step cap) takes at most
//!   [`TRAIN_DAYS`] of background training. Training runs while you play
//!   or leave the game open, so this is generous; a PC without a graphics
//!   card trains big brains far slower and so stops sooner.
//! - Battles: [`BATTLE_BRAINS`] brains of that size can all think within
//!   [`THINK_MS`], so battles still play smoothly at 2x speed.
//! - Memory: training it needs at most [`RAM_SHARE`] of the PC's memory.
//!
//! Each size past 256 is one level past 5, so this PC's max brain is also
//! its max troop level.

use godcore::classes::BaseClass;
use godcore::nn::{Cache, Mlp};
use godcore::ppo::{actor_dims_h, brain_cap, brain_params, brain_sizes, level_for_brain, PpoConfig, Trainer, LEVEL_BRAIN_MAX};
use godcore::rng::Rng;
use godcore::troop::Troop;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Instant;

/// Days of background training a full training may take on this PC.
pub const TRAIN_DAYS: f64 = 7.0;

/// Brains that think every battle tick in a big battle (a full army against
/// a full base).
pub const BATTLE_BRAINS: f64 = 12.0;

/// Time they may take for one tick, in milliseconds: half a tick at 2x
/// battle speed (12 ticks a second), leaving the rest for the game.
pub const THINK_MS: f64 = 40.0;

/// Share of the PC's memory training one brain may use.
pub const RAM_SHARE: f64 = 0.5;

/// Memory assumed when the PC won't say how much it has, in MB.
const RAM_FALLBACK_MB: f64 = 8192.0;

/// The two sizes the check times.
const PROBE: [usize; 2] = [128, 256];

/// The size whose thinking the check times.
const THINK_PROBE: usize = 1024;

/// What stops a brain size from fitting this PC.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Limit {
    /// Its full training would take longer than [`TRAIN_DAYS`].
    TrainTime,
    /// Battles with it would stutter.
    BattleSpeed,
    /// Training it needs too much memory.
    Memory,
}

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
    /// Cost of one brain thinking once (a battle tick), per brain number,
    /// in microseconds. 0 in results from before battles were timed, which
    /// re-runs the check.
    #[serde(default)]
    pub us_think_per_param: f64,
    /// The PC's memory in MB (0 if it wouldn't say).
    #[serde(default)]
    pub ram_mb: f64,
    /// Whether training ran on a graphics card.
    #[serde(default)]
    pub gpu: bool,
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

    /// Predicted milliseconds for [`BATTLE_BRAINS`] brains of `h` neurons
    /// to think once.
    pub fn think_ms(&self, h: usize) -> f64 {
        BATTLE_BRAINS * self.us_think_per_param * brain_params(h) as f64 / 1000.0
    }

    /// Memory this PC lets training use, in MB.
    pub fn ram_budget_mb(&self) -> f64 {
        RAM_SHARE * if self.ram_mb > 0.0 { self.ram_mb } else { RAM_FALLBACK_MB }
    }

    /// Why a brain of `h` neurons doesn't fit this PC, or None if it does.
    /// Sizes levels give always fit.
    pub fn limit(&self, h: usize) -> Option<Limit> {
        if h <= LEVEL_BRAIN_MAX {
            None
        } else if Self::train_mb(h) > self.ram_budget_mb() {
            Some(Limit::Memory)
        } else if self.think_ms(h) > THINK_MS {
            Some(Limit::BattleSpeed)
        } else if self.hours_to_full(h) > TRAIN_DAYS * 24.0 {
            Some(Limit::TrainTime)
        } else {
            None
        }
    }

    /// The biggest brain this PC can train and fight with. Never less than
    /// what levels give anyway.
    pub fn max_brain(&self) -> usize {
        brain_sizes().take_while(|&h| self.limit(h).is_none()).last().unwrap_or(LEVEL_BRAIN_MAX)
    }

    /// The highest troop level this PC allows: 5, plus one for every brain
    /// size past 256 it can handle.
    pub fn max_level(&self) -> u32 {
        level_for_brain(self.max_brain())
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
        self.obs_dim == godcore::sim::OBS_DIM && self.us_per_param > 0.0 && self.us_think_per_param > 0.0
    }
}

/// "5 h", "3 days", "40 min": how long a full training takes, in words.
pub fn duration_words(hours: f64) -> String {
    if hours < 1.0 {
        format!("{:.0} min", (hours * 60.0).max(1.0))
    } else if hours < 48.0 {
        format!("{hours:.0} h")
    } else if hours < 24.0 * 60.0 {
        format!("{:.0} days", hours / 24.0)
    } else {
        format!("{:.0} months", hours / 24.0 / 30.0)
    }
}

/// The PC's total memory in MB, if it says.
pub fn total_ram_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: f64 = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024.0)
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemoryStatusEx {
            length: u32,
            memory_load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page_file: u64,
            avail_page_file: u64,
            total_virtual: u64,
            avail_virtual: u64,
            avail_extended_virtual: u64,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
        }
        let mut m = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // SAFETY: the struct has the layout Windows expects and its length
        // field is set, as the call requires.
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
        ok.then(|| m.total_phys as f64 / (1024.0 * 1024.0))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
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
    let gpu = crate::train_device().flatten();
    let device = match gpu {
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
    let mut r = fit(device, &pts);
    r.us_think_per_param = time_thinking();
    r.ram_mb = total_ram_mb().unwrap_or(0.0);
    r.gpu = gpu.is_some();
    r
}

/// Time a big brain thinking, as battles do (one move at a time on the
/// CPU), and return the cost per brain number in microseconds.
fn time_thinking() -> f64 {
    let mut rng = Rng::new(7);
    let brain = Mlp::new(&actor_dims_h(THINK_PROBE), 0.01, &mut rng);
    let obs: Vec<f32> = (0..godcore::sim::OBS_DIM).map(|k| (k as f32 * 0.37).sin()).collect();
    let mut cache = Cache::default();
    brain.forward(&obs, &mut cache);
    let (t0, mut n) = (Instant::now(), 0u32);
    while n < 20 || t0.elapsed().as_secs_f64() < 0.3 {
        brain.forward(&obs, &mut cache);
        n += 1;
    }
    std::hint::black_box(brain.output(&cache));
    (t0.elapsed().as_secs_f64() * 1e6 / n as f64 / brain_params(THINK_PROBE) as f64).max(1e-6)
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
    PcResult { device, us_fixed: a, us_per_param: b, obs_dim: godcore::sim::OBS_DIM, us_think_per_param: 0.0, ram_mb: 0.0, gpu: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 4-core cloud machine this was built on (sim with 154 inputs):
    /// 3,081 steps/s at 128 and 952 at 256 measured; 512 and 1024 measured
    /// at 275 and 83 (`godcore` example `brainbench`). A 1024 brain thinks
    /// in about 1.1 ms there; 16 GB of memory.
    fn cloud() -> PcResult {
        let mut r = fit("CPU".into(), &[(brain_params(128) as f64, 1e6 / 3081.0), (brain_params(256) as f64, 1e6 / 952.0)]);
        r.us_think_per_param = 1100.0 / brain_params(1024) as f64;
        r.ram_mb = 16384.0;
        r
    }

    fn faster(r: &PcResult, k: f64) -> PcResult {
        PcResult { us_fixed: r.us_fixed / k, us_per_param: r.us_per_param / k, ..r.clone() }
    }

    #[test]
    fn the_fit_predicts_bigger_brains_within_a_tenth() {
        let r = cloud();
        for (h, measured) in [(512, 275.0), (1024, 83.0)] {
            let p = r.steps_per_sec(h);
            assert!((p - measured).abs() / measured < 0.12, "{h}: predicted {p:.0}, measured {measured}");
        }
    }

    #[test]
    fn max_brain_and_level_come_from_the_pc() {
        let r = cloud();
        // A CPU without a graphics card: a week of background training
        // takes it to 1536 (level 10, about 6 days); 2048 would take weeks
        // to train and make battles stutter.
        assert_eq!(r.max_brain(), 1536, "{:.0} h at 1536, {:.0} h at 2048", r.hours_to_full(1536), r.hours_to_full(2048));
        assert_eq!(r.max_level(), 10);
        assert!(r.limit(2048).is_some());
        // Ten times slower at training (an old laptop): stops at 512.
        let laptop = faster(&r, 0.1);
        assert_eq!(laptop.max_brain(), 512);
        assert_eq!(laptop.limit(768), Some(Limit::TrainTime));
        // A graphics card that trains 20x faster goes past the old 2048
        // top, until battles would stutter on its CPU.
        let gpu = faster(&r, 20.0);
        assert!(gpu.max_brain() >= 1536, "{}", gpu.max_brain());
        assert_eq!(gpu.limit(gpu.max_brain() * 2), Some(Limit::BattleSpeed));
        // With fast thinking too, it goes past 2048, with no fixed top.
        let monster = PcResult { us_think_per_param: r.us_think_per_param / 10.0, ram_mb: 131072.0, ..faster(&r, 100.0) };
        assert!(monster.max_brain() > 2048, "{}", monster.max_brain());
        assert!(monster.max_level() > 11);
        // Even a PC 50x slower reaches level 6 in a week; a hopelessly slow
        // one still gets everything levels give.
        assert_eq!(faster(&r, 1.0 / 50.0).max_level(), 6);
        let slow = faster(&r, 1.0 / 1000.0);
        assert_eq!(slow.max_brain(), LEVEL_BRAIN_MAX);
        assert_eq!(slow.max_level(), 5);
    }

    #[test]
    fn little_memory_limits_brain_size() {
        let mut r = PcResult { us_think_per_param: 1e-9, ..faster(&cloud(), 1e4) };
        r.ram_mb = 4096.0;
        let max = r.max_brain();
        assert!(PcResult::train_mb(max) <= 2048.0);
        assert_eq!(r.limit(godcore::ppo::next_brain_size(max).unwrap()), Some(Limit::Memory));
    }

    #[test]
    fn noisy_timings_never_give_negative_costs() {
        let mut r = fit("x".into(), &[(1000.0, 500.0), (4000.0, 400.0)]);
        assert!(r.us_per_param > 0.0 && r.us_fixed >= 0.0);
        r.us_think_per_param = 1e-3;
        assert!(r.max_brain() >= LEVEL_BRAIN_MAX);
    }

    #[test]
    fn old_results_without_battle_timing_rerun_the_check() {
        let old = r#"{"device":"CPU","us_fixed":10.0,"us_per_param":0.01,"obs_dim":0}"#;
        let mut r: PcResult = serde_json::from_str(old).unwrap();
        r.obs_dim = godcore::sim::OBS_DIM;
        assert!(!r.fresh());
    }

    #[test]
    fn memory_is_read_on_this_os() {
        if cfg!(any(target_os = "linux", windows)) {
            assert!(total_ram_mb().unwrap() > 256.0);
        }
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(duration_words(0.5), "30 min");
        assert_eq!(duration_words(5.0), "5 h");
        assert_eq!(duration_words(24.0 * 9.0), "9 days");
    }
}
