//! Dodge, block, weapon skills, weapons and growing brains.

use godcore::classes::BaseClass;
use godcore::nn::{Cache, Mlp};
use godcore::ppo::{actor_dims, hidden_for_level};
use godcore::rng::Rng;
use godcore::sim::*;
use godcore::troop::Troop;
use godcore::moves::Move;
use godcore::weapons::{mastery_level, Weapon};

fn duel(attacker: BaseClass) -> (World, usize, usize) {
    let mut w = World::new(900);
    let me = w.add_troop(&attacker.spec(), 10.0, 10.0, Control::External, 0);
    let guard = w.add_guard(11.0, 10.0);
    w.add_core(25.0, 25.0);
    (w, me, guard)
}

fn act(w: &mut World, i: usize, a: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    w.step(&acts);
}

#[test]
fn dodging_rolls_away_and_avoids_hits() {
    let (w, me, _) = duel(BaseClass::Fighter);
    assert!(!w.mask(me)[ACT_DODGE as usize], "fighters can't dodge");
    let (mut w, me, guard) = duel(BaseClass::Assassin);
    assert!(w.mask(me)[ACT_DODGE as usize]);
    let before = w.units[me].dist(&w.units[guard]);
    act(&mut w, me, ACT_DODGE);
    assert!(w.units[me].dist(&w.units[guard]) > before + 2.0);
    assert!(w.units[me].invuln > 0);
    assert!(!w.mask(me)[ACT_DODGE as usize], "dodge is on cooldown");
}

#[test]
fn blocking_soaks_damage() {
    let (mut w, me, _) = duel(BaseClass::Fighter);
    act(&mut w, me, ACT_BLOCK);
    for _ in 0..10 {
        act(&mut w, me, ACT_IDLE);
    }
    let s = w.units[me].stats;
    assert!(s.blocked > 0.0, "guard hits were blocked");
    assert!(!w.mask(me)[ACT_HIT_UNIT as usize] || w.units[me].block_ticks == 0);
}

#[test]
fn shield_bash_stuns() {
    let mut w = World::new(900);
    let mut spec = BaseClass::Fighter.spec();
    spec.weapon = Some(Weapon::SwordShield);
    let me = w.add_troop(&spec, 10.0, 10.0, Control::External, 0);
    let guard = w.add_guard(11.0, 10.0);
    w.add_core(25.0, 25.0);
    assert!(w.mask(me)[ACT_SKILL as usize]);
    act(&mut w, me, ACT_SKILL);
    assert!(w.units[guard].stun > 0);
    assert!(w.units[guard].hp < GUARD_HP);
    assert!(!w.mask(me)[ACT_SKILL as usize], "skill is on cooldown");
}

#[test]
fn every_weapon_skill_does_something() {
    for weapon in Weapon::ALL {
        let mut w = World::new(900);
        let mut spec = BaseClass::Fighter.spec();
        spec.weapon = Some(weapon);
        let me = w.add_troop(&spec, 10.0, 10.0, Control::External, 0);
        let ally = w.add_troop(&BaseClass::Fighter.spec(), 9.0, 10.0, Control::External, 1);
        w.units[ally].hp -= 30.0;
        let g = w.add_guard(11.0, 10.0);
        w.add_guard(10.5, 11.0);
        w.add_core(25.0, 25.0);
        assert!(w.mask(me)[ACT_SKILL as usize], "{weapon:?} skill ready");
        let (hp_g, hp_a) = (w.units[g].hp, w.units[ally].hp);
        act(&mut w, me, ACT_SKILL);
        assert_eq!(w.units[me].stats.skills, 1);
        assert!(w.units[g].hp < hp_g || w.units[ally].hp > hp_a, "{weapon:?} hit or healed");
    }
}

#[test]
fn a_class_with_its_own_weapon_keeps_its_stats() {
    for c in BaseClass::ALL.iter().chain(BaseClass::DEFENDERS.iter()) {
        let spec = c.spec();
        let mut w = World::new(10);
        let i = w.add_troop(&spec, 5.0, 5.0, Control::External, 0);
        let u = &w.units[i];
        assert_eq!((u.dmg, u.cd_max, u.range, u.speed), (spec.dmg, spec.cooldown, spec.range, spec.speed), "{c:?}");
    }
    // An archer with daggers stabs up close.
    let mut spec = BaseClass::Archer.spec();
    spec.weapon = Some(Weapon::Daggers);
    let mut w = World::new(10);
    let i = w.add_troop(&spec, 5.0, 5.0, Control::External, 0);
    assert!(w.units[i].range < 2.0);
    // Mastery adds damage.
    spec.mastery = 4;
    let j = w.add_troop(&spec, 6.0, 5.0, Control::External, 0);
    assert!(w.units[j].dmg > w.units[i].dmg);
}

#[test]
fn old_saves_grow_new_inputs_and_moves() {
    let mut t = Troop::new(1, "old".into(), BaseClass::Fighter.spec(), 5);
    let mut rng = Rng::new(3);
    t.actor = Mlp::new(&[OBS_DIM_V1, 64, 64, N_ACTIONS_V1], 1.0, &mut rng);
    t.critic = Mlp::new(&[OBS_DIM_V1, 64, 64, 1], 1.0, &mut rng);
    t.steps = 200_000;
    t.level = 2;
    let old = t.actor.clone();
    let json = t.save_json().replace(",\"mastery\":{}", "");
    let loaded = Troop::load_json(&json).unwrap();
    assert_eq!(loaded.actor.dims, actor_dims(2));
    assert_eq!(loaded.mastery_steps(Weapon::Greataxe), 200_000);
    assert_eq!(loaded.spec.mastery, mastery_level(200_000));
    // Same scores for the old moves on the old inputs.
    let x: Vec<f32> = (0..OBS_DIM_V1).map(|k| (k as f32 * 0.3).sin()).collect();
    let mut xb = x.clone();
    xb.resize(OBS_DIM, 0.5);
    let (mut c1, mut c2) = (Cache::default(), Cache::default());
    old.forward(&x, &mut c1);
    loaded.actor.forward(&xb, &mut c2);
    for a in 0..N_ACTIONS_V1 {
        assert!((old.output(&c1)[a] - loaded.actor.output(&c2)[a]).abs() < 1e-5);
    }
}

#[test]
fn levelling_up_grows_the_brain() {
    let mut t = Troop::new(1, "t".into(), BaseClass::Mage.spec(), 5);
    t.brain = None; // a troop from before players picked brain sizes
    assert_eq!(t.actor.dims[1], hidden_for_level(1));
    let small = t.brain_params();
    while t.level < 5 {
        t.level_up();
    }
    assert_eq!(t.actor.dims, actor_dims(5));
    assert!(t.brain_params() > 3 * small);
}

#[test]
fn mastery_follows_the_equipped_weapon() {
    let mut t = Troop::new(1, "t".into(), BaseClass::Fighter.spec(), 5);
    assert_eq!(t.add_training(60_000), Some(1));
    t.equip(Weapon::Crossbow);
    assert_eq!(t.spec.mastery, 0);
    t.add_training(10_000);
    t.equip(Weapon::Greataxe);
    assert_eq!(t.spec.mastery, 1);
    assert_eq!(t.steps, 70_000);
}

fn far_duel(attacker: BaseClass, d: f32) -> (World, usize, usize) {
    let mut w = World::new(900);
    let me = w.add_troop(&attacker.spec(), 10.0, 10.0, Control::External, 0);
    let guard = w.add_guard(10.0 + d, 10.0);
    w.add_core(25.0, 25.0);
    (w, me, guard)
}

#[test]
fn charge_closes_the_gap_and_staggers() {
    let (mut w, me, guard) = far_duel(BaseClass::Fighter, 4.0);
    assert!(w.mask(me)[ACT_CHARGE as usize]);
    act(&mut w, me, ACT_CHARGE);
    assert!(w.units[me].dist(&w.units[guard]) < 1.5);
    assert!(w.units[guard].hp < GUARD_HP);
    assert!(w.units[guard].stun > 0);
}

#[test]
fn cloaked_troops_cant_be_targeted() {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Assassin.spec(), 10.0, 10.0, Control::External, 0);
    w.add_tower(11.9, 10.0);
    w.add_core(25.0, 25.0);
    act(&mut w, me, ACT_CLOAK);
    let hp = w.units[me].hp;
    for _ in 0..20 {
        act(&mut w, me, ACT_IDLE);
    }
    assert_eq!(w.units[me].hp, hp, "the tower can't see it");
    act(&mut w, me, ACT_HIT_BUILDING);
    assert_eq!(w.units[me].cloak, 0, "striking breaks the cloak");
}

#[test]
fn snares_stun_whoever_steps_in() {
    let (mut w, me, guard) = far_duel(BaseClass::Archer, 1.5);
    assert!(w.mask(me)[ACT_SNARE as usize]);
    act(&mut w, me, ACT_SNARE);
    assert_eq!(w.traps.len(), 1);
    for _ in 0..30 {
        act(&mut w, me, 5); // walk west, the guard follows over the trap
        if w.units[guard].stun > 0 {
            break;
        }
    }
    assert!(w.units[guard].stun > 0, "the guard walked into it");
    assert!(w.traps.is_empty());
}

#[test]
fn wards_and_blinks() {
    let (mut w, me, _) = duel(BaseClass::Healer);
    assert!(w.mask(me)[ACT_WARD as usize]);
    act(&mut w, me, ACT_WARD);
    assert!(w.units[me].ward > 0);
    let (mut w, me, guard) = duel(BaseClass::Mage);
    act(&mut w, me, ACT_BLINK);
    assert!(w.units[me].dist(&w.units[guard]) > 3.0);
}

#[test]
fn specialists_master_their_moves_sooner_than_mixes() {
    let mut pure = Troop::new(1, "p".into(), BaseClass::Fighter.spec(), 1);
    let mix = godcore::classes::ClassSpec::blend(&[(BaseClass::Fighter, 40.0), (BaseClass::Archer, 30.0), (BaseClass::Assassin, 30.0)]);
    let mut jack = Troop::new(2, "j".into(), mix, 2);
    pure.add_training(300_000);
    jack.add_training(300_000);
    assert!(pure.move_practice(Move::Block) > 2.0 * jack.move_practice(Move::Block));
    assert!(pure.spec.move_power(Move::Block) > jack.spec.move_power(Move::Block));
    let mut w = World::new(10);
    let (a, b) = (w.add_troop(&pure.spec, 5.0, 5.0, Control::External, 0), w.add_troop(&jack.spec, 6.0, 5.0, Control::External, 1));
    assert!(w.units[a].moves[Move::Block as usize] > w.units[b].moves[Move::Block as usize]);
}

#[test]
fn players_pick_the_brain_size() {
    let mut t = Troop::new(1, "t".into(), BaseClass::Fighter.spec(), 5);
    assert_eq!(t.hidden(), 64);
    t.level_up();
    assert_eq!(t.hidden(), 64, "picked brains don't grow on their own");
    let small = t.actor.clone();
    assert!(t.set_brain(256));
    assert_eq!(t.actor.dims, godcore::ppo::actor_dims_h(256));
    assert!(!t.set_brain(128), "brains never shrink");
    assert!(!t.set_brain(200), "only listed sizes");
    assert_eq!(t.next_brain(), None);
    // Growing keeps what it learned.
    let x: Vec<f32> = (0..OBS_DIM).map(|k| (k as f32 * 0.7).cos()).collect();
    let (mut c1, mut c2) = (Cache::default(), Cache::default());
    small.forward(&x, &mut c1);
    t.actor.forward(&x, &mut c2);
    for a in 0..N_ACTIONS {
        assert!((small.output(&c1)[a] - t.actor.output(&c2)[a]).abs() < 1e-4);
    }
    // The pick survives a save.
    let back = Troop::load_json(&t.save_json()).unwrap();
    assert_eq!(back.hidden(), 256);
}
