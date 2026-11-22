//! How much training a troop needs to use newly unlocked human moves:
//! `cargo run --release -p godcore --example movecost -- <neurons> <before> <after>`.
//! Trains a troop `before` steps with the moves locked, then `after` more
//! steps twice from the same point: once still locked, once unlocked, and
//! prints drill wins along the way and how often it uses the new moves in
//! the four drill bases.
use godcore::battle::Battle;
use godcore::classes::BaseClass;
use godcore::human_moves::tier_for_brain;
use godcore::ppo::{PpoConfig, Trainer};
use godcore::scenario::BASES;
use godcore::sim::{Kind, ACT_SPRINT};
use godcore::troop::Troop;
use std::time::Instant;

fn train(t: &mut Troop, steps: u64, label: &str) {
    let mut tr = Trainer::new(&t.spec, t.actor.clone(), t.critic.clone(), t.seed ^ t.steps, PpoConfig::default());
    let (mut wins, mut n, mut next) = (0u32, 0u32, 100_000u64);
    let t0 = Instant::now();
    while tr.steps_done < steps {
        let rep = tr.iterate();
        for e in &rep.episodes {
            wins += e.won as u32;
            n += 1;
        }
        if tr.steps_done >= next || tr.steps_done >= steps {
            println!("  {label:<10} +{:>4}k steps: drill wins {:5.1}% ({n} drills, {:.0}s)", tr.steps_done / 1000, 100.0 * wins as f32 / n.max(1) as f32, t0.elapsed().as_secs_f32());
            (wins, n, next) = (0, 0, next + 100_000);
        }
    }
    t.actor = tr.actor.clone();
    t.critic = tr.critic.clone();
    t.steps += tr.steps_done;
}

/// Share of the troop's turns spent on human moves, and win count, in the
/// four drill bases.
fn usage(t: &Troop) -> (f32, u32) {
    let (mut used, mut turns, mut won) = (0u32, 0u32, 0u32);
    for base in &BASES {
        let mut b = Battle::new(base, vec![(t.clone(), (3.0, 3.0))]);
        while b.outcome() == godcore::sim::Outcome::Running {
            b.step();
            let u = b.world.units.iter().find(|u| u.kind == Kind::Troop && u.control == godcore::sim::Control::External).unwrap();
            if u.alive {
                turns += 1;
                used += (u.last_action >= ACT_SPRINT) as u32;
            }
        }
        won += (b.outcome() == godcore::sim::Outcome::Victory) as u32;
    }
    (used as f32 / turns.max(1) as f32, won)
}

fn main() {
    let a: Vec<u64> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let (h, before, after) = (a[0] as usize, a[1], a[2]);
    let mut t = Troop::new(1, "Bench".into(), BaseClass::Fighter.spec(), 7);
    // Level 5 brains are at least 256; smaller sizes stay at level 1.
    t.level = if h > 256 { 5 } else { 1 };
    t.sync_moves();
    assert!(t.set_brain(h));
    assert_eq!(t.hidden(), h);
    let tier = tier_for_brain(h);
    t.spec.move_tier = 0;
    println!("{h} neurons, tier {tier}: {before} steps with the moves locked");
    train(&mut t, before, "locked");
    let (mut locked, mut open) = (t.clone(), t.clone());
    open.spec.move_tier = tier;
    println!("then {after} more steps each way");
    train(&mut locked, after, "locked");
    train(&mut open, after, "unlocked");
    let (u, w) = usage(&locked);
    println!("locked:   new moves {:4.1}% of turns, {w}/4 drill bases won", u * 100.0);
    let (u, w) = usage(&open);
    println!("unlocked: new moves {:4.1}% of turns, {w}/4 drill bases won", u * 100.0);
}
