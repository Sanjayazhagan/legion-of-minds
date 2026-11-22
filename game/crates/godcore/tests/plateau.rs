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
    let w = plateau_window(1);
    assert_eq!(t.plateau(), Plateau::Warming { done: 0, need: 2 * w + w / 4 });
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
fn once_plateaued_it_stays_ready_until_the_next_level() {
    let mut t = trained(200_000, |s| (s as f32 / 60_000.0).min(0.7));
    t.note_plateau();
    assert!(t.plateaued);
    // A lucky streak afterwards doesn't take it away.
    for k in 1..=10u64 {
        t.curve.push(CurvePoint { steps: 200_000 + k * 8192, win: 0.7 + k as f32 * 0.02, died: 0.5, timeout: 0.0, ret: 0.0 });
    }
    t.steps = 200_000 + 10 * 8192;
    assert!(t.plateau().ready());
    t.level_up();
    assert!(!t.plateaued && !t.plateau().ready());
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

#[test]
fn level_5_brains_keep_growing_past_256_on_each_plateau() {
    use godcore::ppo::{brain_cap, brain_grow_cost, brain_bracket, HIDDEN_SIZES};
    let mut t = Troop::new(1, "t".into(), BaseClass::Fighter.spec(), 5);
    for _ in 1..5 {
        t.level_up();
    }
    assert_eq!((t.level, t.hidden()), (5, 256));
    assert_eq!(t.cap(), 3_500_000);
    // Level 5 no longer means "at cap": the check runs for the next size.
    assert!(!t.plateau().ready(), "{:?}", t.plateau());
    assert!(!t.can_grow_brain());
    // Flat after learning: ready to grow.
    let w = t.plateau_window();
    let steps = 3 * w;
    for k in 1..=(steps / 8192) {
        let s = k * 8192;
        t.curve.push(CurvePoint { steps: s, win: (s as f32 / w as f32).min(0.7), died: 0.5, timeout: 0.0, ret: 0.0 });
    }
    t.steps = steps;
    assert!(t.can_grow_brain(), "{:?}", t.plateau());
    let obs: Vec<f32> = (0..godcore::sim::OBS_DIM).map(|k| (k as f32 * 0.13).sin()).collect();
    let before = {
        let mut c = godcore::nn::Cache::default();
        t.actor.forward(&obs, &mut c);
        c.acts.last().unwrap().clone()
    };
    assert_eq!(t.grow_brain(), Some(384));
    assert_eq!((t.hidden(), t.brain, t.cap()), (384, Some(384), brain_cap(384)));
    assert_eq!(t.level_from, Some(steps));
    assert!(!t.plateau().ready());
    // Same choices as before the growth.
    let mut c = godcore::nn::Cache::default();
    t.actor.forward(&obs, &mut c);
    for (a, b) in before.iter().zip(c.acts.last().unwrap()) {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }
    // Saved and loaded, it keeps its size (the file packs the big brain).
    let back = Troop::load_json(&t.save_json()).unwrap();
    assert_eq!(back.hidden(), 384);
    assert_eq!(back.actor.params, t.actor.params);
    // Costs and categories.
    assert_eq!(brain_grow_cost(384), 3600);
    assert_eq!(brain_grow_cost(512), 4900);
    assert_eq!(brain_bracket(256), Some(0));
    assert_eq!(brain_bracket(384), Some(1));
    assert_eq!(brain_bracket(1024), Some(1));
    assert_eq!(brain_bracket(1536), None);
    // The biggest size has nowhere to grow.
    let mut big = Troop::new(2, "b".into(), BaseClass::Fighter.spec(), 6);
    big.level = 5;
    assert!(big.set_brain(*HIDDEN_SIZES.last().unwrap()));
    assert_eq!(big.next_brain(), None);
    assert_eq!(big.plateau(), Plateau::AtCap);
    // Below level 5 there is no growing past the level's size.
    let mut low = Troop::new(3, "l".into(), BaseClass::Fighter.spec(), 7);
    assert_eq!(low.grow_brain(), None);
}
