//! Learning moments: noticing when a troop in training picks up a new
//! behaviour ("Sol learned to heal the most-hurt ally first").
//!
//! Every so often during training, the troop's current brain plays a fixed
//! set of test drills (`PROBES` of them, the same seeds every time). The
//! results are compared with what the brain did at the start of the session,
//! drill by drill: the same drill, the same start, only the brain differs.
//! A change only counts when it shows up in most of the test drills, so a
//! lucky drill never makes a moment.
//!
//! Two layers look for changes:
//! 1. Named behaviours, one stat each (heal targeting, distance kept from
//!    enemies, dodges, breath kept...), with a ready-made caption.
//! 2. Surprises: a move used for the first time in most drills, or a big
//!    change in what the brain does in the same situations, described by the
//!    action and situation that changed most ("now shoves more near towers"),
//!    else "found a new trick".
//!
//! A moment keeps only what it takes to replay it: the spec, the probe seed
//! and the two brains. Replays are deterministic, so no video is stored.

use crate::classes::{ClassSpec, Role as Side};
use crate::nn::{Cache, Mlp};
use crate::ppo::greedy_action;
use crate::rng::Rng;
use crate::scenario::{add_raiders, build_defense_drill, build_drill, Drill, DEFENSE_SLOTS};
use crate::sim::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Test drills per check. Half solo raids, half squad raids for attackers
/// (squads give healers someone to heal), all defence drills for defenders.
pub const PROBES: usize = 12;
/// Battles a change must show up in (out of those where it can show at all).
const MIN_SHOWN: usize = 3;
/// Share of those battles it must show up in.
const SHOWN_SHARE: f32 = 0.6;
/// Situations sampled per probe battle, for the "same situation" check.
const SAMPLES_PER_BATTLE: usize = 60;
const SAMPLE_EVERY: u32 = 5;

/// The seed of test drill `k` for a troop born with `seed`.
pub fn probe_seed(seed: u64, k: usize) -> u64 {
    seed ^ 0x1ea4_2b0e_5eed_0000 ^ (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// One test drill being played: the world, which units the brain drives,
/// and running tallies of what it did.
#[derive(Clone)]
pub struct Run {
    pub world: World,
    /// Units the brain drives (one for raids, three for defence).
    pub controlled: Vec<usize>,
    pub defense: bool,
    pub tally: Tally,
    cache: Cache,
    actions: Vec<u8>,
    /// What each unit did last tick (for drawing).
    pub last: Vec<u8>,
}

/// Running counts for one battle.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub ticks: u32,
    /// Heals where both the nearest and second nearest ally were hurt and in
    /// reach, and how many went to the more hurt of the two.
    pub heal_choices: u32,
    pub heal_hurt_first: u32,
    /// Ticks with an enemy fighter within 8, and of those, ticks with none
    /// in striking reach (2.5).
    pub fight_ticks: u32,
    pub safe_ticks: u32,
    /// Times each action was taken, and the first tick it was.
    pub uses: Vec<u32>,
    pub first_use: Vec<Option<u32>>,
    /// First tick each stat's good behaviour showed.
    pub key: [Option<u32>; N_STATS],
    /// Situations seen: what the brain saw, what it could do, and tags.
    pub samples: Vec<Sample>,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub obs: Vec<f32>,
    pub mask: [bool; N_ACTIONS],
    pub tags: u8,
}

// Situation tags.
const NEAR_TOWER: u8 = 1;
const HURT: u8 = 2;
const TIRED: u8 = 4;
const CROWDED: u8 = 8;
const DARK: u8 = 16;
const SITUATIONS: [(u8, &str); 6] = [
    (0, ""),
    (NEAR_TOWER, " near towers"),
    (HURT, " when hurt"),
    (TIRED, " when tired"),
    (CROWDED, " when outnumbered"),
    (DARK, " in the dark"),
];

impl Run {
    /// Test drill `k` for a troop of this spec, from `seed` (see `probe_seed`).
    pub fn new(spec: &ClassSpec, seed: u64, k: usize) -> Run {
        let mut rng = Rng::new(seed);
        let (world, controlled, defense) = if spec.role == Side::Defend {
            let (mut w, from) = build_defense_drill(spec, &mut rng);
            let n = 2 + rng.below(2);
            add_raiders(&mut w, n, from, 1.0, &mut rng);
            (w, (0..DEFENSE_SLOTS).collect(), true)
        } else {
            let drill = if k % 2 == 0 { Drill::Solo } else { Drill::Squad };
            (build_drill(spec, drill, &mut rng), vec![0], false)
        };
        let n = world.units.len();
        Run {
            world,
            controlled,
            defense,
            tally: Tally { uses: vec![0; N_ACTIONS], first_use: vec![None; N_ACTIONS], ..Default::default() },
            cache: Cache::default(),
            actions: vec![0; n],
            last: vec![0; n],
        }
    }

    /// The drill has ended: the base fell or held, time ran out, or (in a
    /// raid) the troop fell.
    pub fn over(&self) -> bool {
        self.world.outcome() != Outcome::Running || (!self.defense && !self.world.units[0].alive)
    }

    /// The brain's side won: the raid took the base, or the base held.
    pub fn won(&self) -> bool {
        (self.world.outcome() == Outcome::Victory) != self.defense
    }

    /// One tick with `actor` driving the controlled units.
    pub fn step(&mut self, actor: &Mlp) {
        if self.over() {
            return;
        }
        let tick = self.world.tick;
        for a in self.actions.iter_mut() {
            *a = 0;
        }
        for k in 0..self.controlled.len() {
            let i = self.controlled[k];
            if !self.world.units[i].alive {
                continue;
            }
            let obs = self.world.observe(i);
            let mask = self.world.mask(i);
            let a = greedy_action(actor, &obs, &mask, &mut self.cache);
            self.actions[i] = a;
            self.note(i, a, &mask, tick);
            if tick % SAMPLE_EVERY == 0 && self.tally.samples.len() < SAMPLES_PER_BATTLE {
                let tags = self.situation(i);
                self.tally.samples.push(Sample { obs: obs.to_vec(), mask, tags });
            }
        }
        self.last.copy_from_slice(&self.actions);
        let standing = self.buildings_left();
        self.world.step(&self.actions);
        if self.buildings_left() < standing && !self.defense {
            self.tally.key[DESTRUCTION].get_or_insert(tick);
        }
        self.tally.ticks += 1;
        // Moves that leave a mark in the unit's stats.
        for &i in &self.controlled {
            let s = &self.world.units[i].stats;
            for (stat, n) in [(DODGES, s.evaded as f32), (BLOCKS, s.blocked), (PARRIES, s.parried as f32), (SKILL, s.skills as f32), (CALLS, s.calls as f32), (KILLS, s.kills as f32)] {
                if n > 0.0 && self.tally.key[stat].is_none() {
                    self.tally.key[stat] = Some(tick);
                }
            }
        }
    }

    fn buildings_left(&self) -> usize {
        self.world.units.iter().filter(|u| u.alive && u.kind.is_building()).count()
    }

    /// Play the drill to the end.
    pub fn run(&mut self, actor: &Mlp) {
        while !self.over() {
            self.step(actor);
        }
    }

    fn note(&mut self, i: usize, a: u8, mask: &[bool; N_ACTIONS], tick: u32) {
        let t = &mut self.tally;
        t.uses[a as usize] += 1;
        t.first_use[a as usize].get_or_insert(tick);
        let w = &self.world;
        let me = &w.units[i];
        // Heal targeting: both choices open, which did it take?
        if (a == ACT_HEAL || a == ACT_HEAL_2) && mask[ACT_HEAL as usize] && mask[ACT_HEAL_2 as usize] {
            let hurt = |j: Option<usize>| j.map_or(1.0, |j| w.units[j].hp / w.units[j].max_hp);
            let (h0, h1) = (hurt(w.aim(i, ACT_HEAL)), hurt(w.aim(i, ACT_HEAL_2)));
            // Only a real choice: one clearly more hurt than the other.
            if (h0 - h1).abs() > 0.05 {
                t.heal_choices += 1;
                let mine = if a == ACT_HEAL { h0 } else { h1 };
                if mine <= h0.min(h1) {
                    t.heal_hurt_first += 1;
                    t.key[HEAL_FIRST].get_or_insert(tick);
                }
            }
        }
        // Distance from enemy fighters.
        let near = w
            .units
            .iter()
            .filter(|u| u.alive && u.team != me.team && !u.kind.is_building())
            .map(|u| me.dist(u))
            .fold(f32::MAX, f32::min);
        if near <= 8.0 {
            t.fight_ticks += 1;
            if near > TOUCH {
                t.safe_ticks += 1;
                if t.safe_ticks >= 10 && (a == ACT_HIT_UNIT || a == ACT_HIT_BUILDING || a == ACT_SKILL) {
                    t.key[DISTANCE].get_or_insert(tick);
                }
            }
        }
    }

    fn situation(&self, i: usize) -> u8 {
        let w = &self.world;
        let me = &w.units[i];
        let mut tags = 0;
        let enemies = |pred: &dyn Fn(&Unit) -> bool, r: f32| w.units.iter().filter(|u| u.alive && u.team != me.team && pred(u) && me.dist(u) <= r).count();
        if enemies(&|u| u.kind.is_building(), 5.0) > 0 {
            tags |= NEAR_TOWER;
        }
        if me.hp < me.max_hp * 0.4 {
            tags |= HURT;
        }
        if me.pool > 0.0 && me.breath() < 0.35 {
            tags |= TIRED;
        }
        if enemies(&|u| !u.kind.is_building(), 4.0) >= 2 {
            tags |= CROWDED;
        }
        if w.light != Light::Day {
            tags |= DARK;
        }
        tags
    }

    /// What this battle showed, once it is over.
    pub fn result(&self) -> BattleResult {
        let w = &self.world;
        let us: Vec<&Unit> = self.controlled.iter().map(|&i| &w.units[i]).collect();
        let sum = |f: &dyn Fn(&UnitStats) -> f32| us.iter().map(|u| f(&u.stats)).sum::<f32>();
        let first = us[0];
        let t = &self.tally;
        let mut v = [None; N_STATS];
        v[WINS] = Some(self.won() as u8 as f32);
        v[SURVIVES] = Some(us.iter().filter(|u| u.alive).count() as f32 / us.len() as f32);
        if t.heal_choices >= 2 {
            v[HEAL_FIRST] = Some(t.heal_hurt_first as f32 / t.heal_choices as f32);
        }
        if first.role() == Role::Ranged && t.fight_ticks >= 20 {
            v[DISTANCE] = Some(t.safe_ticks as f32 / t.fight_ticks as f32);
        }
        if first.moves[crate::moves::Move::Dodge as usize] > 0.0 {
            v[DODGES] = Some(sum(&|s| s.evaded as f32));
        }
        if first.moves[crate::moves::Move::Block as usize] > 0.0 {
            v[BLOCKS] = Some(sum(&|s| s.blocked));
        }
        if first.tier >= 3 {
            v[PARRIES] = Some(sum(&|s| s.parried as f32));
        }
        if first.skill.is_some() {
            v[SKILL] = Some(sum(&|s| s.skills as f32));
        }
        if !self.defense {
            v[DESTRUCTION] = Some(w.destruction());
        }
        if first.pool > 0.0 && t.ticks > 0 {
            let winded = sum(&|s| s.winded as f32) / us.len() as f32;
            v[PACING] = Some(1.0 - winded / t.ticks as f32);
        }
        // Calls need someone to hear them: squads and defence.
        if self.defense || w.units.iter().filter(|u| u.team == first.team && u.kind == Kind::Troop).count() > 1 {
            v[CALLS] = Some(sum(&|s| s.calls as f32));
        }
        if first.heal < 5.0 {
            v[KILLS] = Some(sum(&|s| s.kills as f32));
        }
        let score = if self.defense {
            0.7 * self.won() as u8 as f32 + 0.3 * v[SURVIVES].unwrap()
        } else {
            0.4 * self.won() as u8 as f32 + 0.4 * w.destruction() + 0.2 * v[SURVIVES].unwrap()
        };
        let mut key = t.key;
        key[WINS] = self.won().then_some(w.tick.saturating_sub(1));
        BattleResult { stats: v, score, key, uses: t.uses.clone(), first_use: t.first_use.clone(), samples: t.samples.clone(), ticks: w.tick }
    }
}

// ---------------------------------------------------------------- stats

pub const N_STATS: usize = 12;
pub const WINS: usize = 0;
pub const SURVIVES: usize = 1;
pub const HEAL_FIRST: usize = 2;
pub const DISTANCE: usize = 3;
pub const DODGES: usize = 4;
pub const BLOCKS: usize = 5;
pub const PARRIES: usize = 6;
pub const SKILL: usize = 7;
pub const DESTRUCTION: usize = 8;
pub const PACING: usize = 9;
pub const CALLS: usize = 10;
pub const KILLS: usize = 11;

/// How a named stat must change to count as a new behaviour.
struct StatRule {
    key: &'static str,
    /// Least rise of the average across test drills.
    jump: f32,
    /// Least average afterwards (a rise from 0 to almost 0 is no behaviour).
    floor: f32,
    /// For counts: least ratio of after to before.
    ratio: f32,
}

const RULES: [StatRule; N_STATS] = [
    StatRule { key: "wins", jump: 0.375, floor: 0.5, ratio: 0.0 },
    StatRule { key: "survives", jump: 0.375, floor: 0.6, ratio: 0.0 },
    StatRule { key: "heal_first", jump: 0.3, floor: 0.65, ratio: 0.0 },
    StatRule { key: "distance", jump: 0.2, floor: 0.6, ratio: 0.0 },
    StatRule { key: "dodges", jump: 1.0, floor: 1.5, ratio: 2.0 },
    StatRule { key: "blocks", jump: 15.0, floor: 20.0, ratio: 2.0 },
    StatRule { key: "parries", jump: 1.0, floor: 1.5, ratio: 2.0 },
    StatRule { key: "skill", jump: 1.0, floor: 1.5, ratio: 2.0 },
    StatRule { key: "destruction", jump: 0.25, floor: 0.4, ratio: 0.0 },
    StatRule { key: "pacing", jump: 0.15, floor: 0.8, ratio: 0.0 },
    StatRule { key: "calls", jump: 2.0, floor: 2.5, ratio: 2.0 },
    StatRule { key: "kills", jump: 1.0, floor: 1.5, ratio: 1.5 },
];

/// The caption for a named stat, after "<name> ".
fn stat_text(stat: usize, spec: &ClassSpec) -> String {
    match stat {
        WINS if spec.role == Side::Defend => "learned to hold the base".into(),
        WINS => "started winning its drills".into(),
        SURVIVES => "learned to stay alive".into(),
        HEAL_FIRST => "learned to heal the most-hurt ally first".into(),
        DISTANCE => "learned to keep its distance".into(),
        DODGES => "learned to dodge incoming blows".into(),
        BLOCKS => "learned to raise its guard in time".into(),
        PARRIES => "learned to parry".into(),
        SKILL => format!("learned to use {}", spec.weapon().skill_name()),
        DESTRUCTION => "learned to tear down the base".into(),
        PACING => "learned to pace itself and keep its breath".into(),
        CALLS => "learned to call out to its squad".into(),
        _ => "learned to finish off its enemies".into(),
    }
}

/// After "<name> learned to ": a move used for the first time.
fn first_use_text(a: u8) -> Option<&'static str> {
    Some(match a {
        ACT_DODGE => "dodge",
        ACT_BLOCK => "raise its guard",
        ACT_CHARGE => "charge in",
        ACT_CLOAK => "vanish from sight",
        ACT_BLINK => "blink out of danger",
        ACT_SNARE => "lay traps",
        ACT_WARD => "shield its allies with wards",
        ACT_SPRINT => "sprint",
        ACT_SNEAK => "sneak up on enemies",
        ACT_BACKSTEP => "step back out of reach",
        ACT_STRAFE_L | ACT_STRAFE_R => "strafe around enemies",
        ACT_SHOVE => "shove enemies away",
        ACT_CROUCH => "crouch under fire",
        ACT_JUMP => "jump",
        ACT_PARRY => "parry",
        ACT_FEINT => "feint",
        ACT_GRAB => "grab and throw enemies",
        ACT_ROCK => "throw rocks",
        _ => return None,
    })
}

/// After "<name> now ": what it does more of.
fn verb(a: u8) -> Option<&'static str> {
    Some(match a {
        ACT_IDLE => "waits",
        1..=8 => return None,
        ACT_HIT_UNIT => "fights enemies",
        ACT_HIT_BUILDING => "hits buildings",
        ACT_HEAL | ACT_HEAL_2 => "heals",
        ACT_SKILL => "uses its skill",
        21..=27 => "calls to its team",
        _ => match first_use_text(a)? {
            "dodge" => "dodges",
            "raise its guard" => "blocks",
            "charge in" => "charges in",
            "vanish from sight" => "vanishes",
            "blink out of danger" => "blinks away",
            "lay traps" => "lays traps",
            "shield its allies with wards" => "raises wards",
            "sprint" => "sprints",
            "sneak up on enemies" => "sneaks",
            "step back out of reach" => "steps back",
            "strafe around enemies" => "strafes",
            "shove enemies away" => "shoves",
            "crouch under fire" => "crouches",
            "jump" => "jumps",
            "parry" => "parries",
            "feint" => "feints",
            "grab and throw enemies" => "throws enemies",
            _ => "throws rocks",
        },
    })
}

// ---------------------------------------------------------------- probes

/// What one test drill showed.
#[derive(Clone, Debug)]
pub struct BattleResult {
    pub stats: [Option<f32>; N_STATS],
    /// How well it went overall, 0 to 1.
    pub score: f32,
    pub key: [Option<u32>; N_STATS],
    pub uses: Vec<u32>,
    pub first_use: Vec<Option<u32>>,
    pub samples: Vec<Sample>,
    pub ticks: u32,
}

/// One brain through every test drill.
#[derive(Clone, Debug)]
pub struct Probe {
    pub battles: Vec<BattleResult>,
}

impl Probe {
    pub fn run(spec: &ClassSpec, actor: &Mlp, seed: u64) -> Probe {
        let battles = (0..PROBES)
            .map(|k| {
                let mut r = Run::new(spec, probe_seed(seed, k), k);
                r.run(actor);
                r.result()
            })
            .collect();
        Probe { battles }
    }

    pub fn score(&self) -> f32 {
        self.battles.iter().map(|b| b.score).sum::<f32>() / self.battles.len().max(1) as f32
    }

    /// Average of a stat over the drills where it shows.
    pub fn mean(&self, stat: usize) -> Option<f32> {
        let v: Vec<f32> = self.battles.iter().filter_map(|b| b.stats[stat]).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
    }
}

/// A new behaviour found by comparing two probes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Found {
    /// What kind of change, for "only once" bookkeeping ("heal_first",
    /// "move:33", "shift").
    pub key: String,
    /// The caption after the troop's name.
    pub text: String,
    /// How clear the change is (1 = just enough). Bigger ones win the cap.
    pub size: f32,
    /// The stat before and after (0 for surprises).
    pub before: f32,
    pub after: f32,
    /// The test drill that shows it best, and the tick it shows at.
    pub probe: usize,
    pub tick: u32,
}

/// What changed between `before` (brain `old`) and `after` (brain `new`).
/// `best` holds the highest level each named behaviour reached in earlier
/// moments: a behaviour only comes back as a moment if it beats that.
/// Biggest first.
pub fn compare(spec: &ClassSpec, before: &Probe, old: &Mlp, after: &Probe, new: &Mlp, best: &BTreeMap<String, f32>) -> Vec<Found> {
    let mut out = Vec::new();
    // Nothing counts as learned if the troop got clearly worse overall.
    if after.score() < before.score() - 0.15 {
        return out;
    }
    let pairs = || before.battles.iter().zip(&after.battles).enumerate();
    // Layer 1: named behaviours.
    for (stat, rule) in RULES.iter().enumerate() {
        let both: Vec<(usize, f32, f32)> = pairs().filter_map(|(k, (b, a))| Some((k, b.stats[stat]?, a.stats[stat]?))).collect();
        let n = both.len();
        if n < MIN_SHOWN {
            continue;
        }
        let shown = both.iter().filter(|(_, b, a)| a > b).count();
        let worse = both.iter().filter(|(_, b, a)| a < b).count();
        let (mb, ma) = (both.iter().map(|x| x.1).sum::<f32>() / n as f32, both.iter().map(|x| x.2).sum::<f32>() / n as f32);
        // Better in most drills, or in many and worse in hardly any (a troop
        // that now wins half its drills, and lost none it used to win).
        let held = shown >= MIN_SHOWN && (shown as f32 >= SHOWN_SHARE * n as f32 || (shown as f32 >= 0.4 * n as f32 && worse * 4 <= shown));
        let rose = ma - mb >= rule.jump && ma >= rule.floor && (rule.ratio == 0.0 || ma >= mb * rule.ratio);
        let beats = best.get(rule.key).is_none_or(|&b| ma >= b + rule.jump * 0.5);
        if !(held && rose && beats) {
            continue;
        }
        // The drill that shows it best, with a tick to look at.
        let (k, tick) = both
            .iter()
            .filter_map(|&(k, b, a)| Some((k, a - b, after.battles[k].key[stat]?)))
            .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap().then(y.0.cmp(&x.0)))
            .map(|(k, _, t)| (k, t))
            .unwrap_or((both[0].0, 0));
        out.push(Found { key: rule.key.into(), text: stat_text(stat, spec), size: (ma - mb) / rule.jump, before: mb, after: ma, probe: k, tick });
    }
    // Layer 2a: a move now used in most drills, hardly ever before, and in
    // drills that went at least as well.
    let mut seen_moves = Vec::new();
    for a in 12..N_ACTIONS as u8 {
        let Some(words) = first_use_text(a) else { continue };
        let key = format!("move:{}", if a == ACT_STRAFE_R { ACT_STRAFE_L } else { a });
        if best.contains_key(&key) || seen_moves.contains(&key) {
            continue;
        }
        let used_before = before.battles.iter().filter(|b| b.uses[a as usize] >= 2).count();
        let used: Vec<usize> = (0..after.battles.len()).filter(|&k| after.battles[k].uses[a as usize] >= 2).collect();
        let need = (PROBES as f32 * 0.5).ceil() as usize;
        if used.len() < need || used_before > 1 {
            continue;
        }
        let gain: f32 = used.iter().map(|&k| after.battles[k].score - before.battles[k].score).sum::<f32>() / used.len() as f32;
        if gain < -0.05 {
            continue;
        }
        let k = used[0];
        seen_moves.push(key.clone());
        out.push(Found {
            key,
            text: format!("learned to {words}"),
            size: used.len() as f32 / need as f32,
            before: used_before as f32,
            after: used.len() as f32,
            probe: k,
            tick: after.battles[k].first_use[a as usize].unwrap_or(0),
        });
    }
    // Layer 2b: the same situations, handled differently. Only when nothing
    // with a clearer name changed.
    // It must also have paid off: a new way of acting that does no better
    // is just the brain wandering (young brains change a lot).
    if out.is_empty() && after.score() >= before.score() + 0.05 {
        if let Some(f) = shift(spec, before, old, new) {
            out.push(f);
        }
    }
    out.sort_by(|a, b| b.size.partial_cmp(&a.size).unwrap().then(a.key.cmp(&b.key)));
    out
}

/// Share of situations (from the old brain's test drills) where the new
/// brain would act differently, if it is big and in most drills.
fn shift(spec: &ClassSpec, before: &Probe, old: &Mlp, new: &Mlp) -> Option<Found> {
    let mut cache = Cache::default();
    let mut differ = 0usize;
    let mut total = 0usize;
    let mut drills_changed = 0usize;
    // (action, situation) -> new minus old count, over changed situations.
    let mut gain: BTreeMap<(u8, usize), i32> = BTreeMap::new();
    let mut best_drill = (0usize, 0usize);
    for (k, b) in before.battles.iter().enumerate() {
        let mut d = 0;
        for s in &b.samples {
            let a0 = greedy_action(old, &s.obs, &s.mask, &mut cache);
            let a1 = greedy_action(new, &s.obs, &s.mask, &mut cache);
            if a0 != a1 {
                d += 1;
                for (si, (tag, _)) in SITUATIONS.iter().enumerate() {
                    if *tag == 0 || s.tags & tag != 0 {
                        *gain.entry((a1, si)).or_default() += 1;
                        *gain.entry((a0, si)).or_default() -= 1;
                    }
                }
            }
        }
        total += b.samples.len();
        differ += d;
        if b.samples.len() >= 5 && d as f32 >= 0.25 * b.samples.len() as f32 {
            drills_changed += 1;
        }
        if d > best_drill.1 {
            best_drill = (k, d);
        }
    }
    let share = differ as f32 / total.max(1) as f32;
    if share < 0.35 || (drills_changed as f32) < SHOWN_SHARE * before.battles.len() as f32 {
        return None;
    }
    // The clearest (action, situation) rise. A situation beats "anywhere"
    // only when it carries most of the rise.
    let mut pick: Option<(u8, usize, i32)> = None;
    for (&(a, si), &g) in &gain {
        if verb(a).is_none() || g < 6 {
            continue;
        }
        let weight = if si == 0 { g } else { g * 3 / 2 };
        if pick.is_none_or(|p| weight > if p.1 == 0 { p.2 } else { p.2 * 3 / 2 }) {
            pick = Some((a, si, g));
        }
    }
    let text = match pick {
        Some((ACT_SKILL, si, _)) => format!("now uses {} more{}", spec.weapon().skill_name(), SITUATIONS[si].1),
        Some((a, si, _)) => format!("now {} more{}", verb(a).unwrap(), SITUATIONS[si].1),
        None => "found a new trick".to_string(),
    };
    Some(Found { key: "shift".into(), text, size: share / 0.35, before: 0.0, after: share, probe: best_drill.0, tick: 0 })
}

// ---------------------------------------------------------------- saved

/// A learning moment as the troop keeps it. The two brains live in their
/// own file (see `MomentBrains`), so the troop file stays small.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Moment {
    pub id: u64,
    /// What it learned, after the troop's name.
    pub text: String,
    /// The player's own name for it, if they renamed it.
    #[serde(default)]
    pub title: Option<String>,
    pub key: String,
    pub before: f32,
    pub after: f32,
    pub size: f32,
    /// Training steps of the brain before and after.
    pub steps_before: u64,
    pub steps: u64,
    /// Unix seconds when it was found.
    pub when: u64,
    /// The test drill to replay: which one, its seed, and the tick to watch.
    pub probe: usize,
    pub seed: u64,
    pub tick: u32,
    /// The class spec the drill was played with.
    pub spec: ClassSpec,
    /// The last video saved of it, for sharing again.
    #[serde(default)]
    pub video: Option<String>,
}

impl Moment {
    /// "learned to keep its distance", or the player's name for it.
    pub fn caption(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.text)
    }

    /// The drill this moment happened in, ready to play.
    pub fn replay(&self) -> Run {
        Run::new(&self.spec, self.seed, self.probe)
    }
}

/// The brain before and after a moment, for replays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MomentBrains {
    pub before: Mlp,
    pub after: Mlp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::BaseClass;

    fn brain(seed: u64) -> Mlp {
        let t = crate::troop::Troop::new(1, "T".into(), BaseClass::Healer.spec(), seed);
        t.actor
    }

    #[test]
    fn probes_replay_exactly() {
        let spec = BaseClass::Healer.spec();
        let a = brain(3);
        let p1 = Probe::run(&spec, &a, 42);
        let p2 = Probe::run(&spec, &a, 42);
        for (x, y) in p1.battles.iter().zip(&p2.battles) {
            assert_eq!(x.ticks, y.ticks);
            assert_eq!(x.stats, y.stats);
        }
        // A replay of one drill ends on the same world as its probe.
        let mut r = Run::new(&spec, probe_seed(42, 1), 1);
        r.run(&a);
        assert_eq!(r.world.tick, p1.battles[1].ticks);
    }

    #[test]
    fn same_brain_finds_nothing() {
        let spec = BaseClass::Archer.spec();
        let a = brain(5);
        let p = Probe::run(&spec, &a, 7);
        assert!(compare(&spec, &p, &a, &p, &a, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn heal_first_is_caught() {
        // Two made-up results: before, heals went to the less hurt ally;
        // after, to the more hurt one, in most drills.
        let spec = BaseClass::Healer.spec();
        let a = brain(9);
        let mut before = Probe::run(&spec, &a, 11);
        let mut after = before.clone();
        for (k, (b, x)) in before.battles.iter_mut().zip(after.battles.iter_mut()).enumerate() {
            b.stats[HEAL_FIRST] = Some(0.3);
            x.stats[HEAL_FIRST] = Some(if k == 0 { 0.2 } else { 0.9 });
            x.key[HEAL_FIRST] = Some(40 + k as u32);
        }
        let f = compare(&spec, &before, &a, &after, &a, &BTreeMap::new());
        assert_eq!(f[0].key, "heal_first");
        assert_eq!(f[0].text, "learned to heal the most-hurt ally first");
        assert!(f[0].probe != 0);
        // Already learned to that level: not a moment again.
        let best = BTreeMap::from([("heal_first".to_string(), 0.85)]);
        assert!(compare(&spec, &before, &a, &after, &a, &best).iter().all(|f| f.key != "heal_first"));
    }

    #[test]
    fn luck_in_one_drill_is_not_a_moment() {
        let spec = BaseClass::Healer.spec();
        let a = brain(9);
        let mut before = Probe::run(&spec, &a, 11);
        let mut after = before.clone();
        for (k, (b, x)) in before.battles.iter_mut().zip(after.battles.iter_mut()).enumerate() {
            b.stats[HEAL_FIRST] = Some(0.3);
            // One drill went wildly better, the rest the same.
            x.stats[HEAL_FIRST] = Some(if k == 2 { 1.0 } else { 0.3 });
        }
        let f = compare(&spec, &before, &a, &after, &a, &BTreeMap::new());
        assert!(f.iter().all(|f| f.key != "heal_first"));
    }

    #[test]
    fn first_move_and_shift() {
        let spec = BaseClass::Fighter.spec();
        let a = brain(1);
        let before = Probe::run(&spec, &a, 3);
        let mut after = before.clone();
        for b in after.battles.iter_mut() {
            b.uses[ACT_SHOVE as usize] = 5;
            b.first_use[ACT_SHOVE as usize] = Some(30);
        }
        let f = compare(&spec, &before, &a, &after, &a, &BTreeMap::new());
        assert!(f.iter().any(|f| f.text == "learned to shove enemies away"), "{f:?}");
        // A different brain acts differently in the same situations.
        let b = brain(77);
        let g = shift(&spec, &before, &a, &b);
        if let Some(g) = g {
            assert!(g.text.starts_with("now ") || g.text == "found a new trick");
        }
    }
}
