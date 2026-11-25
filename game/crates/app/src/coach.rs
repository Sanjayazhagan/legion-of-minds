//! The after-battle coach: one plain sentence on why the fight went the way
//! it did, and what to train next, with a button that opens that troop's
//! training plan with the suggestion filled in (nothing changes until the
//! player presses Apply plan).
//!
//! It reads only what happened in the battle (each troop's stats) and the
//! troops' own training, so it never needs the network.

use godcore::classes::{ClassSpec, Role, TrainPlan, FOCUS_NAMES_ATTACK, FOCUS_NAMES_DEFEND};
use godcore::sim::{Kind, Outcome, Team, Unit, World};
use godcore::troop::Troop;

/// Reward goals in a training plan.
pub const BREAK: usize = 0;
pub const FIGHT: usize = 1;
pub const HEAL: usize = 2;
pub const LIVE: usize = 3;

#[derive(Clone, Debug)]
pub struct Tip {
    pub line: String,
    /// The troop to train, and the plan goal to lean on (None: its usual plan).
    pub train: Option<(u64, String, Option<usize>)>,
}

/// The suggestion as a plan: `focus` gets 20 more points, taken evenly from
/// the troop's other goals.
pub fn nudged_plan(spec: &ClassSpec, focus: usize) -> TrainPlan {
    let base = ClassSpec::blend(&spec.parents);
    let mut p = spec.plan.unwrap_or_else(|| TrainPlan::default_for(&base));
    let active = TrainPlan::active(&base);
    if !active[focus] {
        return p;
    }
    let mut need = 20u32.min(100 - p.focus[focus]);
    p.focus[focus] += need;
    // Take it back from the others, largest first, 1 point at a time.
    while need > 0 {
        let Some(k) = (0..4).filter(|&k| k != focus && p.focus[k] > 0).max_by_key(|&k| p.focus[k]) else { break };
        p.focus[k] -= 1;
        need -= 1;
    }
    let sum: u32 = p.focus.iter().sum();
    if sum != 100 {
        p.focus[focus] = (p.focus[focus] + 100).saturating_sub(sum);
    }
    p
}

pub fn focus_name(spec: &ClassSpec, focus: usize) -> &'static str {
    if spec.role == Role::Defend { FOCUS_NAMES_DEFEND[focus] } else { FOCUS_NAMES_ATTACK[focus] }
}

fn name(t: &Troop) -> String {
    t.name.trim().to_string()
}

/// Read a finished battle. `mine` is the side the player's troops were on.
pub fn coach(w: &World, troops: &[Troop], mine: Team) -> Option<Tip> {
    let squad: Vec<(&Unit, &Troop)> = w
        .units
        .iter()
        .filter(|u| u.kind == Kind::Troop && u.team == mine && u.tag < troops.len() && u.control == godcore::sim::Control::External)
        .map(|u| (u, &troops[u.tag]))
        .collect();
    if squad.is_empty() {
        return None;
    }
    let stars = w.stars();
    let tip = |line: String, t: &Troop, focus: Option<usize>| Some(Tip { line, train: Some((t.id, name(t), focus)) });
    let dmg = |u: &Unit| u.stats.unit_dmg + u.stats.building_dmg;
    let by = |f: &dyn Fn(&Unit) -> f32| squad.iter().copied().min_by(|a, b| f(a.0).partial_cmp(&f(b.0)).unwrap());
    let top = |f: &dyn Fn(&Unit) -> f32| squad.iter().copied().max_by(|a, b| f(a.0).partial_cmp(&f(b.0)).unwrap());
    // The troop hitting buildings least, among those built to hit things
    // (a healer isn't told to break walls).
    let weakest_breaker = || {
        let hitters: Vec<_> = squad.iter().copied().filter(|(_, t)| t.spec.dmg >= 5.0).collect();
        let pool = if hitters.is_empty() { squad.clone() } else { hitters };
        pool.into_iter().min_by(|a, b| a.0.stats.building_dmg.partial_cmp(&b.0.stats.building_dmg).unwrap())
    };

    if mine == Team::Defend {
        let held = stars == 0;
        if held && w.destruction() < 0.01 {
            let (u, t) = top(&|u| u.stats.kills as f32 * 10.0 + u.stats.unit_dmg)?;
            return Some(Tip {
                line: format!("Flawless. {} led the defence with {} raider{} down. Try a harder wave next.", name(t), u.stats.kills, if u.stats.kills == 1 { "" } else { "s" }),
                train: None,
            });
        }
        if held {
            let (_, t) = by(&|u| u.stats.unit_dmg)?;
            return tip(format!("Held, but buildings took damage. {} did the least fighting: train it with more weight on Fight raiders.", name(t)), t, Some(FIGHT));
        }
        if let Some((_, t)) = squad.iter().copied().find(|(u, _)| !u.alive && u.stats.unit_dmg < 20.0) {
            return tip(format!("Raiders got through. {} fell before it could do much: train it with more weight on Stay alive.", name(t)), t, Some(LIVE));
        }
        let (_, t) = by(&|u| u.stats.unit_dmg)?;
        return tip(format!("Raiders got through. {} landed the fewest blows: train it with more weight on Fight raiders.", name(t)), t, Some(FIGHT));
    }

    // Raids.
    let outcome = w.outcome();
    if outcome == Outcome::Victory {
        let (u, t) = top(&|u| crate::journal::score(u))?;
        let what = if u.stats.heal > u.stats.unit_dmg { format!("{:.0} health healed", u.stats.heal) } else { format!("{:.0} damage", dmg(u)) };
        return Some(Tip { line: format!("Total victory! {} was the star with {what}. Try a harder base next.", name(t)), train: None });
    }
    // A troop that fell early and did little.
    let fell_early = squad
        .iter()
        .copied()
        .filter(|(u, _)| !u.alive && dmg(u) + u.stats.heal < 25.0)
        .min_by(|a, b| dmg(a.0).partial_cmp(&dmg(b.0)).unwrap());
    if let Some((u, t)) = fell_early {
        if u.stats.surprised >= 2 {
            return tip(format!("{} was hit from behind {} times and fell early. Training with more weight on Stay alive teaches it to watch its back.", name(t), u.stats.surprised), t, Some(LIVE));
        }
        return tip(format!("{} fell before it could do much. Train it with more weight on Stay alive so it picks its fights.", name(t)), t, Some(LIVE));
    }
    // A healer that barely healed.
    if let Some((u, t)) = squad.iter().copied().find(|(u, t)| t.spec.can_heal() && u.stats.heal < 15.0) {
        let hurt = squad.iter().any(|(o, _)| o.hp < o.max_hp * 0.7 || !o.alive);
        if hurt && u.stats.heal < 15.0 {
            return tip(format!("{} barely healed while the squad got hurt. Train it with more weight on Heal allies.", name(t)), t, Some(HEAL));
        }
    }
    // Lots of missed shots.
    if let Some((u, t)) = top(&|u| u.stats.missed as f32).filter(|(u, _)| u.stats.missed >= 8) {
        return tip(format!("{} missed {} shots. More practice against enemy troops sharpens its aim.", name(t), u.stats.missed), t, Some(FIGHT));
    }
    // Out of breath for much of it.
    if let Some((u, t)) = top(&|u| u.stats.winded as f32).filter(|(u, _)| u.stats.winded as f32 > w.tick as f32 * 0.25) {
        let _ = u;
        return tip(format!("{} was out of breath for a lot of the fight. Rested troops hit harder, and training builds stamina.", name(t)), t, None);
    }
    // Time ran out with buildings standing.
    if outcome == Outcome::Timeout && w.destruction() < 0.5 {
        let (_, t) = weakest_breaker()?;
        return tip(format!("Time ran out with most of the base standing. {} did the least to the buildings: train it with more weight on Break buildings.", name(t)), t, Some(BREAK));
    }
    if stars > 0 {
        let (_, t) = weakest_breaker()?;
        return tip(format!("Good raid. To get the last star{}, train {} with more weight on Break buildings.", if stars == 1 { "s" } else { "" }, name(t)), t, Some(BREAK));
    }
    // Nothing stood out: the least-trained troop.
    let t = squad.iter().map(|(_, t)| *t).min_by(|a, b| a.skills.win.partial_cmp(&b.skills.win).unwrap())?;
    tip(format!("Every loss teaches something. {} wins the fewest drills: more training is the surest fix.", name(t)), t, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use godcore::classes::BaseClass;

    #[test]
    fn a_nudged_plan_still_adds_up_to_100() {
        for c in BaseClass::ALL {
            let spec = c.spec();
            for f in 0..4 {
                let p = nudged_plan(&spec, f);
                assert_eq!(p.focus.iter().sum::<u32>(), 100, "{:?} focus {f}", c);
                if TrainPlan::active(&spec)[f] {
                    assert!(p.focus[f] > TrainPlan::default_for(&spec).focus[f] || p.focus[f] == 100);
                }
            }
        }
    }

    #[test]
    fn a_squad_that_never_fought_gets_a_tip_about_someone() {
        let t = Troop::new(3, "Ann".into(), BaseClass::Fighter.spec(), 1);
        let mut w = World::new(10);
        w.add_troop(&t.spec, 5.0, 5.0, godcore::sim::Control::External, 0);
        w.add_tower(15.0, 15.0);
        w.recount_buildings();
        let tip = coach(&w, &[t], Team::Attack).unwrap();
        assert!(tip.line.contains("Ann"), "{}", tip.line);
    }
}
