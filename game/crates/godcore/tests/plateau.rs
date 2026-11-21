//! Levelling up waits until training has plateaued at the current brain size.

use godcore::classes::BaseClass;
use godcore::troop::{plateau_window, CurvePoint, Plateau, Troop};

/// A troop at level 1 trained to `steps`, whose drill wins follow `win(steps)`.
fn trained(steps: u64, win: impl Fn(u64) -> f32) -> Troop {
    let mut t = Troop::new(1, "t".into(), BaseClass::Fighter.spec(), 5);
    let mut s = 0;
    while s < steps {
        s = (s + 8192).min(steps);
        t.curve.push(CurvePoint { steps: s, win: win(s), died: 0.5, timeout: 0.0, ret: 0.0 });
    }
    t.steps = steps;
    t
}

#[test]
fn a_fresh_troop_must_train_before_it_can_level() {
    let t = Troop::new(1, "t".into(), BaseClass::Fighter.spec(), 5);
    assert_eq!(t.plateau(), Plateau::Warming { done: 0, need: 2 * plateau_window(1) });
    assert!(!t.plateau().ready());
}

#[test]
fn a_rising_score_is_still_improving() {
    let t = trained(200_000, |s| s as f32 / 250_000.0);
    assert!(matches!(t.plateau(), Plateau::Improving { gain } if gain > 0.1), "{:?}", t.plateau());
    assert!(!t.plateau().ready());
}

#[test]
fn a_flat_score_has_plateaued() {
    // Learned fast, then stopped getting better.
    let t = trained(200_000, |s| (s as f32 / 60_000.0).min(0.7));
    assert!(matches!(t.plateau(), Plateau::Reached { .. }), "{:?}", t.plateau());
    assert!(t.plateau().ready());
}

#[test]
fn flat_before_learning_anything_is_not_a_plateau() {
    // RL often sits near zero before it clicks.
    let t = trained(200_000, |_| 0.05);
    assert!(matches!(t.plateau(), Plateau::Starting { .. }), "{:?}", t.plateau());
    assert!(!t.plateau().ready());
    // A troop already near the top counts as full.
    let t = trained(200_000, |_| 1.0);
    assert!(t.plateau().ready());
}

#[test]
fn using_the_whole_level_cap_counts() {
    let t = trained(300_000, |s| s as f32 / 300_000.0);
    assert_eq!(t.plateau(), Plateau::AtCap);
}

#[test]
fn levelling_restarts_the_check_and_grows_the_brain() {
    let mut t = trained(200_000, |s| (s as f32 / 60_000.0).min(0.7));
    assert!(t.plateau().ready());
    t.level_up();
    assert_eq!(t.hidden(), 96);
    assert!(matches!(t.plateau(), Plateau::Warming { done: 0, .. }));
}

#[test]
fn old_saves_get_one_free_level_up() {
    let t = trained(50_000, |s| s as f32 / 100_000.0);
    let json = t.save_json().replace(",\"level_from\":0", "");
    assert!(!json.contains("level_from"));
    let mut old = Troop::load_json(&json).unwrap();
    assert_eq!(old.plateau(), Plateau::Grandfathered);
    old.level_up();
    assert!(!old.plateau().ready(), "after that the check applies");
}
