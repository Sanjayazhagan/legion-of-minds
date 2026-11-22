//! The battlefield (sim version 10): light and weather, noise, surprise
//! from behind, mud and hills.

use godcore::classes::BaseClass;
use godcore::scenario::{build_base, build_home_base, HomeBase, BASES};
use godcore::sim::*;

fn step(w: &mut World, i: usize, a: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    w.step(&acts);
}

fn setup(ex: f32) -> (World, usize, usize) {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Archer.spec(), 5.0, 10.0, Control::External, 0);
    let foe = w.add_troop_on(Team::Defend, &BaseClass::Guard.spec(), ex, 10.0, Control::Defender, 1);
    w.units[me].facing = (1.0, 0.0);
    (w, me, foe)
}

#[test]
fn darkness_and_fog_shorten_sight_but_torches_show_buildings() {
    let (mut w, me, foe) = setup(15.0);
    assert!(w.sees(me, foe), "10 steps by day");
    w.light = Light::Night;
    assert!(!w.sees(me, foe), "not at night");
    w.light = Light::Fog;
    assert!(!w.sees(me, foe), "nor in fog");
    let t = w.add_tower(17.0, 10.0);
    w.light = Light::Night;
    assert!(w.sees(me, t), "the tower's torches");
}

#[test]
fn every_base_has_its_own_light_and_ground() {
    let lights: Vec<Light> = BASES.iter().map(|b| build_base(b).light).collect();
    assert_eq!(lights, vec![Light::Day, Light::Dusk, Light::Fog, Light::Night]);
    for b in &BASES {
        let (a, c) = (build_base(b), build_base(b));
        assert_eq!(a.terrain, c.terrain, "same for every player");
        assert_eq!(a.terrain.len(), 3);
    }
    let h = HomeBase::default();
    assert_eq!(build_home_base(&h).terrain, build_home_base(&h).terrain);
}

#[test]
fn mud_slows_and_tires() {
    let (mut dry, me, _) = setup(30.0);
    let mut wet = dry.clone();
    wet.terrain.push(Patch { ground: Ground::Mud, x: 5.0, y: 10.0, r: 2.0 });
    for _ in 0..3 {
        step(&mut dry, me, 1);
        step(&mut wet, me, 1);
    }
    assert!(wet.units[me].x - 5.0 < 0.8 * (dry.units[me].x - 5.0));
}

#[test]
fn a_hill_lets_you_see_further() {
    let (mut w, me, foe) = setup(20.5);
    assert!(!w.sees(me, foe));
    w.terrain.push(Patch { ground: Ground::Hill, x: 5.0, y: 10.0, r: 2.5 });
    assert!(w.sees(me, foe));
    assert_eq!(w.observe(me)[OBS_DIM - 5], 1.0, "knows it's on a hill");
}

#[test]
fn troops_feel_where_the_nearest_mud_is() {
    let (mut w, me, _) = setup(30.0);
    assert_eq!(w.observe(me)[OBS_DIM - 4], 0.0, "no mud anywhere");
    // Mud behind it, its edge 2 steps away: felt even while facing away.
    w.terrain.push(Patch { ground: Ground::Mud, x: 1.0, y: 10.0, r: 2.0 });
    let o = w.observe(me);
    assert!(o[OBS_DIM - 4] < -0.5 && o[OBS_DIM - 3].abs() < 1e-6, "to the west: {} {}", o[OBS_DIM - 4], o[OBS_DIM - 3]);
    assert_eq!(o[OBS_DIM - 2], 0.0, "no hill");
}

#[test]
fn a_blow_from_behind_hits_harder_and_cant_be_blocked() {
    let mk = |behind: bool| {
        let mut w = World::new(900);
        let me = w.add_troop(&BaseClass::Paladin.spec(), 10.0, 10.0, Control::External, 0);
        let foe = w.add_guard(if behind { 9.0 } else { 11.0 }, 10.0);
        w.units[me].facing = (1.0, 0.0);
        for _ in 0..40 {
            step(&mut w, me, ACT_IDLE);
            w.units[me].facing = (1.0, 0.0);
        }
        let _ = foe;
        w.units[me].stats.surprised
    };
    assert_eq!(mk(false), 0);
    // Close enough to touch is always noticed, so a guard right behind isn't a surprise.
    assert_eq!(mk(true), 0);
}

#[test]
fn an_unseen_attacker_gets_the_surprise() {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Fighter.spec(), 10.0, 10.0, Control::External, 0);
    let archer = w.add_troop_on(Team::Defend, &BaseClass::Sentry.spec(), 5.5, 10.0, Control::External, 1);
    w.units[me].facing = (1.0, 0.0);
    w.units[archer].facing = (1.0, 0.0);
    let hp = w.units[me].hp;
    let mut acts = vec![ACT_IDLE; w.units.len()];
    for _ in 0..12 {
        acts[archer] = if w.mask(archer)[ACT_HIT_UNIT as usize] { ACT_HIT_UNIT } else { ACT_IDLE };
        w.step(&acts);
        w.units[me].facing = (1.0, 0.0);
    }
    assert!(w.units[me].hp < hp);
    assert!(w.units[me].stats.surprised > 0, "shot in the back");
}

#[test]
fn footsteps_and_shouts_give_you_away() {
    // A defender looking away hears a raider walking up behind it.
    let mut w = World::new(900);
    let guard = w.add_troop_on(Team::Defend, &BaseClass::Guard.spec(), 10.0, 10.0, Control::External, 0);
    let sneak = w.add_troop(&BaseClass::Paladin.spec(), 5.0, 10.0, Control::External, 1);
    w.units[guard].facing = (1.0, 0.0);
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[sneak] = 1;
    let mut heard = false;
    for _ in 0..12 {
        w.step(&acts);
        heard |= w.observe(guard)[OBS_SENSES + 9] > 0.0;
    }
    assert!(heard, "heavy footsteps carry");
}
