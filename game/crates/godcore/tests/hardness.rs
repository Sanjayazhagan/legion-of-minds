//! Team drill hardness: harder drills have more and tougher defenders.

use godcore::classes::BaseClass;
use godcore::rng::Rng;
use godcore::scenario::{build_team_drill, MAX_HARDNESS};
use godcore::sim::Kind;

#[test]
fn harder_drills_have_more_and_tougher_defence() {
    let team = vec![BaseClass::Fighter.spec(), BaseClass::Archer.spec(), BaseClass::Healer.spec()];
    let count = |w: &godcore::sim::World, k: Kind| w.units.iter().filter(|u| u.kind == k).count();
    let easy = build_team_drill(&team, 1, &mut Rng::new(4));
    let hard = build_team_drill(&team, MAX_HARDNESS, &mut Rng::new(4));
    assert_eq!(count(&hard, Kind::Guard), count(&easy, Kind::Guard) + 4);
    assert_eq!(count(&hard, Kind::Tower), 4);
    assert_eq!(count(&easy, Kind::Tower), 3);
    let hp = |w: &godcore::sim::World| w.units.iter().find(|u| u.kind == Kind::Tower).unwrap().max_hp;
    assert!((hp(&hard) / hp(&easy) - 1.4).abs() < 1e-4);
    assert!(hard.destruction().abs() < 1e-6, "nothing destroyed at the start");
}
