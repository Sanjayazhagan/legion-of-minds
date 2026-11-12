//! The "see what it's thinking" view must tell the truth: the move it says a
//! troop is about to make is the move the troop then makes.

use godcore::battle::Battle;
use godcore::classes::BaseClass;
use godcore::mind::think;
use godcore::scenario::BASES;
use godcore::sim::{Control, Kind, Outcome, ACT_CALL_FOCUS, ACT_CALL_WATCH};
use godcore::troop::Troop;

fn squad() -> Vec<(Troop, (f32, f32))> {
    [
        BaseClass::Fighter,
        BaseClass::Healer,
        BaseClass::Archer,
        BaseClass::Mage,
    ]
    .into_iter()
    .enumerate()
    .map(|(k, c)| {
        (
            Troop::new(k as u64, format!("t{k}"), c.spec(), 40 + k as u64),
            (3.0 + 1.5 * k as f32, 4.0),
        )
    })
    .collect()
}

#[test]
fn predicted_move_is_the_move_made() {
    for base in &BASES {
        let mut b = Battle::new(base, squad());
        let mut checked = 0;
        while b.outcome() == Outcome::Running {
            let mut said = Vec::new();
            for i in 0..b.world.units.len() {
                let u = &b.world.units[i];
                if u.alive && u.kind == Kind::Troop && u.control == Control::External && u.stun == 0
                {
                    let t = think(&b.world, i, &b.troops[u.tag].actor);
                    let sum: f32 = t.choices.iter().map(|c| c.1).sum();
                    assert!((sum - 1.0).abs() < 1e-3, "choices add up to {sum}");
                    assert!(t.choices.iter().any(|c| c.0 == t.next));
                    said.push((i, t.next));
                }
            }
            b.step();
            for (i, a) in said {
                // A call doesn't use the turn: the troop shouts and carries on.
                if !(ACT_CALL_FOCUS..=ACT_CALL_WATCH).contains(&a) {
                    assert_eq!(
                        b.world.units[i].last_action, a,
                        "{} tick {}",
                        base.name, b.world.tick
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 50, "{}: only {checked} checks", base.name);
    }
}

#[test]
fn looking_does_not_change_the_battle() {
    let base = &BASES[1];
    let mut a = Battle::new(base, squad());
    let mut b = Battle::new(base, squad());
    while a.outcome() == Outcome::Running {
        for i in 0..a.world.units.len() {
            let u = &a.world.units[i];
            if u.alive && u.kind == Kind::Troop && u.control == Control::External {
                think(&a.world, i, &a.troops[u.tag].actor);
            }
        }
        a.step();
        b.step();
    }
    assert_eq!(
        godcore::battle::fingerprint(&a.world),
        godcore::battle::fingerprint(&b.world)
    );
}
