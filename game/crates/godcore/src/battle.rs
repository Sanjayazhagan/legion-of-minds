//! Battles and tournaments: trained troops act greedily from their policy in
//! the same deterministic sim they trained in.

use crate::env::{DefenseEnv, DrillEnv, RlEnv};
use crate::nn::Cache;
use crate::ppo::greedy_action;
use crate::scenario::{
    add_raiders, build_base, build_home_base, BaseLayout, Drill, HomeBase, Wave, RAID_ENTRY,
};
use crate::sim::{Control, Kind, Outcome, Team, World};
use crate::troop::Troop;

pub struct Battle {
    pub world: World,
    pub troops: Vec<Troop>,
    cache: Cache,
    actions: Vec<u8>,
}

impl Battle {
    /// `deploy` pairs each troop with its starting position.
    pub fn new(base: &BaseLayout, deploy: Vec<(Troop, (f32, f32))>) -> Battle {
        let mut world = build_base(base);
        let mut troops = Vec::new();
        for (k, (t, (x, y))) in deploy.into_iter().enumerate() {
            world.add_troop(&t.spec, x, y, Control::External, k);
            troops.push(t);
        }
        let n = world.units.len();
        Battle {
            world,
            troops,
            cache: Cache::default(),
            actions: vec![0; n],
        }
    }

    /// A raid on the player's home base. Defenders are trained troops; the
    /// raiders are a scripted wave or another army of trained troops.
    pub fn defense(
        base: &HomeBase,
        defenders: Vec<(Troop, (f32, f32))>,
        wave: Option<&Wave>,
        army: Vec<(Troop, (f32, f32))>,
    ) -> Battle {
        let mut world = build_home_base(base);
        let mut troops = Vec::new();
        for (t, (x, y)) in defenders {
            world.add_troop_on(Team::Defend, &t.spec, x, y, Control::External, troops.len());
            troops.push(t);
        }
        if let Some(w) = wave {
            let mut rng = crate::rng::Rng::new(0x5EED + w.raiders as u64);
            add_raiders(&mut world, w.raiders, RAID_ENTRY, w.strength, &mut rng);
        }
        for (t, (x, y)) in army {
            world.add_troop(&t.spec, x, y, Control::External, troops.len());
            troops.push(t);
        }
        let n = world.units.len();
        Battle {
            world,
            troops,
            cache: Cache::default(),
            actions: vec![0; n],
        }
    }

    pub fn outcome(&self) -> Outcome {
        self.world.outcome()
    }

    pub fn step(&mut self) {
        if self.world.outcome() != Outcome::Running {
            return;
        }
        for i in 0..self.world.units.len() {
            let u = &self.world.units[i];
            self.actions[i] = 0;
            if u.alive && u.kind == Kind::Troop && u.control == Control::External {
                let obs = self.world.observe(i);
                let mask = self.world.mask(i);
                self.actions[i] =
                    greedy_action(&self.troops[u.tag].actor, &obs, &mask, &mut self.cache);
            }
        }
        self.world.step(&self.actions);
    }

    pub fn run(&mut self) {
        while self.world.outcome() == Outcome::Running {
            self.step();
        }
    }
}

/// Order-sensitive hash of the full world state, to check replays match.
pub fn fingerprint(w: &World) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    mix(w.tick as u64);
    for u in &w.units {
        mix(u.x.to_bits() as u64);
        mix(u.y.to_bits() as u64);
        mix(u.hp.to_bits() as u64);
        mix(u.cd as u64);
        mix(u.alive as u64);
    }
    h
}

/// Run a troop greedily through one fixed drill.
pub fn run_drill(troop: &Troop, drill: Drill, seed: u64) -> (crate::env::EpisodeInfo, DrillEnv) {
    let mut env = DrillEnv::fixed(troop.spec.clone(), drill, seed);
    let start = env.world.clone();
    let mut cache = Cache::default();
    loop {
        let a = greedy_action(&troop.actor, &env.obs(), &env.mask(), &mut cache);
        if let (_, Some(info)) = env.step(a) {
            // `step` resets on finish; hand back the starting world for replays.
            env.world = start;
            return (info, env);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CupResult {
    pub wins: u32,
    pub played: u32,
    pub destruction: f32,
    pub survived: u32,
    pub score: f32,
}

#[derive(Clone, Debug)]
pub struct TournamentEntry {
    pub troop_index: usize,
    pub solo: CupResult,
    pub squad: CupResult,
    pub total: f32,
}

pub const CUP_ROUNDS: u64 = 12;
pub const CUP_SEED: u64 = 9000;

/// Proving-grounds tournament: every troop runs the same seeded solo and
/// squad raids; points for destruction, wins and survival.
pub fn tournament(troops: &[Troop]) -> Vec<TournamentEntry> {
    let mut out: Vec<TournamentEntry> = troops
        .iter()
        .enumerate()
        .map(|(k, t)| {
            let mut cups = [CupResult::default(), CupResult::default()];
            for (ci, drill) in [Drill::Solo, Drill::Squad].into_iter().enumerate() {
                let c = &mut cups[ci];
                for r in 0..CUP_ROUNDS {
                    let (info, _) = run_drill(t, drill, CUP_SEED + r);
                    c.played += 1;
                    c.wins += info.won as u32;
                    c.survived += (!info.died) as u32;
                    c.destruction += info.destruction;
                    c.score += info.destruction * 100.0
                        + if info.won { 50.0 } else { 0.0 }
                        + if info.died { 0.0 } else { 20.0 };
                }
                c.destruction /= CUP_ROUNDS as f32;
                c.score /= CUP_ROUNDS as f32;
            }
            let [solo, squad] = cups;
            TournamentEntry {
                troop_index: k,
                total: solo.score + squad.score,
                solo,
                squad,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.total
            .partial_cmp(&a.total)
            .unwrap()
            .then(a.troop_index.cmp(&b.troop_index))
    });
    out
}

/// Run trained defenders greedily through one fixed defence drill.
pub fn run_defense_drill(troop: &Troop, seed: u64) -> (crate::env::EpisodeInfo, World) {
    let mut env = DefenseEnv::fixed(troop.spec.clone(), seed);
    let start = env.world.clone();
    let mut cache = Cache::default();
    let n = env.slots();
    loop {
        let acts: Vec<u8> = (0..n)
            .map(|k| {
                if env.active(k) {
                    greedy_action(&troop.actor, &env.obs(k), &env.mask(k), &mut cache)
                } else {
                    0
                }
            })
            .collect();
        if let Some(info) = env.act(&acts).episode {
            return (info, start);
        }
    }
}
