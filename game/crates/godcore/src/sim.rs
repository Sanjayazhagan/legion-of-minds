//! The deterministic battle simulation. Training drills, battles and
//! tournaments all run on this one `World`, so what a troop learns is exactly
//! what happens when it fights.
//!
//! Determinism: units update in index order, only +, -, *, / and sqrt are used
//! (all exactly rounded by IEEE 754), and randomness comes from the seeded
//! `Rng`. The same layout, models and actions give the same battle anywhere.

use crate::classes::ClassSpec;
use crate::moves::{Move, N_MOVES};
use crate::weapons::{mastery_dmg, mastery_skill_cd, mastery_skill_power, Skill, Weapon};
use serde::{Deserialize, Serialize};

pub const ARENA: f32 = 32.0;
/// Bumped whenever battle maths, observations or actions change: battles
/// recorded under another version will not replay the same.
pub const SIM_VERSION: u32 = 7;
pub const OBS_DIM: usize = 109;
/// Where the teamwork part of the view starts (the two allies' roles,
/// threats and actions, sim version 4).
pub const OBS_TEAM: usize = 74;
/// Where the senses part starts (sim version 6), and the calls part (7).
pub const OBS_SENSES: usize = 90;
pub const OBS_CALLS: usize = 100;
pub const N_ACTIONS: usize = 24;
/// The first observation features and actions, as they were before dodge,
/// block and skills (brains from older saves are grown from these).
pub const OBS_DIM_V1: usize = 47;
pub const N_ACTIONS_V1: usize = 12;

pub const ACT_IDLE: u8 = 0;
pub const ACT_HIT_UNIT: u8 = 9;
pub const ACT_HIT_BUILDING: u8 = 10;
/// Heal the nearest ally (`ACT_HEAL`) or the second nearest (`ACT_HEAL_2`).
pub const ACT_HEAL: u8 = 11;
/// Roll away from the nearest threat, untouchable for a moment.
pub const ACT_DODGE: u8 = 12;
/// Raise the guard: take much less damage for a while, no attacks, half speed.
pub const ACT_BLOCK: u8 = 13;
/// Use the weapon's skill.
pub const ACT_SKILL: u8 = 14;
/// Class moves (see `moves`): only classes with the move can use them.
pub const ACT_CHARGE: u8 = 15;
pub const ACT_CLOAK: u8 = 16;
pub const ACT_BLINK: u8 = 17;
pub const ACT_SNARE: u8 = 18;
pub const ACT_WARD: u8 = 19;
pub const ACT_HEAL_2: u8 = 20;
/// Calls to teammates (sim version 7). What each one comes to mean is up to
/// the team: troops learn it in training. "Focus" points at the enemy the
/// caller is looking at; "fall back" and "help" point at the caller.
pub const ACT_CALL_FOCUS: u8 = 21;
pub const ACT_CALL_BACK: u8 = 22;
pub const ACT_CALL_HELP: u8 = 23;
pub const N_CALLS: usize = 3;

const DODGE_DIST: f32 = 2.4;
const DODGE_INVULN: u32 = 6;
const DODGE_CD: u32 = 45;
const BLOCK_TICKS: u32 = 12;
const BLOCK_CD: u32 = 30;
const CHARGE_REACH: f32 = 6.0;
const CHARGE_DIST: f32 = 3.5;
const CHARGE_CD: u32 = 90;
const CLOAK_TICKS: u32 = 60;
const CLOAK_CD: u32 = 180;
const BLINK_INVULN: u32 = 4;
const BLINK_CD: u32 = 120;
const SNARE_CD: u32 = 100;
const SNARE_LIFE: u32 = 450;
const SNARE_RADIUS: f32 = 0.9;
const MAX_SNARES: usize = 2;
const WARD_RADIUS: f32 = 3.0;

// Senses (sim version 6). Trained troops no longer know everything: they see
// in a cone ahead of them, walls block the view, they hear fights they can't
// see, and they remember where an enemy was for a moment after losing sight.
// Like people, they also need a moment to react to something new, have to
// turn to face what they hit, and ranged shots sometimes miss.
/// How far a troop can see.
pub const SIGHT_RANGE: f32 = 14.0;
/// Cosine of half the sight cone (120 degrees wide).
pub const SIGHT_COS: f32 = 0.5;
/// Closer than this a troop always notices you, even from behind.
pub const TOUCH: f32 = 2.5;
/// How far the sound of a fight carries.
pub const HEAR_RANGE: f32 = 12.0;
/// Ticks an unseen enemy is remembered (3 sim seconds).
pub const MEMORY: u32 = 60;
/// Ticks between first seeing something and being able to act on it.
pub const REACT: u32 = 4;
/// Turning speed: 0.5 radians a tick (a half turn takes about 6 ticks).
const TURN_COS: f32 = 0.877_582_56;
const TURN_SIN: f32 = 0.479_425_54;
/// A troop must face its target this squarely to strike (within 45 degrees).
const FACE_COS: f32 = 0.707_106_77;
const NEVER: u32 = u32::MAX;
/// How far a call carries (sim version 7). Walls don't stop it.
pub const CALL_RANGE: f32 = 12.0;
/// Ticks a call stays in a listener's mind (1.5 sim seconds).
pub const CALL_LIFE: u32 = 30;
/// Ticks a troop waits before it can call again (0.5 s).
pub const CALL_GAP: u32 = 10;
const WARD_TICKS: u32 = 45;
const WARD_CD: u32 = 160;

/// A class move's cooldown: weaker (mixed-in) versions recharge slower.
fn move_cd(base: u32, power: f32) -> u32 {
    (base as f32 / power).round() as u32
}
/// Enemy units this close count as a threat (for dodge and block).
const THREAT_RANGE: f32 = 3.0;

const D: f32 = 0.707_106_77;
/// Move directions for actions 1..=8 (E, NE, N, NW, W, SW, S, SE).
pub const DIRS: [(f32, f32); 8] = [
    (1.0, 0.0),
    (D, D),
    (0.0, 1.0),
    (-D, D),
    (-1.0, 0.0),
    (-D, -D),
    (0.0, -1.0),
    (D, -D),
];

/// Distances in observations are divided by this.
const OBS_SCALE: f32 = 16.0;

// Defender stats.
pub const GUARD_HP: f32 = 40.0;
const GUARD_SPEED: f32 = 0.25;
const GUARD_RANGE: f32 = 1.2;
const GUARD_DMG: f32 = 3.0;
const GUARD_CD: u32 = 8;
const GUARD_AGGRO: f32 = 6.0;
const GUARD_LEASH: f32 = 9.0;
pub const TOWER_HP: f32 = 200.0;
pub const TOWER_RANGE: f32 = 6.0;
const TOWER_DMG: f32 = 4.0;
const TOWER_CD: u32 = 10;
const TOWER_RADIUS: f32 = 1.0;
pub const CORE_HP: f32 = 300.0;
const CORE_RADIUS: f32 = 1.5;
const TROOP_RADIUS: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Team {
    Attack,
    Defend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Troop,
    Guard,
    Tower,
    Core,
}

impl Kind {
    pub fn is_building(self) -> bool {
        matches!(self, Kind::Tower | Kind::Core)
    }
}

/// Who decides what a unit does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Actions come from outside (a trained policy or the trainer).
    External,
    /// Hand-written "guards first, then buildings" fighter.
    Scripted,
    /// Built-in defender behaviour.
    Defender,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitStats {
    pub unit_dmg: f32,
    pub building_dmg: f32,
    pub heal: f32,
    pub kills: u32,
    pub skills: u32,
    pub dodges: u32,
    /// Hits avoided by dodging.
    pub evaded: u32,
    /// Damage soaked up by blocking.
    pub blocked: f32,
    /// Ranged shots that missed.
    pub missed: u32,
    /// Calls made to teammates.
    pub calls: u32,
}

#[derive(Clone, Debug)]
pub struct Unit {
    pub team: Team,
    pub kind: Kind,
    pub control: Control,
    pub x: f32,
    pub y: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub radius: f32,
    pub speed: f32,
    pub range: f32,
    pub dmg: f32,
    pub cd_max: u32,
    pub cd: u32,
    pub heal: f32,
    pub heal_range: f32,
    pub home: (f32, f32),
    pub alive: bool,
    pub stats: UnitStats,
    /// Free tag for the caller (e.g. which roster troop this is).
    pub tag: usize,
    pub last_action: u8,
    /// Drawing hint only (e.g. a cannon is a short, heavy tower). Not simulated.
    pub style: u8,
    /// Attacks also hit enemies this close to the target, for half damage.
    pub splash: f32,
    /// Most units a multi-target talent reaches (from the troop's level).
    pub talents: u8,
    pub weapon: Option<Weapon>,
    pub skill: Option<Skill>,
    pub skill_cd: u32,
    pub skill_cd_max: u32,
    /// Skill strength multiplier (weapon mastery).
    pub skill_power: f32,
    /// Strength of each class move (0 = not in this troop's kit).
    pub moves: [f32; N_MOVES],
    /// Cooldown of each class move.
    pub move_cd: [u32; N_MOVES],
    pub block_ticks: u32,
    /// Untargetable until it strikes.
    pub cloak: u32,
    /// Shielded by an ally's ward: damage taken is cut by `ward_power`.
    pub ward: u32,
    pub ward_power: f32,
    /// Share of damage still taken while blocking.
    pub block_taken: f32,
    /// Stunned units skip their turns.
    pub stun: u32,
    /// Dodging units can't be hit.
    pub invuln: u32,
    /// Which way the unit looks (unit length). Trained troops only see ahead.
    pub facing: (f32, f32),
}

impl Unit {
    pub fn dist(&self, o: &Unit) -> f32 {
        let dx = o.x - self.x;
        let dy = o.y - self.y;
        (dx * dx + dy * dy).sqrt()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Melee,
    Arrow,
    TowerShot,
    GuardHit,
    Heal,
    /// A splash attack landing (drawn as a burst at the target).
    Blast,
    Death,
    /// A roll: `from` where it started, `to` where it ended.
    Dodge,
    /// A hit landing on a raised guard.
    Blocked,
    /// A unit got stunned.
    Stun,
    /// A weapon skill: `from` the user, `to` the target or centre.
    Skill(Skill),
    /// A rush at an enemy: `from` start, `to` where it stopped.
    Charge,
    /// A unit vanished.
    Cloak,
    /// A teleport: `from` start, `to` end.
    Blink,
    /// A trap was set (at `to`).
    TrapSet,
    /// A trap went off (at `to`).
    TrapHit,
    /// A ward went up around `from`.
    Ward,
    /// A call to teammates (0 focus, 1 fall back, 2 help): `from` the
    /// caller, `to` the spot it points at.
    Call(u8),
}

/// A hidden snare on the ground.
#[derive(Clone, Copy, Debug)]
pub struct Trap {
    pub x: f32,
    pub y: f32,
    pub team: Team,
    pub owner: usize,
    pub power: f32,
    pub ttl: u32,
}

/// A unit's job in a team, as its teammates see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Holds the front: melee with a block.
    Tank,
    /// Melee damage without a block.
    Striker,
    /// Fights from range.
    Ranged,
    /// Heals.
    Healer,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Tank, Role::Striker, Role::Ranged, Role::Healer];

    /// The role a troop of this class mix plays.
    pub fn of_spec(spec: &ClassSpec) -> Role {
        let mut w = World::new(1);
        let i = w.add_troop(spec, 1.0, 1.0, Control::External, 0);
        w.units[i].role()
    }
}

impl Unit {
    pub fn role(&self) -> Role {
        if self.heal >= 5.0 {
            Role::Healer
        } else if self.range >= 2.5 {
            Role::Ranged
        } else if self.moves[Move::Block as usize] > 0.0 {
            Role::Tank
        } else {
            Role::Striker
        }
    }
}

/// What happened this tick, for drawing.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub kind: EventKind,
    pub from: (f32, f32),
    pub to: (f32, f32),
}

#[derive(Clone, Debug)]
pub struct World {
    pub tick: u32,
    pub max_ticks: u32,
    pub units: Vec<Unit>,
    pub events: Vec<Event>,
    pub traps: Vec<Trap>,
    total_building_hp: f32,
    /// What each trained troop remembers of each unit, `n * n` entries.
    mem: Vec<Mem>,
    /// Where fights could be heard last tick.
    noise: Vec<(f32, f32)>,
    /// Recent calls to teammates.
    calls: Vec<Call>,
}

/// A call one troop made to its team.
#[derive(Clone, Copy, Debug)]
struct Call {
    team: Team,
    from: usize,
    kind: u8,
    /// Where the caller stood.
    cx: f32,
    cy: f32,
    /// The spot it points at.
    x: f32,
    y: f32,
    tick: u32,
}

/// A troop's memory of one unit: where it was and how hurt, when it was
/// last seen, and when this sighting started (for the reaction delay).
#[derive(Clone, Copy, Debug)]
struct Mem {
    x: f32,
    y: f32,
    hp: f32,
    seen: u32,
    /// The tick from which the troop can act on it (reaction time).
    ready: u32,
}

const FORGOTTEN: Mem = Mem { x: 0.0, y: 0.0, hp: 0.0, seen: NEVER, ready: 0 };

/// What a troop knows about a unit: where (as remembered), how hurt, and how
/// fresh the knowledge is (1 = in sight now).
#[derive(Clone, Copy, Debug)]
struct Known {
    j: usize,
    x: f32,
    y: f32,
    hp: f32,
    fresh: f32,
    d: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    /// Every building destroyed.
    Victory,
    /// All attacking troops dead.
    Wiped,
    /// Time ran out.
    Timeout,
}

impl World {
    pub fn new(max_ticks: u32) -> Self {
        World {
            tick: 0,
            max_ticks,
            units: Vec::new(),
            events: Vec::new(),
            traps: Vec::new(),
            total_building_hp: 0.0,
            mem: Vec::new(),
            noise: Vec::new(),
            calls: Vec::new(),
        }
    }

    fn push(&mut self, u: Unit) -> usize {
        if u.kind.is_building() {
            self.total_building_hp += u.max_hp;
        }
        self.units.push(u);
        self.units.len() - 1
    }

    fn blank(team: Team, kind: Kind, control: Control, x: f32, y: f32, hp: f32) -> Unit {
        Unit {
            team,
            kind,
            control,
            x,
            y,
            hp,
            max_hp: hp,
            radius: TROOP_RADIUS,
            speed: 0.0,
            range: 0.0,
            dmg: 0.0,
            cd_max: 0,
            cd: 0,
            heal: 0.0,
            heal_range: 0.0,
            home: (x, y),
            alive: true,
            stats: UnitStats::default(),
            tag: 0,
            last_action: 0,
            style: 0,
            splash: 0.0,
            talents: crate::classes::TALENT_MAX,
            weapon: None,
            skill: None,
            skill_cd: 0,
            skill_cd_max: 0,
            skill_power: 1.0,
            moves: [0.0; N_MOVES],
            move_cd: [0; N_MOVES],
            block_ticks: 0,
            cloak: 0,
            ward: 0,
            ward_power: 0.0,
            block_taken: 1.0,
            stun: 0,
            invuln: 0,
            facing: (0.0, 1.0),
        }
    }

    pub fn add_troop(
        &mut self,
        spec: &ClassSpec,
        x: f32,
        y: f32,
        control: Control,
        tag: usize,
    ) -> usize {
        self.add_troop_on(Team::Attack, spec, x, y, control, tag)
    }

    /// A trained troop on either side. Defending troops replace guards.
    pub fn add_troop_on(
        &mut self,
        team: Team,
        spec: &ClassSpec,
        x: f32,
        y: f32,
        control: Control,
        tag: usize,
    ) -> usize {
        let mut u = Self::blank(team, Kind::Troop, control, x, y, spec.hp);
        // The class's numbers already assume its own weapon; another weapon
        // changes them by the ratio between the two.
        let w = spec.weapon();
        let native = Weapon::default_for(spec.main_class());
        let (ws, ns) = (w.stats(), native.stats());
        let m = spec.mastery;
        let same = w == native;
        let ratio = |a: f32, b: f32| if same { 1.0 } else { a / b };
        u.speed = spec.speed * ratio(ws.speed, ns.speed);
        u.range = if same || ws.ranged == (spec.range > 2.5) { spec.range } else { ws.range };
        u.dmg = spec.dmg * ratio(ws.dmg, ns.dmg) * mastery_dmg(m);
        u.cd_max = if same { spec.cooldown } else { (spec.cooldown as f32 * ws.cooldown / ns.cooldown).round().max(1.0) as u32 };
        u.heal = spec.heal;
        u.heal_range = spec.heal_range;
        if w == Weapon::HolyWand && native != Weapon::HolyWand {
            if u.heal > 0.0 {
                u.heal *= 1.3;
            } else {
                u.heal = 2.0;
                u.heal_range = 3.5;
            }
        }
        u.splash = spec.splash;
        u.talents = spec.talents;
        u.weapon = Some(w);
        u.skill = Some(ws.skill);
        u.skill_cd_max = (ws.skill_cd as f32 * mastery_skill_cd(m)).round() as u32;
        u.skill_power = mastery_skill_power(m);
        for m in Move::ALL {
            u.moves[m as usize] = spec.move_power(m);
        }
        // A mixed-in block keeps less of the weapon's protection.
        u.block_taken = 1.0 - (1.0 - ws.block_taken) * u.moves[Move::Block as usize];
        u.tag = tag;
        // Raiders look towards the middle of the map, defenders out from it.
        let c = ARENA * 0.5;
        let (dx, dy) = if team == Team::Attack { (c - x, c - y) } else { (x - c, y - c) };
        let d = (dx * dx + dy * dy).sqrt();
        if d > 1e-4 {
            u.facing = (dx / d, dy / d);
        }
        self.push(u)
    }

    pub fn add_guard(&mut self, x: f32, y: f32) -> usize {
        let mut u = Self::blank(Team::Defend, Kind::Guard, Control::Defender, x, y, GUARD_HP);
        u.speed = GUARD_SPEED;
        u.range = GUARD_RANGE;
        u.dmg = GUARD_DMG;
        u.cd_max = GUARD_CD;
        self.push(u)
    }

    pub fn add_tower(&mut self, x: f32, y: f32) -> usize {
        let mut u = Self::blank(Team::Defend, Kind::Tower, Control::Defender, x, y, TOWER_HP);
        u.radius = TOWER_RADIUS;
        u.range = TOWER_RANGE;
        u.dmg = TOWER_DMG;
        u.cd_max = TOWER_CD;
        self.push(u)
    }

    pub fn add_core(&mut self, x: f32, y: f32) -> usize {
        let mut u = Self::blank(Team::Defend, Kind::Core, Control::Defender, x, y, CORE_HP);
        u.radius = CORE_RADIUS;
        self.push(u)
    }

    /// Recompute the destruction baseline after building HP was changed.
    pub fn recount_buildings(&mut self) {
        self.total_building_hp = self
            .units
            .iter()
            .filter(|u| u.kind.is_building())
            .map(|u| u.max_hp)
            .sum();
    }

    // ------------------------------------------------------------ queries

    pub fn outcome(&self) -> Outcome {
        let buildings = self.units.iter().any(|u| u.alive && u.kind.is_building());
        let troops = self
            .units
            .iter()
            .any(|u| u.alive && u.team == Team::Attack && u.kind == Kind::Troop);
        if !buildings {
            Outcome::Victory
        } else if !troops {
            Outcome::Wiped
        } else if self.tick >= self.max_ticks {
            Outcome::Timeout
        } else {
            Outcome::Running
        }
    }

    /// Share of total building HP destroyed, 0..=1.
    pub fn destruction(&self) -> f32 {
        if self.total_building_hp <= 0.0 {
            return 1.0;
        }
        let left: f32 = self
            .units
            .iter()
            .filter(|u| u.kind.is_building())
            .map(|u| u.hp.max(0.0))
            .sum();
        1.0 - left / self.total_building_hp
    }

    pub fn core_destroyed(&self) -> bool {
        self.units.iter().any(|u| u.kind == Kind::Core && !u.alive)
    }

    /// Clash-style stars: 50% destruction, core down, everything down.
    pub fn stars(&self) -> u32 {
        let d = self.destruction();
        (d >= 0.5) as u32 + self.core_destroyed() as u32 + (d >= 0.999) as u32
    }

    /// Nearest alive unit matching `pred`, measured to its edge for buildings.
    fn nearest(&self, i: usize, pred: impl Fn(&Unit) -> bool) -> Option<(usize, f32)> {
        let me = &self.units[i];
        let mut best: Option<(usize, f32)> = None;
        for (j, u) in self.units.iter().enumerate() {
            if j == i || !u.alive || !pred(u) || (u.cloak > 0 && u.team != me.team) {
                continue;
            }
            let d = me.dist(u) - if u.kind.is_building() { u.radius } else { 0.0 };
            if best.map_or(true, |(_, bd)| d < bd) {
                best = Some((j, d));
            }
        }
        best
    }

    fn nearest_k(&self, i: usize, k: usize, pred: impl Fn(&Unit) -> bool) -> Vec<(usize, f32)> {
        let me = &self.units[i];
        let mut v: Vec<(usize, f32)> = self
            .units
            .iter()
            .enumerate()
            .filter(|(j, u)| *j != i && u.alive && pred(u) && !(u.cloak > 0 && u.team != me.team))
            .map(|(j, u)| (j, me.dist(u)))
            .collect();
        // Stable sort with index tie-break keeps this deterministic.
        v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0)));
        v.truncate(k);
        v
    }

    /// The ally a heal would land on: the nearest (slot 0) or second
    /// nearest (slot 1) ally, the same two the troop sees, if hurt and in
    /// reach. The troop picks whom to heal; nothing picks for it.
    fn heal_target(&self, i: usize, slot: usize) -> Option<usize> {
        let me = &self.units[i];
        if me.heal <= 0.0 {
            return None;
        }
        let team = me.team;
        let al = self.nearest_k(i, 2, |u| u.team == team && !u.kind.is_building());
        al.get(slot)
            .filter(|&&(j, d)| d <= me.heal_range && self.units[j].hp < self.units[j].max_hp)
            .map(|&(j, _)| j)
    }

    /// Keep at most the troop's talent reach of `units`, best first by `key`
    /// (lower first, index breaks ties so it stays deterministic).
    fn reach(&self, i: usize, mut units: Vec<usize>, key: impl Fn(usize) -> f32) -> Vec<usize> {
        let mut v: Vec<(usize, f32)> = units.drain(..).map(|k| (k, key(k))).collect();
        v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0)));
        v.truncate(self.units[i].talents as usize);
        v.into_iter().map(|(k, _)| k).collect()
    }

    fn unit_target(&self, i: usize) -> Option<usize> {
        let me = &self.units[i];
        let team = me.team;
        self.nearest_noticed(i, |u| u.team != team && !u.kind.is_building())
            .filter(|&(_, d)| d <= me.range)
            .map(|b| b.0)
    }

    fn building_target(&self, i: usize) -> Option<usize> {
        let me = &self.units[i];
        let team = me.team;
        self.nearest_noticed(i, |u| u.team != team && u.kind.is_building())
            .filter(|&(_, d)| d <= me.range)
            .map(|b| b.0)
    }

    /// Which actions do something right now. Moving and idling always do.
    pub fn mask(&self, i: usize) -> [bool; N_ACTIONS] {
        let mut m = [false; N_ACTIONS];
        for v in m.iter_mut().take(9) {
            *v = true;
        }
        let me = &self.units[i];
        if me.stun > 0 {
            // Stunned: nothing to decide.
            let mut idle = [false; N_ACTIONS];
            idle[ACT_IDLE as usize] = true;
            return idle;
        }
        if me.block_ticks > 0 {
            // Guard up: can only shuffle about.
            return m;
        }
        if me.cd == 0 {
            m[ACT_HIT_UNIT as usize] = me.dmg > 0.0 && self.unit_target(i).is_some();
            m[ACT_HIT_BUILDING as usize] = me.dmg > 0.0 && self.building_target(i).is_some();
            m[ACT_HEAL as usize] = self.heal_target(i, 0).is_some();
            m[ACT_HEAL_2 as usize] = self.heal_target(i, 1).is_some();
        }
        if me.kind == Kind::Troop {
            let threat = self.threat(i).is_some();
            let ready = |mv: Move| me.moves[mv as usize] > 0.0 && me.move_cd[mv as usize] == 0;
            m[ACT_DODGE as usize] = ready(Move::Dodge) && threat;
            m[ACT_BLOCK as usize] = ready(Move::Block) && threat;
            m[ACT_SKILL as usize] = me.skill_cd == 0 && self.skill_ready(i);
            m[ACT_CHARGE as usize] = ready(Move::Charge) && self.charge_target(i).is_some();
            m[ACT_CLOAK as usize] = ready(Move::Cloak) && me.cloak == 0 && threat;
            m[ACT_BLINK as usize] = ready(Move::Blink) && threat;
            m[ACT_SNARE as usize] = ready(Move::Snare)
                && self.traps.iter().filter(|t| t.owner == i).count() < MAX_SNARES
                && self.nearest_noticed(i, |u| u.team != me.team && !u.kind.is_building()).is_some_and(|(_, d)| d <= 8.0);
            m[ACT_WARD as usize] = ready(Move::Ward) && self.ward_useful(i);
            let tick = self.tick;
            if self.anyone_to_call(i) && !self.calls.iter().any(|c| c.from == i && tick - c.tick < CALL_GAP) {
                for a in [ACT_CALL_FOCUS, ACT_CALL_BACK, ACT_CALL_HELP] {
                    m[a as usize] = true;
                }
            }
        }
        m
    }

    /// The nearest thing that can hurt this unit right now: an enemy unit
    /// within `THREAT_RANGE`, or an enemy tower it stands in range of.
    fn threat(&self, i: usize) -> Option<usize> {
        let me = &self.units[i];
        let mut best: Option<(usize, f32)> = None;
        for (j, u) in self.units.iter().enumerate() {
            if !u.alive || u.team == me.team || u.kind == Kind::Core || u.cloak > 0 {
                continue;
            }
            let d = me.dist(u);
            let reach = if u.kind == Kind::Tower { u.range } else { THREAT_RANGE };
            if d <= reach && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((j, d));
            }
        }
        best.map(|b| b.0)
    }

    /// An enemy unit worth charging: in sight, not already in reach.
    fn charge_target(&self, i: usize) -> Option<usize> {
        let me = &self.units[i];
        let team = me.team;
        self.nearest_noticed(i, |u| u.team != team && !u.kind.is_building())
            .filter(|&(_, d)| d > me.range && d <= CHARGE_REACH)
            .map(|b| b.0)
    }

    /// A ward helps when an ally nearby (or this unit) is under threat.
    fn ward_useful(&self, i: usize) -> bool {
        let me = &self.units[i];
        (0..self.units.len()).any(|k| {
            let u = &self.units[k];
            u.alive && u.team == me.team && u.kind == Kind::Troop && me.dist(u) <= WARD_RADIUS && u.ward == 0 && self.threat(k).is_some()
        })
    }

    /// Enemies within `r` of (x, y), buildings measured to their edge.
    fn enemies_near(&self, team: Team, x: f32, y: f32, r: f32, buildings: bool) -> Vec<usize> {
        (0..self.units.len())
            .filter(|&k| {
                let u = &self.units[k];
                if !u.alive || u.team == team || (u.kind.is_building() && !buildings) {
                    return false;
                }
                let (dx, dy) = (u.x - x, u.y - y);
                let d = (dx * dx + dy * dy).sqrt() - if u.kind.is_building() { u.radius } else { 0.0 };
                d <= r
            })
            .collect()
    }

    fn shadowstep_target(&self, i: usize) -> Option<usize> {
        let team = self.units[i].team;
        self.nearest_noticed(i, |u| u.team != team && !u.kind.is_building())
            .filter(|&(_, d)| d <= 6.0)
            .map(|b| b.0)
    }

    /// Whether using the skill now would do anything.
    fn skill_ready(&self, i: usize) -> bool {
        let me = &self.units[i];
        let (team, x, y) = (me.team, me.x, me.y);
        match me.skill {
            None => false,
            Some(Skill::ShieldBash) | Some(Skill::Volley) => self.unit_target(i).is_some(),
            Some(Skill::Whirlwind) => !self.enemies_near(team, x, y, 2.2, true).is_empty(),
            Some(Skill::GroundSlam) => !self.enemies_near(team, x, y, 2.0, false).is_empty(),
            Some(Skill::Shadowstep) => self.shadowstep_target(i).is_some(),
            Some(Skill::Fireball) => self.unit_target(i).is_some() || self.building_target(i).is_some(),
            Some(Skill::HealingNova) => self.units.iter().any(|u| {
                u.alive && u.team == team && u.kind == Kind::Troop && u.max_hp - u.hp >= 5.0 && me.dist(u) <= 3.5
            }),
        }
    }

    // ------------------------------------------------------------ senses

    /// Trained troops have senses; towers, guards and scripted troops still
    /// know where everyone is.
    pub fn has_senses(&self, i: usize) -> bool {
        let u = &self.units[i];
        u.kind == Kind::Troop && u.control == Control::External
    }

    /// Whether troop `i` can see unit `j` right now: close enough to touch,
    /// or in its sight cone with no building in the way.
    pub fn sees(&self, i: usize, j: usize) -> bool {
        let (me, u) = (&self.units[i], &self.units[j]);
        if !u.alive || (u.cloak > 0 && u.team != me.team) {
            return false;
        }
        let (dx, dy) = (u.x - me.x, u.y - me.y);
        let c = (dx * dx + dy * dy).sqrt();
        let edge = if u.kind.is_building() { u.radius } else { 0.0 };
        if c - edge <= TOUCH {
            return true;
        }
        if c - edge > SIGHT_RANGE || me.facing.0 * dx + me.facing.1 * dy < SIGHT_COS * c - edge {
            return false;
        }
        for (k, b) in self.units.iter().enumerate() {
            if k == j || !b.alive || !b.kind.is_building() {
                continue;
            }
            let (bx, by) = (b.x - me.x, b.y - me.y);
            let t = (bx * dx + by * dy) / (c * c);
            if t <= 0.0 || t >= 1.0 {
                continue;
            }
            let (px, py) = (bx - t * dx, by - t * dy);
            if px * px + py * py < 0.81 * b.radius * b.radius {
                return false;
            }
        }
        true
    }

    /// After every tick: what each trained troop sees goes into its memory.
    fn update_senses(&mut self) {
        let n = self.units.len();
        let first_look = self.mem.len() != n * n;
        if first_look {
            self.mem = vec![FORGOTTEN; n * n];
        }
        let tick = self.tick;
        for i in 0..n {
            if !self.units[i].alive || !self.has_senses(i) {
                continue;
            }
            for j in 0..n {
                if j == i || self.units[j].team == self.units[i].team || !self.sees(i, j) {
                    continue;
                }
                let u = &self.units[j];
                let (x, y, hp, building) = (u.x, u.y, u.hp / u.max_hp, u.kind.is_building());
                let m = &mut self.mem[i * n + j];
                // Something new (or long forgotten) takes a moment to react to;
                // what is in view when the battle starts is already noticed.
                if m.seen == NEVER || (!building && tick > m.seen + MEMORY) {
                    m.ready = if first_look { 0 } else { tick + REACT };
                }
                *m = Mem { x, y, hp, seen: tick, ready: m.ready };
            }
        }
    }

    /// What troop `i` knows of unit `j`. Units without senses, and allies,
    /// are known exactly; enemies only as remembered.
    fn knows(&self, i: usize, j: usize) -> Option<Known> {
        let (me, u) = (&self.units[i], &self.units[j]);
        if !u.alive || (u.cloak > 0 && u.team != me.team) {
            return None;
        }
        let n = self.units.len();
        let (x, y, hp, fresh) = if !self.has_senses(i) || u.team == me.team {
            (u.x, u.y, u.hp / u.max_hp, 1.0)
        } else if self.mem.len() != n * n {
            if !self.sees(i, j) {
                return None;
            }
            (u.x, u.y, u.hp / u.max_hp, 1.0)
        } else {
            let m = self.mem[i * n + j];
            if m.seen == NEVER || self.tick < m.ready {
                return None;
            }
            let age = self.tick.saturating_sub(m.seen + 1);
            let fresh = if age == 0 {
                1.0
            } else if u.kind.is_building() {
                0.5
            } else if age >= MEMORY {
                return None;
            } else {
                1.0 - age as f32 / MEMORY as f32
            };
            (m.x, m.y, m.hp, fresh)
        };
        let (dx, dy) = (x - me.x, y - me.y);
        Some(Known { j, x, y, hp, fresh, d: (dx * dx + dy * dy).sqrt() })
    }

    /// The `k` nearest units matching `pred` that troop `i` knows about,
    /// by where it thinks they are.
    fn known_k(&self, i: usize, k: usize, pred: impl Fn(&Unit) -> bool) -> Vec<Known> {
        let mut v: Vec<Known> = (0..self.units.len())
            .filter(|&j| j != i && pred(&self.units[j]))
            .filter_map(|j| self.knows(i, j))
            .collect();
        v.sort_by(|a, b| a.d.partial_cmp(&b.d).unwrap().then(a.j.cmp(&b.j)));
        v.truncate(k);
        v
    }

    /// Whether troop `i` has `j` in sight and has had time to react to it:
    /// only then can it aim at it.
    fn noticed(&self, i: usize, j: usize) -> bool {
        if !self.has_senses(i) || self.units[j].team == self.units[i].team {
            return true;
        }
        let n = self.units.len();
        if self.mem.len() != n * n {
            return self.sees(i, j);
        }
        let m = self.mem[i * n + j];
        m.seen != NEVER && m.seen + 1 >= self.tick && self.tick >= m.ready
    }

    /// Like `nearest`, but only what troop `i` has noticed.
    fn nearest_noticed(&self, i: usize, pred: impl Fn(&Unit) -> bool) -> Option<(usize, f32)> {
        let me = &self.units[i];
        let mut best: Option<(usize, f32)> = None;
        for (j, u) in self.units.iter().enumerate() {
            if j == i || !u.alive || !pred(u) || (u.cloak > 0 && u.team != me.team) || !self.noticed(i, j) {
                continue;
            }
            let d = me.dist(u) - if u.kind.is_building() { u.radius } else { 0.0 };
            if best.map_or(true, |(_, bd)| d < bd) {
                best = Some((j, d));
            }
        }
        best
    }

    /// The nearest fight troop `i` can hear (not one it is standing in):
    /// where, and how far.
    fn heard(&self, i: usize) -> Option<((f32, f32), f32)> {
        let me = &self.units[i];
        let mut best: Option<((f32, f32), f32)> = None;
        for &(x, y) in &self.noise {
            let (dx, dy) = (x - me.x, y - me.y);
            let d = (dx * dx + dy * dy).sqrt();
            if d > TOUCH && d <= HEAR_RANGE && best.map_or(true, |(_, bd)| d < bd) {
                best = Some(((dx, dy), d));
            }
        }
        best
    }

    /// Turn unit `i` towards (dx, dy), at most one tick's worth. Returns
    /// whether it was already facing that way squarely enough to strike.
    fn turn_toward(&mut self, i: usize, dx: f32, dy: f32) -> bool {
        let d = (dx * dx + dy * dy).sqrt();
        if d < 1e-4 {
            return true;
        }
        let (tx, ty) = (dx / d, dy / d);
        let (fx, fy) = self.units[i].facing;
        let dot = fx * tx + fy * ty;
        self.units[i].facing = if dot >= TURN_COS {
            (tx, ty)
        } else {
            let s = if fx * ty - fy * tx >= 0.0 { TURN_SIN } else { -TURN_SIN };
            let (nx, ny) = (fx * TURN_COS - fy * s, fx * s + fy * TURN_COS);
            let l = (nx * nx + ny * ny).sqrt();
            (nx / l, ny / l)
        };
        dot >= FACE_COS || !self.has_senses(i)
    }

    /// Turn to face `j`; true if facing it well enough to strike.
    fn face(&mut self, i: usize, j: usize) -> bool {
        let (dx, dy) = (self.units[j].x - self.units[i].x, self.units[j].y - self.units[i].y);
        self.turn_toward(i, dx, dy)
    }

    /// A fixed dice roll in [0, 1) for this tick and these two units.
    fn roll(&self, i: usize, j: usize) -> f32 {
        let mut h = self.tick.wrapping_mul(0x9E37_79B1) ^ (i as u32).wrapping_mul(0x85EB_CA77) ^ (j as u32).wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        (h >> 8) as f32 / 16_777_216.0
    }

    /// Calls only make sense with a trained teammate in earshot, so troops
    /// on their own never see them as an option.
    fn anyone_to_call(&self, i: usize) -> bool {
        if !self.has_senses(i) {
            return false;
        }
        let me = &self.units[i];
        (0..self.units.len()).any(|k| {
            k != i && self.units[k].alive && self.units[k].team == me.team && self.has_senses(k) && me.dist(&self.units[k]) <= CALL_RANGE
        })
    }

    /// The freshest call of each kind troop `i` has heard and had time to
    /// take in: where it points, and how fresh it is.
    fn heard_calls(&self, i: usize) -> [Option<(f32, f32, f32)>; N_CALLS] {
        let me = &self.units[i];
        let mut out = [None; N_CALLS];
        let mut best = [0u32; N_CALLS];
        for c in &self.calls {
            let age = self.tick.saturating_sub(c.tick);
            let (dx, dy) = (c.cx - me.x, c.cy - me.y);
            if c.team != me.team || c.from == i || age < REACT || age >= CALL_LIFE || dx * dx + dy * dy > CALL_RANGE * CALL_RANGE {
                continue;
            }
            let k = c.kind as usize;
            if out[k].is_none() || c.tick > best[k] {
                best[k] = c.tick;
                out[k] = Some((c.x, c.y, 1.0 - age as f32 / CALL_LIFE as f32));
            }
        }
        out
    }

    /// The fixed-size view a troop's policy sees.
    pub fn observe(&self, i: usize) -> [f32; OBS_DIM] {
        let mut o = [0.0f32; OBS_DIM];
        let me = &self.units[i];
        let team = me.team;
        let mask = self.mask(i);
        let rel = |u: &Unit| ((u.x - me.x) / OBS_SCALE, (u.y - me.y) / OBS_SCALE);
        let mut k = 0;
        let mut put = |o: &mut [f32; OBS_DIM], v: f32| {
            o[k] = v.clamp(-1.0, 1.0);
            k += 1;
        };
        // Self (8)
        put(&mut o, me.x / ARENA * 2.0 - 1.0);
        put(&mut o, me.y / ARENA * 2.0 - 1.0);
        put(&mut o, me.hp / me.max_hp);
        put(
            &mut o,
            if me.cd_max > 0 {
                me.cd as f32 / me.cd_max as f32
            } else {
                0.0
            },
        );
        put(&mut o, self.tick as f32 / self.max_ticks as f32);
        put(&mut o, mask[ACT_HIT_UNIT as usize] as u8 as f32);
        put(&mut o, mask[ACT_HIT_BUILDING as usize] as u8 as f32);
        put(&mut o, mask[ACT_HEAL as usize] as u8 as f32);
        // Two nearest buildings (5 each)
        // Attackers see enemy buildings; defenders see the buildings they protect.
        // Trained troops see these as they last saw them (sim version 6).
        let bs = self.known_k(i, 2, |u| {
            u.kind.is_building() && ((team == Team::Attack) != (u.team == team))
        });
        for s in 0..2 {
            if let Some(b) = bs.get(s) {
                put(&mut o, (b.x - me.x) / OBS_SCALE);
                put(&mut o, (b.y - me.y) / OBS_SCALE);
                put(&mut o, b.hp);
                put(&mut o, (self.units[b.j].kind == Kind::Tower) as u8 as f32);
                put(&mut o, 1.0);
            } else {
                for _ in 0..5 {
                    put(&mut o, 0.0);
                }
            }
        }
        // Three nearest enemy units (5 each)
        let es = self.known_k(i, 3, |u| u.team != team && !u.kind.is_building());
        for s in 0..3 {
            if let Some(e) = es.get(s) {
                let u = &self.units[e.j];
                put(&mut o, (e.x - me.x) / OBS_SCALE);
                put(&mut o, (e.y - me.y) / OBS_SCALE);
                put(&mut o, e.hp);
                put(
                    &mut o,
                    if u.cd_max > 0 {
                        u.cd as f32 / u.cd_max as f32
                    } else {
                        0.0
                    },
                );
                put(&mut o, 1.0);
            } else {
                for _ in 0..5 {
                    put(&mut o, 0.0);
                }
            }
        }
        // Two nearest allies (4 each)
        let al = self.nearest_k(i, 2, |u| u.team == team && !u.kind.is_building());
        for s in 0..2 {
            if let Some(&(j, _)) = al.get(s) {
                let u = &self.units[j];
                let (rx, ry) = rel(u);
                put(&mut o, rx);
                put(&mut o, ry);
                put(&mut o, u.hp / u.max_hp);
                put(&mut o, 1.0);
            } else {
                for _ in 0..4 {
                    put(&mut o, 0.0);
                }
            }
        }
        // Most injured ally within 10 (4)
        let mut hurt: Option<(usize, f32)> = None;
        for (j, u) in self.units.iter().enumerate() {
            if j != i && u.alive && u.team == team && !u.kind.is_building() && me.dist(u) <= 10.0 {
                let f = u.hp / u.max_hp;
                if f < 1.0 && hurt.map_or(true, |(_, bf)| f < bf) {
                    hurt = Some((j, f));
                }
            }
        }
        if let Some((j, f)) = hurt {
            let (rx, ry) = rel(&self.units[j]);
            put(&mut o, rx);
            put(&mut o, ry);
            put(&mut o, f);
            put(&mut o, 1.0);
        } else {
            for _ in 0..4 {
                put(&mut o, 0.0);
            }
        }
        // Base summary (2)
        let (mut b_alive, mut b_all, mut g_alive, mut g_all) = (0, 0, 0, 0);
        for u in &self.units {
            if u.kind.is_building() {
                if (team == Team::Attack) != (u.team == team) {
                    b_all += 1;
                    b_alive += u.alive as u32;
                }
            } else if u.team != team {
                g_all += 1;
                g_alive += u.alive as u32;
            }
        }
        put(
            &mut o,
            if b_all > 0 {
                b_alive as f32 / b_all as f32
            } else {
                0.0
            },
        );
        put(
            &mut o,
            if g_all > 0 {
                g_alive as f32 / g_all as f32
            } else {
                0.0
            },
        );
        // Everything below came with dodge, block and weapon skills, appended
        // so brains grown from older saves still read the first part the same.
        // Own moves (6)
        let frac = |v: u32, max: u32| if max > 0 { v as f32 / max as f32 } else { 0.0 };
        put(&mut o, mask[ACT_DODGE as usize] as u8 as f32);
        put(&mut o, mask[ACT_BLOCK as usize] as u8 as f32);
        put(&mut o, mask[ACT_SKILL as usize] as u8 as f32);
        put(&mut o, (me.block_ticks > 0) as u8 as f32);
        put(&mut o, frac(me.skill_cd, me.skill_cd_max));
        put(&mut o, frac(me.move_cd[Move::Dodge as usize], DODGE_CD));
        // Crowd: enemy units close enough for a spin or slam (1)
        put(&mut o, self.enemies_near(team, me.x, me.y, 2.2, false).len() as f32 / 4.0);
        // The three nearest enemies again: stunned, guard up (2 each)
        for s in 0..3 {
            if let Some(e) = es.get(s).filter(|e| e.fresh >= 1.0) {
                let u = &self.units[e.j];
                put(&mut o, (u.stun > 0) as u8 as f32);
                put(&mut o, (u.block_ticks > 0) as u8 as f32);
            } else {
                put(&mut o, 0.0);
                put(&mut o, 0.0);
            }
        }
        // Weapon (7)
        for w in Weapon::ALL {
            put(&mut o, (me.weapon == Some(w)) as u8 as f32);
        }
        // Class moves ready (5), cloaked, warded (sim version 3)
        for a in [ACT_CHARGE, ACT_CLOAK, ACT_BLINK, ACT_SNARE, ACT_WARD] {
            put(&mut o, mask[a as usize] as u8 as f32);
        }
        put(&mut o, (me.cloak > 0) as u8 as f32);
        put(&mut o, (me.ward > 0) as u8 as f32);
        // The two nearest allies again, for teamwork (sim version 4): their
        // role, whether something is attacking them, and whether they are
        // fighting or healing right now (7 each).
        for s in 0..2 {
            if let Some(&(j, _)) = al.get(s) {
                let u = &self.units[j];
                let role = u.role();
                for r in Role::ALL {
                    put(&mut o, (role == r) as u8 as f32);
                }
                put(&mut o, self.threat(j).is_some() as u8 as f32);
                let a = u.last_action;
                put(&mut o, matches!(a, ACT_HIT_UNIT | ACT_HIT_BUILDING | ACT_SKILL | ACT_CHARGE) as u8 as f32);
                put(&mut o, (a == ACT_HEAL || a == ACT_HEAL_2 || a == ACT_WARD) as u8 as f32);
            } else {
                for _ in 0..7 {
                    put(&mut o, 0.0);
                }
            }
        }
        // Whom this troop can heal right now, and how far its talents reach
        // (sim version 5).
        put(&mut o, mask[ACT_HEAL_2 as usize] as u8 as f32);
        put(&mut o, me.talents as f32 / crate::classes::TALENT_MAX as f32);
        // Senses (sim version 6): which way it looks, how fresh what it
        // knows of the buildings and enemies above is, and the nearest fight
        // it can hear.
        put(&mut o, me.facing.0);
        put(&mut o, me.facing.1);
        for s in 0..2 {
            put(&mut o, bs.get(s).map_or(0.0, |b| b.fresh));
        }
        for s in 0..3 {
            put(&mut o, es.get(s).map_or(0.0, |e| e.fresh));
        }
        if let Some(((dx, dy), d)) = self.heard(i) {
            put(&mut o, dx / HEAR_RANGE);
            put(&mut o, dy / HEAR_RANGE);
            put(&mut o, 1.0 - d / HEAR_RANGE);
        } else {
            for _ in 0..3 {
                put(&mut o, 0.0);
            }
        }
        // Calls from teammates (sim version 7): for focus, fall back and
        // help, where the freshest one points and how fresh it is.
        for c in self.heard_calls(i) {
            match c {
                Some((x, y, fresh)) => {
                    put(&mut o, (x - me.x) / CALL_RANGE);
                    put(&mut o, (y - me.y) / CALL_RANGE);
                    put(&mut o, fresh);
                }
                None => {
                    for _ in 0..3 {
                        put(&mut o, 0.0);
                    }
                }
            }
        }
        debug_assert_eq!(k, OBS_DIM);
        o
    }

    // ------------------------------------------------------------ stepping

    /// Hand-written fighter: kill the nearest guard, then the nearest building.
    pub fn scripted_action(&self, i: usize) -> u8 {
        let m = self.mask(i);
        let me = &self.units[i];
        let team = me.team;
        if let Some((j, _)) = self.nearest(i, |u| u.team != team && !u.kind.is_building()) {
            if m[ACT_HIT_UNIT as usize] {
                return ACT_HIT_UNIT;
            }
            if me.dist(&self.units[j]) <= me.range {
                return ACT_IDLE; // in range, waiting on cooldown
            }
            return move_toward(me, &self.units[j]);
        }
        if let Some((j, d)) = self.nearest(i, |u| u.team != team && u.kind.is_building()) {
            if m[ACT_HIT_BUILDING as usize] {
                return ACT_HIT_BUILDING;
            }
            if d <= me.range {
                return ACT_IDLE;
            }
            return move_toward(me, &self.units[j]);
        }
        ACT_IDLE
    }

    /// Advance one tick. `actions[i]` is used for units with `Control::External`.
    pub fn step(&mut self, actions: &[u8]) {
        if self.mem.len() != self.units.len() * self.units.len() {
            self.update_senses();
        }
        self.tick += 1;
        self.events.clear();
        let tick = self.tick;
        self.calls.retain(|c| tick - c.tick < CALL_LIFE);
        let n = self.units.len();

        // Attacking troops, in index order.
        for i in 0..n {
            let u = &self.units[i];
            if !u.alive || u.kind != Kind::Troop {
                continue;
            }
            let a = match u.control {
                Control::External => actions.get(i).copied().unwrap_or(ACT_IDLE),
                Control::Scripted => self.scripted_action(i),
                Control::Defender => ACT_IDLE,
            };
            self.troop_act(i, a);
        }

        self.spring_traps();

        // Guards: chase troops that come close, otherwise walk home.
        for i in 0..n {
            if !self.units[i].alive || self.units[i].kind != Kind::Guard {
                continue;
            }
            self.guard_act(i);
        }

        // Towers shoot the nearest troop in range.
        for i in 0..n {
            if !self.units[i].alive || self.units[i].kind != Kind::Tower {
                continue;
            }
            if self.units[i].cd > 0 {
                self.units[i].cd -= 1;
            }
            if self.units[i].cd > 0 {
                continue;
            }
            let (team, range) = (self.units[i].team, self.units[i].range);
            if let Some((j, d)) = self.nearest(i, |v| v.team != team && v.kind == Kind::Troop) {
                if d <= range {
                    self.units[i].cd = self.units[i].cd_max;
                    let (from, to) = (self.pos(i), self.pos(j));
                    self.events.push(Event {
                        kind: EventKind::TowerShot,
                        from,
                        to,
                    });
                    let dmg = self.units[i].dmg;
                    self.damage(i, j, dmg);
                }
            }
        }

        // What could be heard this tick, then what everyone saw.
        self.noise.clear();
        for e in &self.events {
            if !matches!(e.kind, EventKind::Heal | EventKind::Cloak | EventKind::Ward | EventKind::TrapSet) {
                self.noise.push(e.to);
            }
        }
        self.update_senses();
    }

    fn pos(&self, i: usize) -> (f32, f32) {
        (self.units[i].x, self.units[i].y)
    }

    fn damage(&mut self, from: usize, to: usize, amount: f32) {
        if self.units[to].invuln > 0 {
            self.units[to].stats.evaded += 1;
            return;
        }
        let mut amount = amount;
        if self.units[to].ward > 0 {
            amount *= 1.0 - self.units[to].ward_power;
        }
        if self.units[to].block_ticks > 0 {
            let kept = amount * self.units[to].block_taken;
            self.units[to].stats.blocked += amount - kept;
            amount = kept;
            let p = self.pos(to);
            self.events.push(Event { kind: EventKind::Blocked, from: self.pos(from), to: p });
        }
        let hit = amount.min(self.units[to].hp);
        self.units[to].hp -= hit;
        let to_building = self.units[to].kind.is_building();
        let s = &mut self.units[from].stats;
        if to_building {
            s.building_dmg += hit;
        } else {
            s.unit_dmg += hit;
        }
        if self.units[to].hp <= 0.0 {
            self.units[to].hp = 0.0;
            self.units[to].alive = false;
            self.units[from].stats.kills += 1;
            let p = self.pos(to);
            self.events.push(Event {
                kind: EventKind::Death,
                from: p,
                to: p,
            });
        }
    }

    fn troop_act(&mut self, i: usize, a: u8) {
        {
            let u = &mut self.units[i];
            for t in [&mut u.cd, &mut u.skill_cd, &mut u.block_ticks, &mut u.invuln, &mut u.cloak, &mut u.ward] {
                *t = t.saturating_sub(1);
            }
            for t in u.move_cd.iter_mut() {
                *t = t.saturating_sub(1);
            }
            if u.stun > 0 {
                u.stun -= 1;
                u.last_action = ACT_IDLE;
                return;
            }
        }
        let (mut a, mut m) = (a, self.mask(i));
        if (ACT_CALL_FOCUS..=ACT_CALL_HELP).contains(&a) && m[a as usize] {
            // A shout doesn't cost the turn: the troop keeps doing what it
            // did last tick (never itself a call).
            self.call(i, a - ACT_CALL_FOCUS);
            a = self.units[i].last_action;
            m = self.mask(i);
        }
        self.units[i].last_action = a;
        match a {
            1..=8 => {
                let (dx, dy) = DIRS[(a - 1) as usize];
                let u = &self.units[i];
                let sp = if u.block_ticks > 0 { u.speed * 0.5 } else { u.speed };
                self.move_unit(i, dx * sp, dy * sp);
                self.turn_toward(i, dx, dy);
            }
            ACT_DODGE if m[ACT_DODGE as usize] => {
                let j = self.threat(i).unwrap();
                let (from, (tx, ty)) = (self.pos(i), self.pos(j));
                let (dx, dy) = (from.0 - tx, from.1 - ty);
                let d = (dx * dx + dy * dy).sqrt();
                let (ux, uy) = if d > 1e-4 { (dx / d, dy / d) } else { (-1.0, 0.0) };
                self.move_unit(i, ux * DODGE_DIST, uy * DODGE_DIST);
                let u = &mut self.units[i];
                u.invuln = DODGE_INVULN;
                u.move_cd[Move::Dodge as usize] = move_cd(DODGE_CD, u.moves[Move::Dodge as usize]);
                u.stats.dodges += 1;
                let to = self.pos(i);
                self.events.push(Event { kind: EventKind::Dodge, from, to });
            }
            ACT_BLOCK if m[ACT_BLOCK as usize] => {
                let u = &mut self.units[i];
                u.block_ticks = BLOCK_TICKS;
                u.move_cd[Move::Block as usize] = move_cd(BLOCK_CD, u.moves[Move::Block as usize]);
            }
            ACT_CHARGE if m[ACT_CHARGE as usize] => self.charge(i),
            ACT_CLOAK if m[ACT_CLOAK as usize] => {
                let u = &mut self.units[i];
                let p = u.moves[Move::Cloak as usize];
                u.cloak = (CLOAK_TICKS as f32 * p).round() as u32;
                u.move_cd[Move::Cloak as usize] = move_cd(CLOAK_CD, p);
                let at = self.pos(i);
                self.events.push(Event { kind: EventKind::Cloak, from: at, to: at });
            }
            ACT_BLINK if m[ACT_BLINK as usize] => {
                let j = self.threat(i).unwrap();
                let p = self.units[i].moves[Move::Blink as usize];
                let (from, (tx, ty)) = (self.pos(i), self.pos(j));
                let (dx, dy) = (from.0 - tx, from.1 - ty);
                let d = (dx * dx + dy * dy).sqrt();
                let (ux, uy) = if d > 1e-4 { (dx / d, dy / d) } else { (-1.0, 0.0) };
                let dist = 3.0 + 1.5 * p;
                self.move_unit(i, ux * dist, uy * dist);
                let u = &mut self.units[i];
                u.invuln = BLINK_INVULN;
                u.move_cd[Move::Blink as usize] = move_cd(BLINK_CD, p);
                u.stats.dodges += 1;
                let to = self.pos(i);
                self.events.push(Event { kind: EventKind::Blink, from, to });
            }
            ACT_SNARE if m[ACT_SNARE as usize] => {
                let (team, (x, y)) = (self.units[i].team, self.pos(i));
                let p = self.units[i].moves[Move::Snare as usize];
                self.traps.push(Trap { x, y, team, owner: i, power: p, ttl: SNARE_LIFE });
                self.units[i].move_cd[Move::Snare as usize] = move_cd(SNARE_CD, p);
                self.events.push(Event { kind: EventKind::TrapSet, from: (x, y), to: (x, y) });
            }
            ACT_WARD if m[ACT_WARD as usize] => {
                let (team, p) = (self.units[i].team, self.units[i].moves[Move::Ward as usize]);
                // Shields the allies most under attack first, then the nearest.
                let near: Vec<usize> = (0..self.units.len())
                    .filter(|&k| {
                        let u = &self.units[k];
                        u.alive && u.team == team && u.kind == Kind::Troop && self.units[i].dist(u) <= WARD_RADIUS
                    })
                    .collect();
                let me = &self.units[i];
                for k in self.reach(i, near, |k| me.dist(&self.units[k]) - if self.threat(k).is_some() { 100.0 } else { 0.0 }) {
                    self.units[k].ward = WARD_TICKS;
                    self.units[k].ward_power = 0.5 * p;
                }
                self.units[i].move_cd[Move::Ward as usize] = move_cd(WARD_CD, p);
                let at = self.pos(i);
                self.events.push(Event { kind: EventKind::Ward, from: at, to: at });
            }
            ACT_SKILL if m[ACT_SKILL as usize] => self.use_skill(i),
            // A troop has to face what it hits: turned away, this tick only turns it.
            ACT_HIT_UNIT if m[ACT_HIT_UNIT as usize] => {
                let j = self.unit_target(i).unwrap();
                if self.face(i, j) {
                    self.strike(i, j);
                }
            }
            ACT_HIT_BUILDING if m[ACT_HIT_BUILDING as usize] => {
                let j = self.building_target(i).unwrap();
                if self.face(i, j) {
                    self.strike(i, j);
                }
            }
            // Standing still, a troop looks towards the noise of a fight.
            ACT_IDLE => {
                if let Some(((dx, dy), _)) = self.heard(i) {
                    self.turn_toward(i, dx, dy);
                }
            }
            ACT_HEAL | ACT_HEAL_2 if m[a as usize] => {
                let j = self.heal_target(i, (a == ACT_HEAL_2) as usize).unwrap();
                let amount = self.units[i]
                    .heal
                    .min(self.units[j].max_hp - self.units[j].hp);
                self.units[j].hp += amount;
                self.units[i].stats.heal += amount;
                self.units[i].cd = self.units[i].cd_max;
                let (from, to) = (self.pos(i), self.pos(j));
                self.events.push(Event {
                    kind: EventKind::Heal,
                    from,
                    to,
                });
            }
            _ => {}
        }
    }

    /// Shout to the team. Calling gives away a cloaked troop.
    fn call(&mut self, i: usize, kind: u8) {
        let (team, cx, cy) = (self.units[i].team, self.units[i].x, self.units[i].y);
        let (x, y) = if kind == 0 {
            self.nearest_noticed(i, |u| u.team != team).map_or((cx, cy), |(j, _)| self.pos(j))
        } else {
            (cx, cy)
        };
        self.calls.push(Call { team, from: i, kind, cx, cy, x, y, tick: self.tick });
        self.units[i].cloak = 0;
        self.units[i].stats.calls += 1;
        self.events.push(Event { kind: EventKind::Call(kind), from: (cx, cy), to: (x, y) });
    }

    fn strike(&mut self, i: usize, j: usize) {
        self.units[i].cd = self.units[i].cd_max;
        self.units[i].cloak = 0;
        let ranged = self.units[i].range > 2.5;
        let kind = if ranged { EventKind::Arrow } else { EventKind::Melee };
        let (from, to) = (self.pos(i), self.pos(j));
        // Trained troops are not perfect shots: far and moving targets get
        // missed more often. Buildings are hard to miss.
        if ranged && self.has_senses(i) {
            let (me, t) = (&self.units[i], &self.units[j]);
            let p = if t.kind.is_building() {
                0.02
            } else {
                let r = (me.dist(t) / me.range).min(1.0);
                0.04 + 0.2 * r * r + if matches!(t.last_action, 1..=8) { 0.12 } else { 0.0 }
            };
            let roll = self.roll(i, j);
            if roll < p {
                let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                let d = (dx * dx + dy * dy).sqrt().max(1e-4);
                let side = if roll < p * 0.5 { 1.0 } else { -1.0 };
                let wide = (to.0 - dy / d * side * 1.2 + dx / d * 0.8, to.1 + dx / d * side * 1.2 + dy / d * 0.8);
                self.events.push(Event { kind, from, to: wide });
                self.units[i].stats.missed += 1;
                return;
            }
        }
        self.events.push(Event { kind, from, to });
        let dmg = self.units[i].dmg;
        self.damage(i, j, dmg);
        let splash = self.units[i].splash;
        if splash > 0.0 {
            let team = self.units[i].team;
            let near: Vec<usize> = (0..self.units.len())
                .filter(|&k| {
                    let u = &self.units[k];
                    k != j && u.alive && u.team != team && u.dist(&self.units[j]) <= splash
                })
                .collect();
            let t = &self.units[j];
            for k in self.reach(i, near, |k| t.dist(&self.units[k])) {
                self.damage(i, k, dmg * 0.5);
            }
            self.events.push(Event { kind: EventKind::Blast, from: to, to });
        }
    }

    fn charge(&mut self, i: usize) {
        let j = self.charge_target(i).unwrap();
        let p = self.units[i].moves[Move::Charge as usize];
        let from = self.pos(i);
        let (tx, ty) = self.pos(j);
        let (dx, dy) = (tx - from.0, ty - from.1);
        let d = (dx * dx + dy * dy).sqrt();
        let step = (d - 0.8).min(CHARGE_DIST).max(0.0);
        self.move_unit(i, dx / d * step, dy / d * step);
        self.units[i].facing = (dx / d, dy / d);
        self.units[i].move_cd[Move::Charge as usize] = move_cd(CHARGE_CD, p);
        self.units[i].cloak = 0;
        let to = self.pos(i);
        self.events.push(Event { kind: EventKind::Charge, from, to });
        if self.units[i].dist(&self.units[j]) <= self.units[i].range + 0.5 {
            let dmg = self.units[i].dmg * 1.2 * p;
            self.damage(i, j, dmg);
            self.stun(j, (10.0 * p).round() as u32);
        }
    }

    /// Traps go off under the first enemy unit standing on them.
    fn spring_traps(&mut self) {
        let mut k = 0;
        while k < self.traps.len() {
            let t = self.traps[k];
            let hit = (0..self.units.len()).find(|&j| {
                let u = &self.units[j];
                u.alive && u.team != t.team && !u.kind.is_building() && {
                    let (dx, dy) = (u.x - t.x, u.y - t.y);
                    (dx * dx + dy * dy).sqrt() <= SNARE_RADIUS
                }
            });
            if let Some(j) = hit {
                self.events.push(Event { kind: EventKind::TrapHit, from: (t.x, t.y), to: (t.x, t.y) });
                self.damage(t.owner, j, 6.0 * t.power);
                self.stun(j, (15.0 * t.power).round() as u32);
                self.traps.remove(k);
                continue;
            }
            self.traps[k].ttl -= 1;
            if self.traps[k].ttl == 0 {
                self.traps.remove(k);
            } else {
                k += 1;
            }
        }
    }

    fn stun(&mut self, j: usize, ticks: u32) {
        let u = &mut self.units[j];
        if !u.alive || u.kind.is_building() || u.invuln > 0 {
            return;
        }
        u.stun = u.stun.max(ticks);
        u.block_ticks = 0;
        let p = self.pos(j);
        self.events.push(Event { kind: EventKind::Stun, from: p, to: p });
    }

    fn use_skill(&mut self, i: usize) {
        let Some(skill) = self.units[i].skill else { return };
        let (team, power) = (self.units[i].team, self.units[i].skill_power);
        let base = self.units[i].dmg * power;
        let me = self.pos(i);
        self.units[i].skill_cd = self.units[i].skill_cd_max;
        self.units[i].stats.skills += 1;
        self.units[i].cloak = 0;
        let mut to = me;
        match skill {
            Skill::ShieldBash => {
                let j = self.unit_target(i).unwrap();
                to = self.pos(j);
                self.damage(i, j, base * 1.5);
                self.stun(j, 20);
            }
            Skill::Whirlwind => {
                let near = self.enemies_near(team, me.0, me.1, 2.2, true);
                let u = &self.units[i];
                for k in self.reach(i, near, |k| u.dist(&self.units[k])) {
                    self.damage(i, k, base * 1.3);
                }
            }
            Skill::GroundSlam => {
                let near = self.enemies_near(team, me.0, me.1, 2.0, true);
                let u = &self.units[i];
                for k in self.reach(i, near, |k| u.dist(&self.units[k])) {
                    self.damage(i, k, base);
                    self.stun(k, 15);
                }
            }
            Skill::Shadowstep => {
                let j = self.shadowstep_target(i).unwrap();
                let (tx, ty) = self.pos(j);
                let (dx, dy) = (tx - me.0, ty - me.1);
                let d = (dx * dx + dy * dy).sqrt();
                if d > 0.8 {
                    let k = (d - 0.8) / d;
                    self.move_unit(i, dx * k, dy * k);
                }
                to = self.pos(i);
                self.damage(i, j, base * 3.0);
            }
            Skill::Volley => {
                let range = self.units[i].range;
                let reach = (self.units[i].talents as usize).min(3);
                let targets = self.nearest_k(i, reach, |u| u.team != team && !u.kind.is_building());
                for (j, d) in targets {
                    if d <= range {
                        let p = self.pos(j);
                        self.events.push(Event { kind: EventKind::Arrow, from: me, to: p });
                        self.damage(i, j, base);
                    }
                }
            }
            Skill::Fireball => {
                let j = self.unit_target(i).or_else(|| self.building_target(i)).unwrap();
                to = self.pos(j);
                self.damage(i, j, base * 2.0);
                let near: Vec<usize> = self.enemies_near(team, to.0, to.1, 2.5, true).into_iter().filter(|&k| k != j).collect();
                let t = &self.units[j];
                for k in self.reach(i, near, |k| t.dist(&self.units[k])) {
                    self.damage(i, k, base);
                }
            }
            Skill::HealingNova => {
                let amount = (15.0 + self.units[i].heal) * power;
                // Heals the most hurt allies first.
                let near: Vec<usize> = (0..self.units.len())
                    .filter(|&k| {
                        let u = &self.units[k];
                        u.alive && u.team == team && u.kind == Kind::Troop && self.units[i].dist(u) <= 3.5 && u.hp < u.max_hp
                    })
                    .collect();
                for k in self.reach(i, near, |k| self.units[k].hp / self.units[k].max_hp) {
                    let u = &self.units[k];
                    let h = amount.min(u.max_hp - u.hp);
                    self.units[k].hp += h;
                    self.units[i].stats.heal += h;
                }
            }
        }
        self.events.push(Event { kind: EventKind::Skill(skill), from: me, to });
    }

    fn move_unit(&mut self, i: usize, dx: f32, dy: f32) {
        let r = self.units[i].radius;
        let mut x = (self.units[i].x + dx).clamp(0.0, ARENA);
        let mut y = (self.units[i].y + dy).clamp(0.0, ARENA);
        // Buildings are solid: push out to their edge.
        for b in &self.units {
            if !b.alive || !b.kind.is_building() {
                continue;
            }
            let (ox, oy) = (x - b.x, y - b.y);
            let d = (ox * ox + oy * oy).sqrt();
            let min = b.radius + r;
            if d < min {
                if d > 1e-4 {
                    x = b.x + ox / d * min;
                    y = b.y + oy / d * min;
                } else {
                    x = b.x + min;
                }
            }
        }
        self.units[i].x = x;
        self.units[i].y = y;
    }

    fn guard_act(&mut self, i: usize) {
        if self.units[i].cd > 0 {
            self.units[i].cd -= 1;
        }
        if self.units[i].stun > 0 {
            self.units[i].stun -= 1;
            return;
        }
        let team = self.units[i].team;
        let target = self.nearest(i, |u| u.team != team && u.kind == Kind::Troop);
        let (gx, gy, (hx, hy)) = (self.units[i].x, self.units[i].y, self.units[i].home);
        let (range, cd, speed) = (self.units[i].range, self.units[i].cd, self.units[i].speed);
        let home = ((gx - hx) * (gx - hx) + (gy - hy) * (gy - hy)).sqrt();
        if let Some((j, d)) = target {
            if d <= range {
                if cd == 0 {
                    self.units[i].cd = self.units[i].cd_max;
                    let (from, to) = (self.pos(i), self.pos(j));
                    self.events.push(Event {
                        kind: EventKind::GuardHit,
                        from,
                        to,
                    });
                    let dmg = self.units[i].dmg;
                    self.damage(i, j, dmg);
                }
                return;
            }
            if d <= GUARD_AGGRO && home <= GUARD_LEASH {
                let (tx, ty) = self.pos(j);
                let (dx, dy) = ((tx - gx) / d, (ty - gy) / d);
                self.move_unit(i, dx * speed, dy * speed);
                return;
            }
        }
        if home > 0.1 {
            let step = speed.min(home);
            self.move_unit(i, (hx - gx) / home * step, (hy - gy) / home * step);
        }
    }

    pub fn troops_alive(&self) -> usize {
        self.units
            .iter()
            .filter(|u| u.alive && u.team == Team::Attack && u.kind == Kind::Troop)
            .count()
    }
}

fn move_toward(me: &Unit, t: &Unit) -> u8 {
    let (dx, dy) = (t.x - me.x, t.y - me.y);
    // Pick the direction with the largest dot product: no trig needed.
    let mut best = 0;
    let mut best_dot = f32::MIN;
    for (k, (ux, uy)) in DIRS.iter().enumerate() {
        let dot = dx * ux + dy * uy;
        if dot > best_dot {
            best_dot = dot;
            best = k;
        }
    }
    best as u8 + 1
}
