//! What the game and the server send each other, the checks both sides
//! apply, and the one function that runs a raid.
//!
//! The server never trusts a troop's stats from the client: it rebuilds each
//! troop's class from its parents, so a hand-edited file with 10x HP is
//! either rejected or fights with the real numbers. Both sides run
//! [`run_raid`] from this crate, so the attacker's local result and the
//! server's check come from the same code.

use godcore::battle::{fingerprint, Battle};
use godcore::classes::{ClassSpec, Role, TrainPlan};
use godcore::nn::Mlp;
use godcore::scenario::{
    can_deploy, can_place_defender, max_troop_level, HomeBase, MAX_TOWN_HALL,
};
use godcore::sim::{Outcome, OBS_DIM};

/// Battle rules version the game and server must agree on. Sent by the
/// client in this header on every request.
pub use godcore::sim::SIM_VERSION;
pub const SIM_VERSION_HEADER: &str = "x-sim-version";
use godcore::troop::{level_cap, Troop, MAX_LEVEL};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_DEFENDERS: usize = 4;
pub const MAX_ARMY: usize = 5;
pub const MAX_NAME: usize = 24;

// ---------------------------------------------------------------- wire types

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Placement {
    pub troop: Troop,
    pub x: f32,
    pub y: f32,
}

/// A published home base: layout plus the defenders' brains. Attacks always
/// run against a fixed snapshot, so replays stay valid after a re-publish.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaseSnapshot {
    pub base: HomeBase,
    pub defenders: Vec<Placement>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RaidResult {
    pub stars: u32,
    pub destruction: f32,
    pub town_hall_destroyed: bool,
    pub ticks: u32,
    /// Hex of the godcore world fingerprint after the last tick.
    pub fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    pub trophies: i32,
    pub gold: i64,
    pub town_hall: Option<i32>,
    pub has_base: bool,
    pub shield_until: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicPlayer {
    pub id: Uuid,
    pub display_name: String,
    pub trophies: i32,
    pub town_hall: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevLoginRequest {
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Providers {
    pub dev: bool,
    pub oauth: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginStartRequest {
    pub provider: String,
}

/// Open `url` in the player's browser, then poll with `poll_secret` until
/// the login completes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginStart {
    pub login_id: Uuid,
    pub poll_secret: String,
    pub url: String,
    pub expires_in_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginPoll {
    pub poll_secret: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginPollResponse {
    Pending,
    Done(AuthResponse),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameRequest {
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Published {
    pub snapshot_id: Uuid,
    pub town_hall: u32,
    pub defenders: usize,
}

/// An opponent found by matchmaking. Submit the raid before `expires_at`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Match {
    pub attack_id: Uuid,
    pub sim_version: u32,
    pub defender: PublicPlayer,
    pub snapshot_id: Uuid,
    pub snapshot: BaseSnapshot,
    pub expires_at: String,
    /// Gold on offer at 100% destruction.
    pub max_loot: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttackSubmit {
    pub army: Vec<Placement>,
    /// What the attacker's machine got. Optional; a mismatch is recorded.
    pub client_result: Option<RaidResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttackOutcome {
    pub attack_id: Uuid,
    /// The server's result. This is the one that counts.
    pub result: RaidResult,
    /// True when the client sent a result and it matched exactly.
    pub client_matched: Option<bool>,
    pub trophies: i32,
    pub gold_looted: i64,
    pub attacker: User,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttackLog {
    pub attack_id: Uuid,
    pub attacker: PublicPlayer,
    pub defender: PublicPlayer,
    pub result: RaidResult,
    pub trophies: i32,
    pub gold_looted: i64,
    pub client_matched: Option<bool>,
    pub finished_at: String,
}

/// Everything needed to watch a raid again, or to turn it into a training
/// drill: the defender's snapshot and the attacker's army.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Replay {
    pub log: AttackLog,
    /// Battle rules the raid was fought under. Re-running it only gives the
    /// same result with the same rules.
    pub sim_version: u32,
    pub snapshot: BaseSnapshot,
    pub army: Vec<Placement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

// ---------------------------------------------------------------- validation

#[derive(Clone, Debug, PartialEq)]
pub struct Invalid(pub String);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn bad<T>(msg: impl Into<String>) -> Result<T, Invalid> {
    Err(Invalid(msg.into()))
}

/// The class a troop really has, rebuilt from its blend and training plan.
///
/// The game recruits a troop from whole percentages of one or more classes
/// of the same side (summing to 100) and stores the normalised shares. We
/// recover those percentages and run the same `ClassSpec::blend`, so the
/// rebuilt stats match the game's bit for bit and any edited stat is lost.
pub fn canonical_spec(spec: &ClassSpec) -> Result<ClassSpec, Invalid> {
    let parents = &spec.parents;
    if parents.is_empty() {
        return bad("a troop needs at least one class");
    }
    let mut parts = Vec::with_capacity(parents.len());
    for (k, &(c, w)) in parents.iter().enumerate() {
        let pct = (w * 100.0).round();
        if !w.is_finite() || (w * 100.0 - pct).abs() > 1e-3 || pct < 1.0 {
            return bad("class shares must be whole percentages");
        }
        if parents[..k].iter().any(|p| p.0 == c) {
            return bad("a class appears twice in the blend");
        }
        if c.role() != parents[0].0.role() {
            return bad("a blend can't mix attackers and defenders");
        }
        parts.push((c, pct));
    }
    if parts.iter().map(|p| p.1 as u32).sum::<u32>() != 100 {
        return bad("class shares must add up to 100%");
    }
    let mut out = ClassSpec::blend(&parts);
    if let Some(plan) = spec.plan {
        let active = TrainPlan::active(&out);
        let focus_ok = plan.focus.iter().sum::<u32>() == 100
            && plan.focus.iter().zip(active).all(|(f, a)| a || *f == 0);
        if plan.solo > 100 || plan.self_play > 100 || !focus_ok {
            return bad("training plan shares must be 0-100% and the focus must add up to 100%");
        }
        out.apply_plan(plan);
    }
    Ok(out)
}

/// Check a troop and return the copy the server keeps: class rebuilt from
/// its parents, critic and history dropped (battles only use the actor).
pub fn canonical_troop(t: &Troop, role: Role, max_level: u32) -> Result<Troop, Invalid> {
    let spec = canonical_spec(&t.spec)?;
    if spec.role != role {
        return bad(match role {
            Role::Attack => "only attackers can raid",
            Role::Defend => "only defenders can guard a base",
        });
    }
    if t.level < 1 || t.level > MAX_LEVEL.min(max_level) {
        return bad(format!(
            "troop level {} is above what this Town Hall allows ({max_level})",
            t.level
        ));
    }
    if t.steps > level_cap(t.level) {
        return bad("troop has more training than its level allows");
    }
    // Players pick the brain size, but only from godcore's list.
    let dims = &t.actor.dims;
    if !godcore::ppo::actor_dims_ok(dims) {
        return bad(format!(
            "troop brain size must be one of {:?} neurons per layer",
            godcore::ppo::HIDDEN_SIZES
        ));
    }
    if t.brain.is_some_and(|h| h != dims[1]) {
        return bad("troop brain doesn't match its chosen size");
    }
    let n: usize = dims.windows(2).map(|d| d[0] * d[1] + d[1]).sum();
    if t.actor.params.len() != n {
        return bad("troop brain has the wrong shape");
    }
    // Weapon mastery comes from training time with each weapon, which can't
    // add up to more than the troop's total training.
    if total(t.mastery.values()) > t.steps {
        return bad("troop has more weapon training than training");
    }
    // Class moves share the troop's training time between them.
    if total(t.move_xp.values()) > t.steps {
        return bad("troop has more move practice than training");
    }
    let mut spec = spec;
    spec.weapon = t.spec.weapon;
    spec.mastery = t.mastery_of(t.spec.weapon());
    if t.actor.params.iter().any(|p| !p.is_finite() || p.abs() > 1e6) {
        return bad("troop brain has invalid numbers");
    }
    let name: String = t.name.trim().chars().take(MAX_NAME).collect();
    let mut out = Troop {
        id: t.id,
        name: if name.is_empty() { "Troop".into() } else { name },
        spec,
        level: t.level,
        steps: t.steps,
        actor: t.actor.clone(),
        critic: Mlp {
            dims: vec![OBS_DIM, 1],
            params: vec![0.0; OBS_DIM + 1],
        },
        curve: Vec::new(),
        skills: t.skills,
        seed: t.seed,
        sessions: t.sessions,
        mastery: t.mastery.clone(),
        move_xp: t.move_xp.clone(),
        brain: Some(t.actor.dims[1]),
        // Team training happens on the player's machine and changes nothing
        // in a battle, so it is kept as sent.
        team_steps: t.team_steps,
    };
    // Move practice comes from the troop's own practice record, never from
    // the client's copy in the spec.
    out.sync_moves();
    Ok(out)
}

/// Sum without wrapping, so huge values can't overflow past a check.
fn total<'a>(v: impl Iterator<Item = &'a u64>) -> u64 {
    v.fold(0u64, |a, b| a.saturating_add(*b))
}

fn finite(x: f32, y: f32) -> bool {
    x.is_finite() && y.is_finite()
}

/// Check a base before publishing and return the canonical copy.
pub fn canonical_snapshot(s: &BaseSnapshot) -> Result<BaseSnapshot, Invalid> {
    let th = s.base.town_hall;
    if !(1..=MAX_TOWN_HALL).contains(&th) {
        return bad("Town Hall level must be 1 to 5");
    }
    let mut base = HomeBase {
        town_hall: th,
        buildings: Vec::new(),
    };
    for b in &s.base.buildings {
        if !finite(b.x, b.y) || !base.fits(b.x, b.y) {
            return bad("a building is outside the base or overlaps another");
        }
        if !base.can_add(b.kind) {
            return bad(format!(
                "Town Hall {th} allows {} {}s",
                b.kind.max_at(th),
                b.kind.name()
            ));
        }
        base.buildings.push(*b);
    }
    if s.defenders.len() > MAX_DEFENDERS {
        return bad(format!("at most {MAX_DEFENDERS} defenders"));
    }
    let defenders = s
        .defenders
        .iter()
        .map(|p| {
            if !finite(p.x, p.y) || !can_place_defender(p.x, p.y) {
                return bad("a defender is outside the base");
            }
            Ok(Placement {
                troop: canonical_troop(&p.troop, Role::Defend, max_troop_level(th))?,
                x: p.x,
                y: p.y,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(BaseSnapshot { base, defenders })
}

/// Check an attacking army. `town_hall` is the attacker's own Town Hall.
pub fn canonical_army(army: &[Placement], town_hall: u32) -> Result<Vec<Placement>, Invalid> {
    if army.is_empty() || army.len() > MAX_ARMY {
        return bad(format!("send 1 to {MAX_ARMY} troops"));
    }
    army.iter()
        .map(|p| {
            if !finite(p.x, p.y) || !can_deploy(p.x, p.y) {
                return bad("troops must be placed outside the red zone");
            }
            Ok(Placement {
                troop: canonical_troop(&p.troop, Role::Attack, max_troop_level(town_hall))?,
                x: p.x,
                y: p.y,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- the raid

pub fn raid_battle(snapshot: &BaseSnapshot, army: &[Placement]) -> Battle {
    let place = |v: &[Placement]| v.iter().map(|p| (p.troop.clone(), (p.x, p.y))).collect();
    Battle::defense(&snapshot.base, place(&snapshot.defenders), None, place(army))
}

/// Run a raid to the end. Pass canonical inputs (see `canonical_*`) so the
/// attacker's machine and the server compute the same thing.
pub fn run_raid(snapshot: &BaseSnapshot, army: &[Placement]) -> RaidResult {
    let mut b = raid_battle(snapshot, army);
    b.run();
    debug_assert_ne!(b.outcome(), Outcome::Running);
    let w = &b.world;
    RaidResult {
        stars: w.stars(),
        destruction: w.destruction(),
        town_hall_destroyed: w.core_destroyed(),
        ticks: w.tick,
        fingerprint: format!("{:016x}", fingerprint(w)),
    }
}

// ---------------------------------------------------------------- economy

/// Trophies the attacker wins (and the defender loses). A raid with no
/// stars counts as a defence win.
pub fn trophy_swing(stars: u32) -> i32 {
    match stars {
        0 => -8,
        1 => 10,
        2 => 20,
        _ => 30,
    }
}

/// Up to 20% of the defender's gold, scaled by destruction.
pub fn loot(defender_gold: i64, destruction: f32) -> i64 {
    let max = defender_gold / 5;
    (max as f64 * destruction.clamp(0.0, 1.0) as f64).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use godcore::classes::BaseClass;
    use godcore::scenario::{home_spots, BuildKind, RAID_ENTRY};

    fn troop(id: u64, spec: ClassSpec) -> Troop {
        Troop::new(id, format!("t{id}"), spec, 100 + id)
    }

    fn snapshot() -> BaseSnapshot {
        let spots = home_spots(2);
        BaseSnapshot {
            base: HomeBase::default(),
            defenders: vec![
                Placement {
                    troop: troop(1, BaseClass::Guard.spec()),
                    x: spots[0].0,
                    y: spots[0].1,
                },
                Placement {
                    troop: troop(2, BaseClass::Sentry.spec()),
                    x: spots[1].0,
                    y: spots[1].1,
                },
            ],
        }
    }

    fn army() -> Vec<Placement> {
        (0..3)
            .map(|k| Placement {
                troop: troop(10 + k, BaseClass::Fighter.spec()),
                x: RAID_ENTRY.0 + k as f32,
                y: RAID_ENTRY.1,
            })
            .collect()
    }

    #[test]
    fn canonical_inputs_pass_and_replay() {
        let s = canonical_snapshot(&snapshot()).unwrap();
        let a = canonical_army(&army(), 1).unwrap();
        let r1 = run_raid(&s, &a);
        let r2 = run_raid(&s, &a);
        assert_eq!(r1, r2);
        // A JSON round trip (what the network does) changes nothing.
        let s2: BaseSnapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let a2: Vec<Placement> = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(run_raid(&s2, &a2), r1);
    }

    #[test]
    fn edited_stats_are_replaced() {
        let mut a = army();
        a[0].troop.spec.hp = 1e9;
        a[0].troop.spec.dmg = 1e9;
        let c = canonical_army(&a, 1).unwrap();
        assert_eq!(c[0].troop.spec.hp, BaseClass::Fighter.spec().hp);
        assert_eq!(c[0].troop.spec.dmg, BaseClass::Fighter.spec().dmg);
    }

    #[test]
    fn bad_inputs_are_rejected() {
        let mut a = army();
        a[0].troop.spec.parents = vec![(BaseClass::Fighter, 0.6), (BaseClass::Healer, 0.6)];
        assert!(canonical_army(&a, 1).is_err(), "blend over 100%");

        let mut a = army();
        a[0].troop.spec.parents = vec![(BaseClass::Guard, 1.0)];
        assert!(canonical_army(&a, 1).is_err(), "defender in an army");

        let mut a = army();
        a[0].troop.level = 4;
        a[0].troop.upgrade_brain();
        assert!(canonical_army(&a, 1).is_err(), "level above Town Hall cap");
        assert!(canonical_army(&a, 3).is_ok());

        let mut a = army();
        a[0].troop.steps = 5_000_000;
        assert!(canonical_army(&a, 1).is_err(), "too much training");

        let mut a = army();
        a[0].troop.actor.params[3] = f32::NAN;
        assert!(canonical_army(&a, 1).is_err(), "NaN brain");

        let mut a = army();
        a[0].troop.actor.params.pop();
        assert!(canonical_army(&a, 1).is_err(), "wrong brain size");

        let mut a = army();
        a[0].x = 16.0;
        a[0].y = 16.0;
        assert!(canonical_army(&a, 1).is_err(), "deployed inside the base");

        assert!(canonical_army(&[], 1).is_err());

        let mut s = snapshot();
        s.base.buildings.push(s.base.buildings[0]);
        assert!(canonical_snapshot(&s).is_err(), "overlapping buildings");

        let mut s = snapshot();
        s.base.buildings.push(godcore::scenario::Placed {
            kind: BuildKind::Cannon,
            x: 0.0,
            y: 4.0,
        });
        assert!(canonical_snapshot(&s).is_err(), "cannon at Town Hall 1");

        let mut s = snapshot();
        s.defenders[0].x = 1.0;
        assert!(canonical_snapshot(&s).is_err(), "defender outside the base");
    }

    #[test]
    fn game_default_bases_pass() {
        for base in [HomeBase::default(), HomeBase::reference()] {
            let s = BaseSnapshot { base, defenders: vec![] };
            assert!(canonical_snapshot(&s).is_ok());
        }
    }

    fn game_blend(parts: &[(BaseClass, u32)]) -> ClassSpec {
        // Exactly what the game's recruit screen does.
        let p: Vec<_> = parts.iter().map(|&(c, v)| (c, v as f32)).collect();
        ClassSpec::blend(&p)
    }

    fn same(a: &ClassSpec, b: &ClassSpec) -> bool {
        serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
    }

    #[test]
    fn game_blends_rebuild_bit_for_bit() {
        use BaseClass::*;
        let blends: Vec<Vec<(BaseClass, u32)>> = vec![
            vec![(Mage, 100)],
            vec![(Fighter, 50), (Healer, 50)],
            vec![(Archer, 75), (Healer, 25)],
            vec![(Fighter, 25), (Mage, 75)],
            vec![(Mage, 50), (Healer, 30), (Fighter, 20)],
            vec![(Fighter, 33), (Archer, 33), (Assassin, 34)],
            vec![(Fighter, 20), (Archer, 20), (Healer, 20), (Mage, 20), (Paladin, 20)],
            vec![(Fighter, 1), (Assassin, 99)],
            vec![(Guard, 40), (Warlock, 35), (Hunter, 25)],
            vec![(Sentry, 50), (Keeper, 50)],
        ];
        for b in blends {
            let g = game_blend(&b);
            let c = canonical_spec(&g).unwrap();
            assert!(same(&g, &c), "{b:?}: {} vs {}", g.describe(), c.describe());
            // With a training plan on top too.
            let mut plan = TrainPlan::default_for(&g);
            plan.solo = 70;
            plan.self_play = 20;
            let gp = ClassSpec::planned(&b.iter().map(|&(c, v)| (c, v as f32)).collect::<Vec<_>>(), plan);
            assert!(same(&gp, &canonical_spec(&gp).unwrap()), "{b:?} planned");
        }
    }

    #[test]
    fn tampered_blends_are_rejected_or_fixed() {
        use BaseClass::*;
        let ok = game_blend(&[(Mage, 50), (Healer, 30), (Fighter, 20)]);
        let err = |f: &dyn Fn(&mut ClassSpec)| {
            let mut s = ok.clone();
            f(&mut s);
            canonical_spec(&s).is_err()
        };
        assert!(err(&|s| s.parents[0].1 = 0.9), "shares add to 140%");
        assert!(err(&|s| s.parents[0].1 = 0.2), "shares add to 90%");
        assert!(err(&|s| s.parents[0].1 = 0.505), "not a whole percentage");
        assert!(err(&|s| s.parents[0].1 = f32::NAN));
        assert!(err(&|s| s.parents[1].1 = -0.3), "negative share");
        assert!(err(&|s| s.parents = vec![]));
        assert!(err(&|s| s.parents[1].0 = Mage), "class twice");
        assert!(err(&|s| s.parents[2].0 = Guard), "attacker and defender mixed");
        assert!(err(&|s| s.plan = Some(TrainPlan { solo: 50, self_play: 50, focus: [50, 50, 50, 0] })));
        assert!(err(&|s| s.plan = Some(TrainPlan { solo: 150, self_play: 50, focus: [25, 25, 25, 25] })));
        let no_heal = game_blend(&[(Fighter, 60), (Archer, 40)]);
        let mut s = no_heal.clone();
        s.plan = Some(TrainPlan { solo: 50, self_play: 50, focus: [25, 25, 25, 25] });
        assert!(canonical_spec(&s).is_err(), "focus on healing for a class that can't heal");

        // Edited battle stats are thrown away.
        let mut s = ok.clone();
        s.hp *= 10.0;
        s.dmg *= 10.0;
        s.splash = 9.0;
        s.heal_range = 20.0;
        s.cooldown = 1;
        s.speed = 2.0;
        assert!(same(&canonical_spec(&s).unwrap(), &ok));
    }

    #[test]
    fn weapons_and_mastery_come_from_training() {
        use godcore::weapons::Weapon;
        let mut a = army();
        // A fighter trained 200k steps, 160k of them with daggers.
        let t = &mut a[0].troop;
        t.steps = 200_000;
        t.mastery.insert(Weapon::Greataxe, 40_000);
        t.mastery.insert(Weapon::Daggers, 160_000);
        t.equip(Weapon::Daggers);
        let level = t.spec.mastery;
        assert!(level > 0);
        let c = canonical_army(&a, 1).unwrap();
        assert_eq!(c[0].troop.spec.weapon, Some(Weapon::Daggers));
        assert_eq!(c[0].troop.spec.mastery, level);
        let s = canonical_snapshot(&snapshot()).unwrap();
        assert_eq!(run_raid(&s, &a), run_raid(&s, &c));

        // Claimed mastery without the training behind it is reset.
        let mut b = a.clone();
        b[0].troop.spec.mastery = 5;
        assert_eq!(canonical_army(&b, 1).unwrap()[0].troop.spec.mastery, level);

        // Weapon training can't exceed total training.
        let mut b = a.clone();
        b[0].troop.mastery.insert(Weapon::Daggers, 290_000);
        assert!(canonical_army(&b, 1).is_err());
    }

    #[test]
    fn move_practice_comes_from_the_practice_record() {
        use godcore::moves::Move;
        let mut a = army();
        // A Battle Priest (fighter and healer moves) trained 200k steps.
        let t = &mut a[0].troop;
        t.spec = ClassSpec::blend(&[(BaseClass::Fighter, 50.0), (BaseClass::Healer, 50.0)]);
        t.add_training(200_000);
        let real = t.spec.move_practice;
        assert!(real.iter().any(|p| *p > 0.0), "has practised moves");
        let c = canonical_army(&a, 1).unwrap();
        assert_eq!(c[0].troop.spec.move_practice, real);
        assert_eq!(c[0].troop.move_xp, a[0].troop.move_xp);
        let s = canonical_snapshot(&snapshot()).unwrap();
        assert_eq!(run_raid(&s, &a), run_raid(&s, &c));

        // Claimed full strength without the practice is reset.
        let mut b = a.clone();
        b[0].troop.spec.move_practice = [1.0; godcore::moves::N_MOVES];
        assert_eq!(canonical_army(&b, 1).unwrap()[0].troop.spec.move_practice, real);

        // Practice for a move the classes don't have counts for nothing.
        let mut b = a.clone();
        b[0].troop.steps += 10;
        b[0].troop.move_xp.insert(Move::Blink, 10);
        let c = canonical_army(&b, 1).unwrap();
        assert_eq!(c[0].troop.spec.move_practice[Move::Blink as usize], 0.0);

        // Practice can't add up to more than the training, even by overflow.
        let mut b = a.clone();
        b[0].troop.move_xp.insert(Move::Charge, 250_000);
        assert!(canonical_army(&b, 1).is_err());
        let mut b = a.clone();
        b[0].troop.move_xp.insert(Move::Block, u64::MAX);
        b[0].troop.move_xp.insert(Move::Charge, 2);
        assert!(canonical_army(&b, 1).is_err(), "wrapping sum");
        let mut b = a.clone();
        b[0].troop.mastery.insert(godcore::weapons::Weapon::Daggers, u64::MAX);
        b[0].troop.mastery.insert(godcore::weapons::Weapon::Crossbow, 2);
        assert!(canonical_army(&b, 1).is_err(), "wrapping mastery sum");
    }

    #[test]
    fn team_training_is_kept_and_raids_match() {
        assert_eq!(godcore::sim::SIM_VERSION, 4);
        let mut a = army();
        a[0].troop.team_steps = 120_000;
        let c = canonical_army(&a, 1).unwrap();
        assert_eq!(c[0].troop.team_steps, 120_000);
        let s = canonical_snapshot(&snapshot()).unwrap();
        assert_eq!(run_raid(&s, &a), run_raid(&s, &c));
    }

    #[test]
    fn any_listed_brain_size_is_accepted() {
        use godcore::ppo::HIDDEN_SIZES;
        let s = canonical_snapshot(&snapshot()).unwrap();
        for h in HIDDEN_SIZES {
            let mut a = army();
            assert!(a[0].troop.set_brain(h), "{h}");
            let c = canonical_army(&a, 1).unwrap();
            assert_eq!(c[0].troop.actor.dims[1], h);
            assert_eq!(c[0].troop.brain, Some(h));
            assert_eq!(run_raid(&s, &a), run_raid(&s, &c), "{h}");
        }
        // Old troops with no pick: whatever size their level grew them to.
        let mut a = army();
        a[0].troop.brain = None;
        a[0].troop.level = 3;
        a[0].troop.upgrade_brain();
        assert!(canonical_army(&a, 3).is_ok());
    }

    #[test]
    fn unlisted_or_mismatched_brain_sizes_are_rejected() {
        use godcore::nn::Mlp;
        use godcore::ppo::actor_dims_h;
        use godcore::rng::Rng;
        for h in [0usize, 1, 32, 100, 65, 512, 4096] {
            let mut a = army();
            a[0].troop.actor = Mlp::new(&actor_dims_h(h.max(1)), 0.01, &mut Rng::new(1));
            a[0].troop.brain = Some(h);
            assert!(canonical_army(&a, 1).is_err(), "{h} neurons");
            a[0].troop.brain = None;
            assert!(canonical_army(&a, 1).is_err(), "{h} neurons, no pick");
        }
        // Layers of different sizes, or the wrong input/output size.
        let mut a = army();
        a[0].troop.actor = Mlp::new(&[OBS_DIM, 128, 64, godcore::sim::N_ACTIONS], 0.01, &mut Rng::new(1));
        assert!(canonical_army(&a, 1).is_err());
        let mut a = army();
        a[0].troop.actor = Mlp::new(&[OBS_DIM - 1, 64, 64, godcore::sim::N_ACTIONS], 0.01, &mut Rng::new(1));
        assert!(canonical_army(&a, 1).is_err());
        // A brain that doesn't match the chosen size.
        let mut a = army();
        a[0].troop.set_brain(128);
        a[0].troop.brain = Some(256);
        assert!(canonical_army(&a, 1).is_err());
        // Params cut short.
        let mut a = army();
        a[0].troop.set_brain(192);
        a[0].troop.actor.params.truncate(1000);
        assert!(canonical_army(&a, 1).is_err());
    }

    #[test]
    fn blended_raid_matches_after_canonicalising() {
        use BaseClass::*;
        let mut a = army();
        a[0].troop.spec = game_blend(&[(Mage, 50), (Healer, 30), (Fighter, 20)]);
        a[1].troop.spec = game_blend(&[(Assassin, 60), (Paladin, 40)]);
        let c = canonical_army(&a, 1).unwrap();
        let s = canonical_snapshot(&snapshot()).unwrap();
        // Same result whether the client fights with its own copy or the canonical one.
        assert_eq!(run_raid(&s, &a), run_raid(&s, &c));
    }
}
