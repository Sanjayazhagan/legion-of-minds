//! Human limits (sim version 10): stamina, wear, pain and class bodies that
//! training conditions.

use godcore::body::{conditioning_of, Body, COST_DODGE};
use godcore::classes::BaseClass;
use godcore::sim::*;
use godcore::troop::Troop;

fn step(w: &mut World, i: usize, a: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    w.step(&acts);
}

/// A trained troop of `class` at (10, 10) looking east, with a skeleton
/// guard right in front of it.
fn duel(class: BaseClass) -> (World, usize, usize) {
    let mut w = World::new(900);
    let me = w.add_troop(&class.spec(), 10.0, 10.0, Control::External, 0);
    let foe = w.add_guard(11.0, 10.0);
    w.add_core(28.0, 28.0);
    w.units[me].facing = (1.0, 0.0);
    w.units[foe].hp = 1e6;
    w.units[foe].max_hp = 1e6;
    w.units[foe].dmg = 0.0;
    (w, me, foe)
}

#[test]
fn troops_start_fresh_and_guards_never_tire() {
    let (w, me, foe) = duel(BaseClass::Fighter);
    assert_eq!(w.units[me].stamina, 100.0);
    assert!(w.human(me));
    assert!(!w.human(foe), "skeleton guards don't tire");
    // Scripted fighters don't either.
    let mut w2 = World::new(10);
    let s = w2.add_troop(&BaseClass::Fighter.spec(), 5.0, 5.0, Control::Scripted, 0);
    assert!(!w2.human(s));
}

#[test]
fn fighting_tires_and_resting_recovers() {
    let (mut w, me, _) = duel(BaseClass::Fighter);
    for _ in 0..200 {
        let a = if w.mask(me)[ACT_HIT_UNIT as usize] { ACT_HIT_UNIT } else { ACT_IDLE };
        step(&mut w, me, a);
    }
    let after_fight = w.units[me].stamina;
    assert!(after_fight < 60.0, "200 ticks of swinging should tire a fighter: {after_fight}");
    assert!(w.units[me].stats.tired > 0);
    for _ in 0..60 {
        step(&mut w, me, ACT_IDLE);
    }
    assert!(w.units[me].stamina > after_fight + 40.0, "a breather brings stamina back");
}

#[test]
fn a_tired_troop_is_slower_and_attacks_less_often() {
    let (mut fresh, me, _) = duel(BaseClass::Fighter);
    let mut tired = fresh.clone();
    tired.units[me].stamina = 0.0;
    step(&mut fresh, me, 1);
    step(&mut tired, me, 1);
    let (df, dt) = (fresh.units[me].x - 10.0, tired.units[me].x - 10.0);
    assert!((dt / df - 0.7).abs() < 0.02, "empty walks at 70% speed: {dt} vs {df}");

    let (mut fresh, me, _) = duel(BaseClass::Fighter);
    let mut tired = fresh.clone();
    tired.units[me].stamina = 0.0;
    step(&mut fresh, me, ACT_HIT_UNIT);
    step(&mut tired, me, ACT_HIT_UNIT);
    assert!(tired.units[me].cd > fresh.units[me].cd, "longer to swing again");
    assert!(tired.units[me].stats.unit_dmg < fresh.units[me].stats.unit_dmg, "and hits softer");
}

#[test]
fn out_of_breath_means_no_dodging() {
    let (mut w, me, _) = duel(BaseClass::Assassin);
    step(&mut w, me, ACT_IDLE);
    assert!(w.mask(me)[ACT_DODGE as usize]);
    w.units[me].stamina = COST_DODGE - 1.0;
    assert!(!w.mask(me)[ACT_DODGE as usize]);
}

#[test]
fn wear_lowers_the_ceiling_for_the_rest_of_the_battle() {
    let (mut w, me, _) = duel(BaseClass::Fighter);
    for _ in 0..300 {
        let a = if w.mask(me)[ACT_HIT_UNIT as usize] { ACT_HIT_UNIT } else { ACT_IDLE };
        step(&mut w, me, a);
    }
    for _ in 0..300 {
        step(&mut w, me, ACT_IDLE);
    }
    let u = &w.units[me];
    assert!(u.max_stamina < 100.0 && u.max_stamina >= 50.0);
    assert_eq!(u.stamina, u.max_stamina, "rested back up to the worn ceiling");
}

#[test]
fn pain_slows_a_badly_hurt_troop() {
    let (mut ok, me, _) = duel(BaseClass::Fighter);
    let mut hurt = ok.clone();
    hurt.units[me].hp = 10.0;
    step(&mut ok, me, 5);
    step(&mut hurt, me, 5);
    assert!(10.0 - hurt.units[me].x < 10.0 - ok.units[me].x);
}

#[test]
fn heavy_armour_costs_more_to_walk() {
    let (mut p, a, _) = duel(BaseClass::Paladin);
    let (mut s, b, _) = duel(BaseClass::Assassin);
    for _ in 0..20 {
        step(&mut p, a, 5);
        step(&mut s, b, 5);
    }
    // Walking with recovery on top: the Paladin loses ground, the Assassin doesn't.
    assert!(p.units[a].stamina < 120.0);
    assert_eq!(s.units[b].stamina, 90.0);
}

#[test]
fn training_conditions_the_body() {
    let mut t = Troop::new(1, "t".into(), BaseClass::Archer.spec(), 3);
    let raw = Body::of(&t.spec);
    t.add_training(1_200_000);
    assert!((t.spec.conditioning - conditioning_of(1_200_000)).abs() < 1e-6);
    let fit = Body::of(&t.spec);
    assert!(fit.stamina > raw.stamina && fit.recovery > raw.recovery && fit.wear < raw.wear);
    // Saved and loaded, the conditioning comes from the training record.
    let mut j = t.clone();
    j.spec.conditioning = 1.0;
    let back = Troop::load_json(&j.save_json()).unwrap();
    assert_eq!(back.spec.conditioning, t.spec.conditioning);
}

#[test]
fn classes_react_at_their_own_speed() {
    let w = |c: BaseClass| {
        let mut w = World::new(10);
        let i = w.add_troop(&c.spec(), 5.0, 5.0, Control::External, 0);
        w.units[i].react
    };
    assert!(w(BaseClass::Assassin) < w(BaseClass::Fighter));
    assert!(w(BaseClass::Paladin) > w(BaseClass::Fighter));
}

#[test]
fn brains_see_their_own_breath() {
    let (mut w, me, _) = duel(BaseClass::Fighter);
    step(&mut w, me, ACT_IDLE);
    assert_eq!(w.observe(me)[OBS_BODY], 1.0);
    w.units[me].stamina = 25.0;
    assert_eq!(w.observe(me)[OBS_BODY], 0.25);
}

// ---- Stage A: injuries, supplies, traits and weapon weight.

#[test]
fn hits_hurt_legs_and_arms_and_big_ones_bleed() {
    let (mut w, me, foe) = duel(BaseClass::Fighter);
    w.units[foe].dmg = 4.0;
    for _ in 0..400 {
        step(&mut w, me, ACT_IDLE);
    }
    let u = &w.units[me];
    assert!(u.legs > 0.0 && u.arms > 0.0, "some hits land on each: {} {}", u.legs, u.arms);
    let (mut w, me, _) = duel(BaseClass::Fighter);
    w.units[me].legs = godcore::body::INJURY_FULL;
    let mut ok = w.clone();
    ok.units[me].legs = 0.0;
    step(&mut w, me, 1);
    step(&mut ok, me, 1);
    assert!(w.units[me].x - 10.0 < 0.85 * (ok.units[me].x - 10.0), "ruined legs limp");
}

#[test]
fn bleeding_drains_until_healed() {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Fighter.spec(), 10.0, 10.0, Control::External, 0);
    let doc = w.add_troop(&BaseClass::Healer.spec(), 11.0, 10.0, Control::External, 1);
    w.add_core(28.0, 28.0);
    w.units[me].bleed = 0.3;
    w.units[me].hp = 80.0;
    let mut acts = vec![ACT_IDLE; w.units.len()];
    w.step(&acts);
    assert!(w.units[me].hp < 80.0);
    assert!(w.mask(doc)[ACT_HEAL as usize]);
    acts[doc] = ACT_HEAL;
    w.step(&acts);
    assert_eq!(w.units[me].bleed, 0.0, "a heal stops the bleeding");
}

#[test]
fn archers_run_out_of_arrows_and_fight_in_close() {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Archer.spec(), 10.0, 10.0, Control::External, 0);
    w.add_core(28.0, 28.0);
    let u = &w.units[me];
    assert_eq!(u.arrows, 90.0);
    assert!(u.range > 2.5);
    w.units[me].arrows = 0.0;
    step(&mut w, me, ACT_IDLE);
    assert_eq!(w.units[me].range, godcore::body::MAKESHIFT_RANGE);
    assert!(w.units[me].dmg < w.units[me].full_dmg);
    assert_eq!(w.units[me].supply(), 0.0);
}

#[test]
fn mages_spend_mana_and_get_it_back() {
    let (mut w, me, _) = duel(BaseClass::Mage);
    assert_eq!(w.units[me].mana, 100.0);
    w.units[me].mana = 1.0;
    step(&mut w, me, ACT_IDLE);
    assert_eq!(w.units[me].splash, 0.0, "no mana, no spell");
    for _ in 0..40 {
        step(&mut w, me, ACT_IDLE);
    }
    assert!(w.units[me].splash > 0.0, "mana came back");
}

#[test]
fn recruits_are_born_different_but_balanced() {
    use godcore::body::Traits;
    let a = Troop::new(1, "a".into(), BaseClass::Fighter.spec(), 101);
    let b = Troop::new(2, "b".into(), BaseClass::Fighter.spec(), 202);
    assert_ne!(a.spec.traits, b.spec.traits);
    for class in [BaseClass::Fighter, BaseClass::Archer, BaseClass::Mage, BaseClass::Healer, BaseClass::Hunter] {
        let spec = class.spec();
        for seed in 0..1000 {
            let t = Traits::roll_for(seed, &spec);
            let worth = t.worth(&spec);
            assert!(worth.abs() < 0.01, "{class:?} seed {seed}: worth {worth}");
            assert!(t.scores().iter().all(|s| s.abs() <= 1.0 + 1e-5));
        }
    }
    // Old saves (no traits) stay average, and a client can't pick traits.
    let mut old = a.clone();
    old.born = false;
    let back = Troop::load_json(&old.save_json()).unwrap();
    assert_eq!(back.spec.traits, Traits::default());
    let mut forged = a.clone();
    forged.spec.traits.strength = 5.0;
    assert_eq!(Troop::load_json(&forged.save_json()).unwrap().spec.traits, a.spec.traits);
}

#[test]
fn heavy_weapons_cost_more_to_swing() {
    let (mut axe, a, _) = duel(BaseClass::Fighter);
    let (mut dag, b, _) = duel(BaseClass::Assassin);
    let (s0, t0) = (axe.units[a].stamina, dag.units[b].stamina);
    step(&mut axe, a, ACT_HIT_UNIT);
    step(&mut dag, b, ACT_HIT_UNIT);
    let (ds_axe, ds_dag) = (s0 - axe.units[a].stamina, t0 - dag.units[b].stamina);
    assert!(ds_axe > ds_dag + 1.0, "greataxe {ds_axe} vs daggers {ds_dag}");
}
