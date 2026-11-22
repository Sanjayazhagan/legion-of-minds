//! Human moves (sim version 11): unlocked by brain size, and each one does
//! what it says.

use godcore::classes::{BaseClass, ClassSpec};
use godcore::human_moves::*;
use godcore::sim::*;
use godcore::troop::Troop;

fn step2(w: &mut World, i: usize, a: u8, j: usize, b: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    acts[j] = b;
    w.step(&acts);
}

fn step(w: &mut World, i: usize, a: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    w.step(&acts);
}

fn spec(class: BaseClass, tier: u8) -> ClassSpec {
    let mut s = class.spec();
    s.move_tier = tier;
    s
}

/// Two trained troops facing each other `gap` steps apart, both able to
/// make every human move.
fn duel(gap: f32) -> (World, usize, usize) {
    let mut w = World::new(900);
    let me = w.add_troop(&spec(BaseClass::Fighter, 4), 10.0, 10.0, Control::External, 0);
    let foe = w.add_troop_on(Team::Defend, &spec(BaseClass::Fighter, 4), 10.0 + gap, 10.0, Control::External, 1);
    w.units[me].facing = (1.0, 0.0);
    w.units[foe].facing = (-1.0, 0.0);
    (w, me, foe)
}

#[test]
fn brain_size_sets_the_tier_and_locked_moves_are_never_allowed() {
    let mut t = Troop::new(1, "t".into(), BaseClass::Fighter.spec(), 1);
    assert_eq!(t.spec.move_tier, 0);
    let mut w = World::new(900);
    let me = w.add_troop(&t.spec, 10.0, 10.0, Control::External, 0);
    w.add_troop_on(Team::Defend, &BaseClass::Fighter.spec(), 11.5, 10.0, Control::External, 1);
    w.units[me].facing = (1.0, 0.0);
    let m = w.mask(me);
    assert!(m[ACT_SPRINT as usize..].iter().all(|&x| !x), "a 64 brain has no human moves");
    for (h, tier) in [(128, 1), (256, 2), (512, 3), (1024, 4)] {
        // Past 256 a brain only grows at level 5.
        t.level = if h <= 256 { 1 } else { 5 };
        assert!(t.set_brain(h));
        assert_eq!(t.spec.move_tier, tier, "{h}");
    }
    // Saved and loaded, the tier comes from the brain, not the file.
    let mut j: serde_json::Value = serde_json::from_str(&t.save_json()).unwrap();
    j["spec"]["move_tier"] = 0.into();
    assert_eq!(Troop::load_json(&j.to_string()).unwrap().spec.move_tier, 4);
}

#[test]
fn each_tier_unlocks_its_moves() {
    for tier in 0..=4u8 {
        let (mut w, me, _) = duel(1.5);
        w.units[me].tier = tier;
        let m = w.mask(me);
        for &(a, name, need) in HUMAN_MOVES.iter() {
            if need > tier {
                assert!(!m[a as usize], "{name} at tier {tier}");
            }
        }
        if tier >= 1 {
            assert!(m[ACT_SPRINT as usize] && m[ACT_SNEAK as usize] && m[ACT_BACKSTEP as usize]);
        }
        if tier >= 4 {
            assert!(m[ACT_GRAB as usize] && m[ACT_ROCK as usize] && m[ACT_SHOVE as usize] && m[ACT_FEINT as usize]);
        }
    }
}

#[test]
fn sprinting_is_fast_and_tiring_sneaking_is_slow_and_silent() {
    let walk = |pace: Option<u8>| {
        let (mut w, me, _) = duel(40.0);
        if let Some(p) = pace {
            step(&mut w, me, p);
        }
        let (x0, s0) = (w.units[me].x, w.units[me].stamina);
        for _ in 0..5 {
            step(&mut w, me, 1); // east
        }
        (w.units[me].x - x0, s0 - w.units[me].stamina, w)
    };
    let (d_walk, s_walk, _) = walk(None);
    let (d_run, s_run, _) = walk(Some(ACT_SPRINT));
    let (d_sneak, _, _) = walk(Some(ACT_SNEAK));
    assert!(d_run > 1.4 * d_walk, "sprint {d_run} vs walk {d_walk}");
    assert!(s_run > 2.0 * s_walk, "sprint tires: {s_run} vs {s_walk}");
    assert!(d_sneak < 0.6 * d_walk, "sneak {d_sneak} vs walk {d_walk}");
    // The pace wears off.
    let (mut w, me, _) = duel(40.0);
    step(&mut w, me, ACT_SPRINT);
    assert_eq!(w.units[me].pace, 1);
    for _ in 0..PACE_TICKS {
        step(&mut w, me, ACT_IDLE);
    }
    assert_eq!(w.units[me].pace, 0);
}

#[test]
fn back_step_and_strafe_keep_facing_the_enemy() {
    let (mut w, me, foe) = duel(3.0);
    let x0 = w.units[me].x;
    step(&mut w, me, ACT_BACKSTEP);
    assert!(w.units[me].x < x0, "stepped back");
    assert!(w.units[me].facing.0 > 0.9, "still facing east: {:?}", w.units[me].facing);
    let y0 = w.units[me].y;
    step(&mut w, me, ACT_STRAFE_L);
    assert!((w.units[me].y - y0).abs() > 0.1, "moved sideways");
    let (dx, dy) = (w.units[foe].x - w.units[me].x, w.units[foe].y - w.units[me].y);
    let d = (dx * dx + dy * dy).sqrt();
    let f = w.units[me].facing;
    assert!((f.0 * dx + f.1 * dy) / d > 0.8, "turned to keep the enemy in front");
}

#[test]
fn a_parry_takes_no_damage_and_stuns_the_attacker() {
    let (mut w, me, foe) = duel(1.5);
    let hp = w.units[me].hp;
    step2(&mut w, me, ACT_PARRY, foe, ACT_HIT_UNIT);
    assert_eq!(w.units[me].hp, hp, "turned aside");
    assert!(w.units[foe].stun > 0, "attacker reeling");
    assert_eq!(w.units[me].stats.parried, 1);
}

#[test]
fn a_feint_catches_a_troop_that_reacts_to_it() {
    let (mut w, me, foe) = duel(1.5);
    step(&mut w, me, ACT_FEINT);
    assert!(w.units[foe].feinted > 0);
    assert!(w.mask(foe)[ACT_PARRY as usize]);
    step(&mut w, foe, ACT_PARRY);
    assert!(w.units[foe].stun > 0, "fooled into parrying nothing");
    assert_eq!(w.units[foe].parry, 0);
    assert_eq!(w.units[foe].stats.fooled, 1);
    // A troop that doesn't bite is fine.
    let (mut w, me, foe) = duel(1.5);
    step(&mut w, me, ACT_FEINT);
    step(&mut w, foe, ACT_HIT_UNIT);
    assert_eq!(w.units[foe].stun, 0);
}

#[test]
fn shove_and_grab_move_and_stun_the_enemy() {
    let (mut w, me, foe) = duel(1.5);
    w.units[foe].block_ticks = 5;
    let x0 = w.units[foe].x;
    step(&mut w, me, ACT_SHOVE);
    assert!(w.units[foe].x > x0 + 1.0, "pushed back");
    assert_eq!(w.units[foe].block_ticks, 0, "guard broken");
    assert!(w.units[foe].stun > 0);
    assert!(!w.mask(me)[ACT_SHOVE as usize], "cooldown");

    let (mut w, me, foe) = duel(1.5);
    let (x0, hp) = (w.units[foe].x, w.units[foe].hp);
    step(&mut w, me, ACT_GRAB);
    assert!(w.units[foe].x > x0 + 2.5, "thrown");
    assert!(w.units[foe].hp < hp);
    assert!(w.units[foe].stun >= GRAB_STUN - 1, "stunned: {}", w.units[foe].stun);
}

#[test]
fn a_rock_hits_at_range_and_crouching_makes_shots_miss_more() {
    let (mut w, me, foe) = duel(5.0);
    assert!(w.mask(me)[ACT_ROCK as usize]);
    assert!(!w.mask(me)[ACT_SHOVE as usize], "too far to shove");
    let hp = w.units[foe].hp;
    // Rolls are seeded: over a few throws some land.
    let mut hits = 0;
    for _ in 0..4 {
        w.units[me].human_cd = [0; N_HUMAN];
        step(&mut w, me, ACT_ROCK);
        hits += (w.units[foe].hp < hp) as u32;
    }
    assert!(hits > 0 && w.units[foe].hp < hp);
    // Crouching: smaller and slower.
    let (mut w, me, _) = duel(40.0);
    step(&mut w, me, ACT_CROUCH);
    assert!(w.units[me].crouch > 0);
    let x0 = w.units[me].x;
    step(&mut w, me, 1);
    let (mut w2, me2, _) = duel(40.0);
    let x2 = w2.units[me2].x;
    step(&mut w2, me2, 1);
    assert!(w.units[me].x - x0 < 0.6 * (w2.units[me2].x - x2));
}

#[test]
fn a_jump_clears_ground_and_dodges_for_a_moment() {
    let (mut w, me, _) = duel(40.0);
    let x0 = w.units[me].x;
    step(&mut w, me, ACT_JUMP);
    assert!((w.units[me].x - x0 - JUMP_DIST).abs() < 0.01);
    assert!(w.units[me].invuln > 0);
}
