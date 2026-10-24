//! Battles must replay bit-for-bit: wars between players depend on it.

use godcore::battle::{fingerprint, Battle};
use godcore::classes::{BaseClass, ClassSpec};
use godcore::scenario::BASES;
use godcore::troop::Troop;

fn squad() -> Vec<(Troop, (f32, f32))> {
    vec![
        (
            Troop::new(1, "a".into(), BaseClass::Fighter.spec(), 11),
            (3.0, 4.0),
        ),
        (
            Troop::new(2, "b".into(), BaseClass::Healer.spec(), 12),
            (4.5, 4.0),
        ),
        (
            Troop::new(
                3,
                "c".into(),
                ClassSpec::hybrid(BaseClass::Fighter, BaseClass::Healer, 0.5),
                13,
            ),
            (6.0, 3.0),
        ),
        (
            Troop::new(4, "d".into(), BaseClass::Archer.spec(), 14),
            (28.0, 28.0),
        ),
    ]
}

#[test]
fn same_battle_same_result() {
    for base in &BASES {
        let mut a = Battle::new(base, squad());
        let mut b = Battle::new(base, squad());
        while a.outcome() == godcore::sim::Outcome::Running {
            a.step();
            b.step();
            assert_eq!(
                fingerprint(&a.world),
                fingerprint(&b.world),
                "{} diverged at tick {}",
                base.name,
                a.world.tick
            );
        }
    }
}

#[test]
fn saved_troop_fights_identically() {
    let base = &BASES[1];
    let reloaded: Vec<(Troop, (f32, f32))> = squad()
        .into_iter()
        .map(|(t, p)| (Troop::load_json(&t.save_json()).unwrap(), p))
        .collect();
    let mut a = Battle::new(base, squad());
    let mut b = Battle::new(base, reloaded);
    a.run();
    b.run();
    assert_eq!(fingerprint(&a.world), fingerprint(&b.world));
}

/// Pinned result. If this fails on another machine or compiler, the sim or
/// the network math is no longer platform-independent.
#[test]
fn golden_fingerprint() {
    let mut b = Battle::new(&BASES[2], squad());
    b.run();
    let fp = fingerprint(&b.world);
    println!("golden fingerprint {fp:#x} after {} ticks", b.world.tick);
    assert_eq!(
        fp, GOLDEN,
        "battle result changed: update GOLDEN only if the sim was changed on purpose"
    );
}

/// Sim version 7 (calls to teammates: 109 inputs, 24 actions).
const GOLDEN: u64 = 0x1a85_e6aa_4226_6b70;
