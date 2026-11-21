//! Training drills and the AI-built bases players attack.

use crate::classes::ClassSpec;
use crate::rng::Rng;
use crate::sim::{Control, Ground, Light, Patch, World, ARENA};
use serde::{Deserialize, Serialize};

pub const DRILL_TICKS: u32 = 500;
pub const BATTLE_TICKS: u32 = 900;
const C: f32 = ARENA / 2.0;
const SQUAD_TOWER_R: f32 = 3.5;
const SQUAD_GUARDS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drill {
    Solo,
    Squad,
}

impl Drill {
    pub fn pick(spec: &ClassSpec, rng: &mut Rng) -> Drill {
        let total = spec.drills.solo + spec.drills.squad;
        if rng.f32() * total < spec.drills.solo {
            Drill::Solo
        } else {
            Drill::Squad
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Drill::Solo => "solo raid",
            Drill::Squad => "squad raid",
        }
    }
}

fn around(rng: &mut Rng, x: f32, y: f32, r: f32) -> (f32, f32) {
    let (ux, uy) = rng.unit_vec();
    (
        (x + ux * r).clamp(0.5, ARENA - 0.5),
        (y + uy * r).clamp(0.5, ARENA - 0.5),
    )
}

/// Build a drill. The trained troop is always unit 0.
pub fn build_drill(spec: &ClassSpec, drill: Drill, rng: &mut Rng) -> World {
    let mut w = World::new(DRILL_TICKS);
    let tx = C + rng.range(-2.0, 2.0);
    let ty = C + rng.range(-2.0, 2.0);
    let dist = rng.range(7.5, 10.5);
    let (sx, sy) = around(rng, tx, ty, dist);
    w.add_troop(spec, sx, sy, Control::External, 0);
    match drill {
        Drill::Solo => {
            w.add_tower(tx, ty);
            if rng.chance(0.5) {
                let (cx, cy) = around(rng, tx, ty, 4.0);
                w.add_core(cx, cy);
            }
            for _ in 0..3 {
                let (gx, gy) = around(rng, tx, ty, 2.0);
                w.add_guard(gx, gy);
            }
        }
        Drill::Squad => {
            let fighter = crate::classes::BaseClass::Fighter.spec();
            for _ in 0..2 {
                let (fx, fy) = around(rng, sx, sy, 1.0);
                w.add_troop(&fighter, fx, fy, Control::Scripted, 0);
            }
            let (ux, uy) = rng.unit_vec();
            w.add_core(tx, ty);
            // Three towers 120 degrees apart, rotated by a random unit vector.
            const S: f32 = 0.866_025_4;
            for (c, s) in [(1.0f32, 0.0f32), (-0.5, S), (-0.5, -S)] {
                let (rx, ry) = (ux * c - uy * s, ux * s + uy * c);
                w.add_tower(
                    (tx + rx * SQUAD_TOWER_R).clamp(1.0, ARENA - 1.0),
                    (ty + ry * SQUAD_TOWER_R).clamp(1.0, ARENA - 1.0),
                );
            }
            for _ in 0..SQUAD_GUARDS {
                let (gx, gy) = around(rng, tx, ty, 2.5);
                w.add_guard(gx, gy);
            }
        }
    }
    lay_battlefield(&mut w, rng, None);
    w
}

/// A preset enemy base. Layouts are fixed so every player faces the same one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaseLayout {
    pub id: usize,
    pub name: &'static str,
    pub blurb: &'static str,
    pub towers: &'static [(f32, f32)],
    pub guards: &'static [(f32, f32)],
    pub reward_per_star: u32,
    /// Defender HP and damage multiplier. Above 1.0 the base is tougher than
    /// anything in the training drills.
    pub strength: f32,
}

pub const BASES: [BaseLayout; 4] = [
    BaseLayout {
        id: 0,
        name: "Outpost",
        blurb: "One tower and its guards. A trained fighter can take it alone.",
        towers: &[(3.0, 0.0)],
        guards: &[(1.0, 2.0), (1.0, -2.0), (5.0, 1.0)],
        reward_per_star: 40,
        strength: 1.0,
    },
    BaseLayout {
        id: 1,
        name: "Fort",
        blurb: "Two towers cover each other. Bring a squad.",
        towers: &[(-4.0, 0.0), (4.0, 0.0)],
        guards: &[
            (0.0, 2.5),
            (0.0, -2.5),
            (-6.0, 1.5),
            (6.0, -1.5),
            (2.0, 3.0),
        ],
        reward_per_star: 80,
        strength: 1.0,
    },
    BaseLayout {
        id: 2,
        name: "Twin Gate",
        blurb: "Towers at 1.5x strength on both flanks and a guard wall. Timing matters.",
        towers: &[(-3.5, 3.5), (3.5, 3.5), (0.0, -4.0)],
        guards: &[
            (-2.0, 1.0),
            (2.0, 1.0),
            (0.0, 2.5),
            (-5.0, 5.5),
            (5.0, 5.5),
            (0.0, -6.0),
        ],
        reward_per_star: 120,
        strength: 1.5,
    },
    BaseLayout {
        id: 3,
        name: "Citadel",
        blurb: "Four towers at 2.5x strength, eight guards. Needs healers and a plan.",
        towers: &[(-4.0, 4.0), (4.0, 4.0), (-4.0, -4.0), (4.0, -4.0)],
        guards: &[
            (0.0, 2.5),
            (0.0, -2.5),
            (2.5, 0.0),
            (-2.5, 0.0),
            (-6.0, 6.0),
            (6.0, 6.0),
            (-6.0, -6.0),
            (6.0, -6.0),
        ],
        reward_per_star: 200,
        strength: 2.5,
    },
];

/// Troops must be placed at least this far from the base centre.
pub const DEPLOY_RADIUS: f32 = 11.0;

pub fn can_deploy(x: f32, y: f32) -> bool {
    let (dx, dy) = (x - C, y - C);
    (0.5..=ARENA - 0.5).contains(&x)
        && (0.5..=ARENA - 0.5).contains(&y)
        && (dx * dx + dy * dy).sqrt() >= DEPLOY_RADIUS
}

/// The defending side of a battle. Troops are added by the caller.
pub fn build_base(base: &BaseLayout) -> World {
    let mut w = World::new(BATTLE_TICKS);
    w.add_core(C, C);
    for &(x, y) in base.towers {
        w.add_tower(C + x, C + y);
    }
    for &(x, y) in base.guards {
        w.add_guard(C + x, C + y);
    }
    for u in &mut w.units {
        u.max_hp *= base.strength;
        u.hp = u.max_hp;
        u.dmg *= base.strength;
    }
    w.recount_buildings();
    // Every player raids the same base in the same light, on the same ground.
    let light = [Light::Day, Light::Dusk, Light::Fog, Light::Night][base.id % 4];
    lay_battlefield(&mut w, &mut Rng::new(0x6669_656c_64 + base.id as u64), Some(light));
    w
}

// ---------------------------------------------------------------- defence

pub const DEFENSE_SLOTS: usize = 3;

/// A defence drill: three trained defenders (units 0..3, all controlled by
/// the policy being trained) protect a core and towers from a raid party.
/// `raiders` is filled by the caller with the attackers' unit indices.
pub fn build_defense_drill(spec: &ClassSpec, rng: &mut Rng) -> (World, (f32, f32)) {
    let mut w = World::new(DRILL_TICKS);
    let cx = C + rng.range(-2.0, 2.0);
    let cy = C + rng.range(-2.0, 2.0);
    for k in 0..DEFENSE_SLOTS {
        let (x, y) = around(rng, cx, cy, 2.5);
        w.add_troop_on(crate::sim::Team::Defend, spec, x, y, Control::External, k);
    }
    w.add_core(cx, cy);
    // Ranged and healing defenders train behind a frontline of built-in guards
    // with Guard-class stats, so they learn their support role.
    if spec.range > 2.5 || spec.can_heal() {
        let g = crate::classes::BaseClass::Guard.spec();
        for _ in 0..2 {
            let (x, y) = around(rng, cx, cy, 3.0);
            let i = w.add_guard(x, y);
            let u = &mut w.units[i];
            u.max_hp = g.hp;
            u.hp = g.hp;
            u.dmg = g.dmg;
            u.cd_max = g.cooldown;
        }
    }
    let towers = if rng.chance(0.5) { 1 } else { 2 };
    let (ux, uy) = rng.unit_vec();
    for t in 0..towers {
        let s = if t == 0 { 1.0 } else { -1.0 };
        w.add_tower(cx + s * uy * 3.5, cy - s * ux * 3.5);
    }
    // Raiders come from one side.
    // Near and far raids, so defenders learn when to wait under tower cover.
    let dist = rng.range(9.0, 16.0);
    let from = (
        (cx + ux * dist).clamp(1.0, ARENA - 1.0),
        (cy + uy * dist).clamp(1.0, ARENA - 1.0),
    );
    lay_battlefield(&mut w, rng, None);
    (w, from)
}

/// Add a raid party of scripted fighters at `from`.
pub fn add_raiders(w: &mut World, n: usize, from: (f32, f32), strength: f32, rng: &mut Rng) {
    let fighter = crate::classes::BaseClass::Fighter.spec();
    for _ in 0..n {
        let (x, y) = around(rng, from.0, from.1, 1.2);
        let i = w.add_troop(&fighter, x, y, Control::Scripted, usize::MAX);
        let u = &mut w.units[i];
        u.max_hp *= strength;
        u.hp = u.max_hp;
        u.dmg *= strength;
    }
}

/// The player's own base: a Town Hall plus the defences they built around it.
pub const HOME_NAME: &str = "Home Keep";

/// Defenders and buildings must be placed within this radius of the centre.
pub const HOME_RADIUS: f32 = 7.0;
pub const MAX_TOWN_HALL: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildKind {
    /// Archer tower: long range, light shots.
    Tower,
    /// Cannon: short range, heavy shots.
    Cannon,
}

impl BuildKind {
    pub const ALL: [BuildKind; 2] = [BuildKind::Tower, BuildKind::Cannon];

    pub fn name(self) -> &'static str {
        match self {
            BuildKind::Tower => "Archer Tower",
            BuildKind::Cannon => "Cannon",
        }
    }

    pub fn cost(self) -> u32 {
        match self {
            BuildKind::Tower => 60,
            BuildKind::Cannon => 100,
        }
    }

    /// How many of this building a Town Hall level allows.
    pub fn max_at(self, town_hall: u32) -> usize {
        match self {
            BuildKind::Tower => 1 + town_hall as usize,
            BuildKind::Cannon => (town_hall / 2) as usize,
        }
    }
}

/// One placed building, as an offset from the base centre.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub kind: BuildKind,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeBase {
    pub town_hall: u32,
    pub buildings: Vec<Placed>,
}

impl Default for HomeBase {
    /// A new player's base: Town Hall 1 with two archer towers.
    fn default() -> Self {
        let t = |x, y| Placed {
            kind: BuildKind::Tower,
            x,
            y,
        };
        HomeBase {
            town_hall: 1,
            buildings: vec![t(-3.5, 0.0), t(3.5, 0.0)],
        }
    }
}

impl HomeBase {
    /// The reference base used by the CLI and balance checks: Town Hall 3,
    /// four towers in a diamond.
    pub fn reference() -> Self {
        let t = |x, y| Placed {
            kind: BuildKind::Tower,
            x,
            y,
        };
        HomeBase {
            town_hall: 3,
            buildings: vec![t(-3.5, 0.0), t(3.5, 0.0), t(0.0, -3.5), t(0.0, 3.5)],
        }
    }

    /// Every building this Town Hall allows, spread in a ring (towers outside,
    /// cannons inside). Used by the CLI to test bases.
    pub fn full(town_hall: u32) -> Self {
        let mut buildings = Vec::new();
        let ring = |n: usize, r: f32, kind| {
            (0..n).map(move |k| {
                let (mut x, mut y) = (r, 0.0f32);
                // Rotate by k/n of a turn without trig: repeated small rotations.
                for _ in 0..(k * 360 / n.max(1)) {
                    let (c, s) = (0.999_847_7f32, 0.017_452_4f32);
                    (x, y) = (x * c - y * s, x * s + y * c);
                }
                Placed { kind, x, y }
            })
        };
        buildings.extend(ring(
            BuildKind::Tower.max_at(town_hall),
            4.6,
            BuildKind::Tower,
        ));
        buildings.extend(ring(
            BuildKind::Cannon.max_at(town_hall),
            3.0,
            BuildKind::Cannon,
        ));
        HomeBase {
            town_hall,
            buildings,
        }
    }

    pub fn count(&self, kind: BuildKind) -> usize {
        self.buildings.iter().filter(|b| b.kind == kind).count()
    }

    pub fn can_add(&self, kind: BuildKind) -> bool {
        self.count(kind) < kind.max_at(self.town_hall)
    }

    /// A building fits at offset (x, y) if it is inside the base ring and
    /// clear of the Town Hall and other buildings.
    pub fn fits(&self, x: f32, y: f32) -> bool {
        let d = (x * x + y * y).sqrt();
        d + 1.0 <= HOME_RADIUS
            && d >= CORE_RADIUS_HOME + 1.2
            && self.buildings.iter().all(|b| {
                let (dx, dy) = (b.x - x, b.y - y);
                (dx * dx + dy * dy).sqrt() >= 2.2
            })
    }

    /// Index of the building under offset (x, y), if any.
    pub fn at(&self, x: f32, y: f32) -> Option<usize> {
        self.buildings.iter().position(|b| {
            let (dx, dy) = (b.x - x, b.y - y);
            (dx * dx + dy * dy).sqrt() <= 1.1
        })
    }
}

const CORE_RADIUS_HOME: f32 = 1.5;

/// Gold to raise the Town Hall from `level` to `level + 1`.
pub fn town_hall_cost(level: u32) -> u32 {
    200 * level * level
}

/// The highest troop level a Town Hall level allows.
pub fn max_troop_level(town_hall: u32) -> u32 {
    (town_hall + 1).min(crate::troop::MAX_LEVEL)
}

pub fn can_place_defender(x: f32, y: f32) -> bool {
    let (dx, dy) = (x - C, y - C);
    (dx * dx + dy * dy).sqrt() <= HOME_RADIUS
}

/// Build the world for a home base. Your own keep is sturdier than a drill
/// base, and a higher Town Hall is tougher.
pub fn build_home_base(base: &HomeBase) -> World {
    let mut w = World::new(BATTLE_TICKS);
    let th = w.add_core(C, C);
    let lvl = 1.0 + 0.25 * (base.town_hall.max(1) - 1) as f32;
    w.units[th].max_hp *= lvl;
    w.units[th].hp = w.units[th].max_hp;
    for b in &base.buildings {
        let i = w.add_tower(C + b.x, C + b.y);
        if b.kind == BuildKind::Cannon {
            let u = &mut w.units[i];
            u.range = 4.5;
            u.dmg *= 2.5;
            u.cd_max = 14;
            u.max_hp *= 1.3;
            u.hp = u.max_hp;
            u.style = 1;
        }
    }
    for u in &mut w.units {
        u.hp *= HOME_STRENGTH;
        u.max_hp *= HOME_STRENGTH;
        u.dmg *= HOME_STRENGTH;
    }
    w.recount_buildings();
    // The ground and light come from the base's own layout, so the player's
    // game and the server agree on them.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ base.town_hall as u64;
    for b in &base.buildings {
        for v in [b.x.to_bits() as u64, b.y.to_bits() as u64, b.kind as u64] {
            h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
        }
    }
    let mut rng = Rng::new(h);
    let r = rng.f32();
    let light = if r < 0.5 {
        Light::Day
    } else if r < 0.7 {
        Light::Dusk
    } else if r < 0.85 {
        Light::Night
    } else {
        Light::Fog
    };
    lay_battlefield(&mut w, &mut rng, Some(light));
    w
}

pub fn build_home() -> World {
    build_home_base(&HomeBase::reference())
}

pub const HOME_STRENGTH: f32 = 1.25;

/// Default defender spots: a line across the front of the base, facing the
/// raid entry. Used by auto-place and the CLI.
pub fn home_spots(n: usize) -> Vec<(f32, f32)> {
    let (ex, ey) = (RAID_ENTRY.0 - C, RAID_ENTRY.1 - C);
    let d = (ex * ex + ey * ey).sqrt();
    let (ux, uy) = (ex / d, ey / d);
    (0..n)
        .map(|k| {
            let off = (k as f32 - (n as f32 - 1.0) / 2.0) * 2.0;
            (C + ux * 3.5 - uy * off, C + uy * 3.5 + ux * off)
        })
        .collect()
}

/// Scripted raid waves that test a home base.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wave {
    pub name: &'static str,
    pub raiders: usize,
    pub strength: f32,
    pub reward: u32,
}

pub const WAVES: [Wave; 4] = [
    Wave {
        name: "Scouts",
        raiders: 3,
        strength: 1.0,
        reward: 40,
    },
    Wave {
        name: "Raiding party",
        raiders: 4,
        strength: 1.0,
        reward: 80,
    },
    Wave {
        name: "Warband",
        raiders: 5,
        strength: 1.0,
        reward: 140,
    },
    Wave {
        name: "Horde",
        raiders: 6,
        strength: 1.2,
        reward: 220,
    },
];

/// Where a wave or an invading army enters the home base: bottom-left edge.
pub const RAID_ENTRY: (f32, f32) = (4.0, 4.0);

// ------------------------------------------------------------- team drills

/// Fewest and most troops a team can train together.
pub const MIN_TEAM: usize = 2;
pub const MAX_TEAM: usize = 5;

/// A team raid: the team's troops are units 0..n, in order. A team smaller
/// than 3 is topped up with scripted fighters; a bigger one faces more guards.
/// Team drill hardness: 1 is the standard drill, 5 the hardest.
pub const MAX_HARDNESS: u8 = 5;

/// How much tougher the defence (or the raiders) are at a hardness: each
/// step adds 10% health and damage.
pub fn hardness_strength(hardness: u8) -> f32 {
    1.0 + 0.1 * (hardness.clamp(1, MAX_HARDNESS) - 1) as f32
}

/// A team attack drill. Harder drills add a guard per step and a fourth
/// tower from hardness 4, and make the defence tougher.
pub fn build_team_drill(team: &[ClassSpec], hardness: u8, rng: &mut Rng) -> World {
    let h = hardness.clamp(1, MAX_HARDNESS);
    let mut w = World::new(DRILL_TICKS);
    let tx = C + rng.range(-2.0, 2.0);
    let ty = C + rng.range(-2.0, 2.0);
    let dist = rng.range(7.5, 10.5);
    let (sx, sy) = around(rng, tx, ty, dist);
    for (k, spec) in team.iter().enumerate() {
        let (x, y) = if k == 0 { (sx, sy) } else { around(rng, sx, sy, 1.0) };
        w.add_troop(spec, x, y, Control::External, k);
    }
    let fighter = crate::classes::BaseClass::Fighter.spec();
    for _ in team.len()..3 {
        let (fx, fy) = around(rng, sx, sy, 1.0);
        w.add_troop(&fighter, fx, fy, Control::Scripted, 0);
    }
    let (ux, uy) = rng.unit_vec();
    w.add_core(tx, ty);
    const S: f32 = 0.866_025_4;
    let towers = 3 + (h >= 4) as usize;
    let spots: Vec<(f32, f32)> = if towers == 3 {
        vec![(1.0, 0.0), (-0.5, S), (-0.5, -S)]
    } else {
        (0..towers)
            .map(|k| {
                let a = k as f32 * std::f32::consts::TAU / towers as f32;
                (a.cos(), a.sin())
            })
            .collect()
    };
    let strength = hardness_strength(h);
    let first = w.units.len();
    for (c, s) in spots {
        let (rx, ry) = (ux * c - uy * s, ux * s + uy * c);
        w.add_tower(
            (tx + rx * SQUAD_TOWER_R).clamp(1.0, ARENA - 1.0),
            (ty + ry * SQUAD_TOWER_R).clamp(1.0, ARENA - 1.0),
        );
    }
    for _ in 0..SQUAD_GUARDS + 2 * team.len().saturating_sub(3) + (h - 1) as usize {
        let (gx, gy) = around(rng, tx, ty, 2.5);
        w.add_guard(gx, gy);
    }
    if h > 1 {
        for u in &mut w.units[first..] {
            u.max_hp *= strength;
            u.hp = u.max_hp;
            u.dmg *= strength;
        }
        w.recount_buildings();
    }
    lay_battlefield(&mut w, rng, None);
    w
}

/// A team defence: the team's defenders are units 0..n, in order, around a
/// core and towers. Built-in guards hold the front only when the team has no
/// melee troop of its own. Returns the world and where the raid comes from.
pub fn build_team_defense(team: &[ClassSpec], rng: &mut Rng) -> (World, (f32, f32)) {
    let mut w = World::new(DRILL_TICKS);
    let cx = C + rng.range(-2.0, 2.0);
    let cy = C + rng.range(-2.0, 2.0);
    for (k, spec) in team.iter().enumerate() {
        let (x, y) = around(rng, cx, cy, 2.5);
        w.add_troop_on(crate::sim::Team::Defend, spec, x, y, Control::External, k);
    }
    w.add_core(cx, cy);
    if team.iter().all(|s| s.range > 2.5 || s.can_heal()) {
        let g = crate::classes::BaseClass::Guard.spec();
        for _ in 0..2 {
            let (x, y) = around(rng, cx, cy, 3.0);
            let i = w.add_guard(x, y);
            let u = &mut w.units[i];
            u.max_hp = g.hp;
            u.hp = g.hp;
            u.dmg = g.dmg;
            u.cd_max = g.cooldown;
        }
    }
    let towers = if rng.chance(0.5) { 1 } else { 2 };
    let (ux, uy) = rng.unit_vec();
    for t in 0..towers {
        let s = if t == 0 { 1.0 } else { -1.0 };
        w.add_tower(cx + s * uy * 3.5, cy - s * ux * 3.5);
    }
    let dist = rng.range(9.0, 16.0);
    let from = (
        (cx + ux * dist).clamp(1.0, ARENA - 1.0),
        (cy + uy * dist).clamp(1.0, ARENA - 1.0),
    );
    lay_battlefield(&mut w, rng, None);
    (w, from)
}

/// Light and ground for a battle (sim version 10): two patches of mud and a
/// hill between the deploy ring and the base, clear of buildings. `light`
/// None picks one at random (training drills see every kind).
pub fn lay_battlefield(w: &mut World, rng: &mut Rng, light: Option<Light>) {
    w.light = light.unwrap_or_else(|| {
        let r = rng.f32();
        if r < 0.4 {
            Light::Day
        } else if r < 0.6 {
            Light::Dusk
        } else if r < 0.8 {
            Light::Night
        } else {
            Light::Fog
        }
    });
    w.terrain.clear();
    for ground in [Ground::Mud, Ground::Mud, Ground::Hill] {
        let r = if ground == Ground::Hill { 2.5 } else { 2.0 };
        for _ in 0..20 {
            let dist = rng.range(4.0, 10.0);
            let (x, y) = around(rng, C, C, dist);
            let clear = w.units.iter().all(|u| {
                let (dx, dy) = (u.x - x, u.y - y);
                !u.kind.is_building() || (dx * dx + dy * dy).sqrt() > r + u.radius + 0.5
            }) && w.terrain.iter().all(|p| {
                let (dx, dy) = (p.x - x, p.y - y);
                (dx * dx + dy * dy).sqrt() > r + p.r
            });
            if clear && (r..=ARENA - r).contains(&x) && (r..=ARENA - r).contains(&y) {
                w.terrain.push(Patch { ground, x, y, r });
                break;
            }
        }
    }
}
