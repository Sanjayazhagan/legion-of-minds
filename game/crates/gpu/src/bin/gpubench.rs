//! Trains the same troop on the CPU and on the GPU and compares speed and
//! results: `cargo run --release -p godgpu --bin gpubench -- [neurons] [steps]`.

use std::time::Instant;

use godcore::classes::BaseClass;
use godcore::ppo::{set_accelerator, PpoConfig, Trainer};
use godcore::troop::Troop;

fn run(h: usize, steps: u64, label: &str) {
    let mut troop = Troop::new(1, "Bench".into(), BaseClass::Fighter.spec(), 1);
    troop.level = 5;
    troop.sync_moves();
    assert!(troop.set_brain(h), "brain must be one of {:?}", godcore::ppo::HIDDEN_SIZES);
    let mut tr = Trainer::new(&troop.spec, troop.actor.clone(), troop.critic.clone(), 1, PpoConfig::default());
    let t0 = Instant::now();
    let mut wins = Vec::new();
    while tr.steps_done < steps {
        let rep = tr.iterate();
        wins.extend(rep.episodes.iter().map(|e| e.won as u32 as f32));
    }
    let last: Vec<f32> = wins.iter().rev().take(200).copied().collect();
    let win = last.iter().sum::<f32>() / last.len().max(1) as f32;
    println!(
        "{label:<40} {h:>3} neurons  {:>7} steps  {:6.1}s  wins (last 200 games) {:4.0}%",
        tr.steps_done,
        t0.elapsed().as_secs_f32(),
        win * 100.0
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let h: usize = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(256);
    let steps: u64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(98_304);
    set_accelerator(None);
    run(h, steps, "CPU");
    match godgpu::enable() {
        Some(name) => run(h, steps, &format!("GPU: {name}")),
        None => println!("no usable GPU found (GODGPU_ALLOW_CPU=1 allows software drivers)"),
    }
}
