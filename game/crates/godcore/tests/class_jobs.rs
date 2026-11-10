//! Rules v9: every class has a job. Healers bless the troops around them,
//! and spellcasters pay for their blasts when a troop reaches them.

use godcore::classes::BaseClass;
use godcore::sim::*;

fn act(w: &mut World, i: usize, a: u8) {
    let mut acts = vec![ACT_IDLE; w.units.len()];
    acts[i] = a;
    w.step(&acts);
}

/// Damage a scripted guard does to `class` over 30 ticks, with or without
/// a healer of its own side standing by.
fn taken(class: BaseClass, with_healer: bool) -> f32 {
    let mut w = World::new(900);
    let me = w.add_troop(&class.spec(), 10.0, 10.0, Control::External, 0);
    w.add_guard(11.0, 10.0);
    w.add_core(25.0, 25.0);
    if with_healer {
        // Idle, so it blesses but never heals.
        w.add_troop(&BaseClass::Healer.spec(), 10.0, 12.0, Control::External, 1);
    }
    for _ in 0..30 {
        act(&mut w, me, ACT_IDLE);
    }
    w.units[me].max_hp - w.units[me].hp
}

#[test]
fn a_healer_nearby_blesses() {
    let (alone, blessed) = (taken(BaseClass::Fighter, false), taken(BaseClass::Fighter, true));
    assert!(alone > 0.0);
    assert!((blessed - alone * (1.0 - BLESSING)).abs() < 1e-3, "{alone} -> {blessed}");
}

#[test]
fn a_healer_does_not_bless_itself_or_from_afar() {
    let mut w = World::new(900);
    let healer = w.add_troop(&BaseClass::Healer.spec(), 10.0, 10.0, Control::External, 0);
    let far = w.add_troop(&BaseClass::Fighter.spec(), 20.0, 10.0, Control::External, 1);
    let near = w.add_troop(&BaseClass::Fighter.spec(), 11.0, 10.0, Control::External, 2);
    let foe = w.add_guard(10.5, 10.5);
    assert!(!w.blessed(healer) && !w.blessed(far) && !w.blessed(foe));
    assert!(w.blessed(near));
    // Two healers bless each other.
    let second = w.add_troop(&BaseClass::Healer.spec(), 10.0, 11.0, Control::External, 3);
    assert!(w.blessed(healer) && w.blessed(second));
}

#[test]
fn spellcasters_are_exposed_to_troops() {
    // Same guard, same time: the Mage loses a quarter more than its share.
    let (fighter, mage) = (taken(BaseClass::Fighter, false), taken(BaseClass::Mage, false));
    assert!((mage - fighter * (1.0 + CASTER_EXPOSED)).abs() < 1e-3, "{fighter} vs {mage}");
}
