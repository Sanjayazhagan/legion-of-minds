//! Senses and human-like limits (sim version 6): trained troops see in a
//! cone, buildings block the view, they hear fights and remember enemies for
//! a moment, need time to react, turn to face what they hit, and miss shots.

use godcore::classes::BaseClass;
use godcore::sim::*;

/// Where the first enemy slot's "present" flag sits in the view.
const ENEMY0: usize = 22;
/// The senses part at the end of the view.
const SENSES: usize = OBS_SENSES;

fn step(w: &mut World, i: usize, a: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    w.step(&acts);
}

/// A trained archer at (10, 10) looking east, and a defending troop that
/// just stands there.
fn setup(ex: f32, ey: f32) -> (World, usize, usize) {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Archer.spec(), 10.0, 10.0, Control::External, 0);
    let foe = w.add_troop_on(Team::Defend, &BaseClass::Guard.spec(), ex, ey, Control::Defender, 1);
    w.add_core(28.0, 28.0);
    w.units[me].facing = (1.0, 0.0);
    (w, me, foe)
}

#[test]
fn troops_see_ahead_not_behind() {
    let (w, me, foe) = setup(14.0, 10.0);
    assert!(w.sees(me, foe), "in front");
    let (w, me, foe) = setup(6.0, 10.0);
    assert!(!w.sees(me, foe), "behind");
    assert_eq!(w.observe(me)[ENEMY0], 0.0, "not in its view");
    let (w, me, foe) = setup(8.0, 10.0);
    assert!(w.sees(me, foe), "close enough to notice from behind");
    let (w, me, foe) = setup(10.0 + SIGHT_RANGE + 1.0, 10.0);
    assert!(!w.sees(me, foe), "too far");
}

#[test]
fn buildings_block_the_view() {
    let (mut w, me, foe) = setup(18.0, 10.0);
    assert!(w.sees(me, foe));
    w.add_tower(14.0, 10.0);
    assert!(!w.sees(me, foe), "the tower is in the way");
}

#[test]
fn a_new_enemy_takes_a_moment_to_react_to() {
    let (mut w, me, foe) = setup(6.5, 10.0);
    step(&mut w, me, ACT_IDLE);
    assert!(!w.mask(me)[ACT_HIT_UNIT as usize], "hasn't seen it");
    w.units[me].facing = (-1.0, 0.0);
    step(&mut w, me, ACT_IDLE);
    let mut waited = 0;
    while !w.mask(me)[ACT_HIT_UNIT as usize] {
        step(&mut w, me, ACT_IDLE);
        waited += 1;
        assert!(waited < 20);
    }
    assert!(waited >= REACT as usize - 1, "reacted after {waited} ticks");
    assert!(w.units[foe].alive);
}

#[test]
fn troops_turn_to_face_what_they_hit() {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Fighter.spec(), 10.0, 10.0, Control::External, 0);
    let foe = w.add_troop_on(Team::Defend, &BaseClass::Guard.spec(), 9.0, 10.0, Control::Defender, 1);
    w.add_core(28.0, 28.0);
    w.units[me].facing = (1.0, 0.0);
    assert!(w.mask(me)[ACT_HIT_UNIT as usize], "felt it right behind");
    let hp = w.units[foe].hp;
    let mut turns = 0;
    while w.units[foe].hp == hp {
        step(&mut w, me, ACT_HIT_UNIT);
        turns += 1;
        assert!(turns < 12);
    }
    assert!(turns >= 4, "a half turn takes a few ticks, took {turns}");
    assert!(w.units[me].facing.0 < -0.7);
}

#[test]
fn lost_enemies_are_remembered_for_a_while() {
    let (mut w, me, foe) = setup(16.0, 10.0);
    step(&mut w, me, ACT_IDLE);
    let o = w.observe(me);
    assert_eq!(o[ENEMY0], 1.0);
    assert_eq!(o[SENSES + 4], 1.0, "in sight now");
    w.units[me].facing = (-1.0, 0.0);
    w.units[foe].x = 18.0; // moves while it isn't looked at
    step(&mut w, me, ACT_IDLE);
    step(&mut w, me, ACT_IDLE);
    let o = w.observe(me);
    assert_eq!(o[ENEMY0], 1.0, "still remembered");
    assert!(o[SENSES + 4] < 1.0 && o[SENSES + 4] > 0.9);
    assert!((o[ENEMY0 - 4] - 6.0 / 16.0).abs() < 1e-5, "where it was last seen, not where it is");
    for _ in 0..MEMORY {
        step(&mut w, me, ACT_IDLE);
    }
    assert_eq!(w.observe(me)[ENEMY0], 0.0, "forgotten");
}

#[test]
fn fights_can_be_heard_and_idle_troops_look_at_them() {
    let mut w = World::new(900);
    let me = w.add_troop(&BaseClass::Archer.spec(), 10.0, 10.0, Control::External, 0);
    let mate = w.add_troop(&BaseClass::Fighter.spec(), 10.0, 18.0, Control::Scripted, 1);
    w.add_guard(10.0, 19.0);
    w.add_core(28.0, 28.0);
    w.units[me].facing = (1.0, 0.0);
    let _ = mate;
    let mut heard = false;
    for _ in 0..10 {
        step(&mut w, me, ACT_IDLE);
        let o = w.observe(me);
        if o[SENSES + 9] > 0.0 {
            heard = true;
            assert!(o[SENSES + 8] > 0.5, "from the south (+y)");
        }
    }
    assert!(heard);
    assert!(w.units[me].facing.1 > 0.9, "turned to look");
}

#[test]
fn ranged_troops_sometimes_miss() {
    let (mut w, me, foe) = setup(15.4, 10.0);
    w.units[foe].hp = 1e9;
    w.units[foe].max_hp = 1e9;
    for _ in 0..600 {
        step(&mut w, me, ACT_HIT_UNIT);
    }
    let s = w.units[me].stats;
    let shots = s.missed + (s.unit_dmg / w.units[me].dmg).round() as u32;
    assert!(shots > 50);
    let rate = s.missed as f32 / shots as f32;
    assert!(rate > 0.12 && rate < 0.4, "missed {rate}");
}

#[test]
fn calls_reach_nearby_teammates_after_a_moment() {
    let mut w = World::new(900);
    let a = w.add_troop(&BaseClass::Fighter.spec(), 10.0, 10.0, Control::External, 0);
    let b = w.add_troop(&BaseClass::Archer.spec(), 12.0, 10.0, Control::External, 1);
    let far = w.add_troop(&BaseClass::Healer.spec(), 10.0 + CALL_RANGE + 6.0, 10.0, Control::External, 2);
    let foe = w.add_troop_on(Team::Defend, &BaseClass::Guard.spec(), 15.0, 10.0, Control::Defender, 3);
    w.add_core(28.0, 28.0);
    w.units[a].facing = (1.0, 0.0);
    // Alone, a troop has nobody to call.
    let mut solo = World::new(900);
    let s = solo.add_troop(&BaseClass::Fighter.spec(), 10.0, 10.0, Control::External, 0);
    solo.add_core(28.0, 28.0);
    assert!(!solo.mask(s)[ACT_CALL_FOCUS as usize]);
    assert!(w.mask(a)[ACT_CALL_FOCUS as usize]);
    w.step(&[ACT_IDLE; 5]);
    let mut acts = vec![ACT_IDLE; 5];
    acts[a] = ACT_CALL_FOCUS;
    let x0 = w.units[a].x;
    w.units[a].last_action = 1; // was walking east
    w.step(&acts);
    assert_eq!(w.units[a].stats.calls, 1);
    assert!(w.units[a].x > x0, "kept walking while it called");
    assert!(!w.mask(a)[ACT_CALL_FOCUS as usize], "has to wait before calling again");
    assert_eq!(w.observe(b)[OBS_CALLS + 2], 0.0, "not taken in yet");
    for _ in 0..REACT {
        w.step(&[ACT_IDLE; 5]);
    }
    let o = w.observe(b);
    assert!(o[OBS_CALLS + 2] > 0.5, "heard");
    let (fx, _) = (w.units[foe].x, w.units[foe].y);
    assert!(((fx - w.units[b].x) / CALL_RANGE - o[OBS_CALLS]).abs() < 1e-5, "points at the enemy");
    assert_eq!(w.observe(far)[OBS_CALLS + 2], 0.0, "too far to hear");
    assert_eq!(w.observe(a)[OBS_CALLS + 2], 0.0, "not its own call");
    for _ in 0..CALL_LIFE {
        w.step(&[ACT_IDLE; 5]);
    }
    assert_eq!(w.observe(b)[OBS_CALLS + 2], 0.0, "faded");
}

#[test]
fn new_calls_point_where_they_should() {
    let mut w = World::new(900);
    let a = w.add_troop(&BaseClass::Fighter.spec(), 10.0, 10.0, Control::External, 0);
    let _b = w.add_troop(&BaseClass::Archer.spec(), 11.0, 12.0, Control::External, 1);
    let tower = w.add_tower(16.0, 10.0);
    w.add_core(28.0, 28.0);
    w.units[a].facing = (1.0, 0.0);
    w.step(&[ACT_IDLE; 4]);
    let shout = |w: &mut World, act: u8| {
        let mut acts = vec![ACT_IDLE; 4];
        acts[a] = act;
        w.step(&acts);
        let e = w.events.iter().find(|e| matches!(e.kind, EventKind::Call(_))).expect("called").clone();
        for _ in 0..CALL_GAP {
            w.step(&[ACT_IDLE; 4]);
        }
        e
    };
    let e = shout(&mut w, ACT_CALL_TOWER);
    assert_eq!(e.to, (w.units[tower].x, w.units[tower].y), "points at the tower");
    let e = shout(&mut w, ACT_CALL_GO);
    assert!(e.to.0 > e.from.0 + 5.0, "points ahead of the caller");
    let e = shout(&mut w, ACT_CALL_WAIT);
    assert_eq!(e.to, e.from, "points at the caller");
}
