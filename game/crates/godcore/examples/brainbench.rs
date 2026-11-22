use std::time::Instant;
use godcore::classes::BaseClass;
use godcore::nn::{Cache, Mlp};
use godcore::ppo::{actor_dims_h, critic_dims_h, greedy_action, PpoConfig, Trainer, set_accelerator};
use godcore::rng::Rng;
use godcore::sim::{N_ACTIONS, OBS_DIM};
use godcore::troop::Troop;

fn main() {
    set_accelerator(None);
    let sizes: Vec<usize> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    for h in sizes {
        let mut rng = Rng::new(7);
        let actor = Mlp::new(&actor_dims_h(h), 0.01, &mut rng);
        let critic = Mlp::new(&critic_dims_h(h), 1.0, &mut rng);
        let json = serde_json::to_string(&actor).unwrap();
        // inference
        let obs: Vec<f32> = (0..OBS_DIM).map(|i| ((i * 37 % 11) as f32) / 11.0 - 0.5).collect();
        let mask = [true; N_ACTIONS];
        let mut cache = Cache::default();
        let n = (2_000_000_00 / (h * h + h * OBS_DIM)).max(50);
        let t0 = Instant::now();
        let mut s = 0u32;
        for _ in 0..n { s += greedy_action(&actor, &obs, &mask, &mut cache) as u32; }
        let us = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
        // training
        let mut troop = Troop::new(1, "B".into(), BaseClass::Fighter.spec(), 1);
        troop.level = 5; troop.sync_moves();
        let mut tr = Trainer::new(&troop.spec, actor.clone(), critic.clone(), 1, PpoConfig::default());
        let t1 = Instant::now();
        tr.iterate();
        let secs = t1.elapsed().as_secs_f64();
        let sps = tr.steps_done as f64 / secs;
        println!("h={h:5} params={:9} json={:6.2}MB forward={:8.1}us train={:8.0} steps/s ({} steps {:.1}s) {}",
            actor.params.len(), json.len() as f64 / 1e6, us, sps, tr.steps_done, secs, s % 2);
    }
}
