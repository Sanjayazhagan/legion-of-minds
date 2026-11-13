//! "See what it's thinking": pick one of your troops in a battle and watch
//! what is going on in its head: what it sees, what it remembers, what it
//! hears, and what it is about to do (with how sure its brain is).
//!
//! This only reads the battle (see `godcore::mind`); it never changes it.

use crate::render::{call_color, call_words, View};
use crate::ui::*;
use godcore::mind::Thought;
use godcore::sim::{self, Kind, World, DIRS};
use macroquad::prelude::*;

const SEE: Color = Color::new(1.0, 0.92, 0.35, 1.0);
const SPOT: Color = Color::new(1.0, 0.6, 0.2, 1.0);
const MEMORY: Color = Color::new(0.7, 0.75, 0.95, 1.0);
const HEAR: Color = Color::new(0.55, 0.85, 1.0, 1.0);
const PLAN: Color = Color::new(0.98, 0.70, 0.25, 1.0);

/// Compass word for a move direction (north is up the map).
fn compass(a: u8) -> &'static str {
    [
        "east",
        "north-east",
        "north",
        "north-west",
        "west",
        "south-west",
        "south",
        "south-east",
    ][(a - 1) as usize]
}

/// Plain words for what action `a` does, naming whom it is aimed at.
/// `who` names a unit ("the tower", "Mira", ...).
pub fn action_words(w: &World, t: &Thought, a: u8, who: &dyn Fn(usize) -> String) -> String {
    let i = t.unit;
    let me = &w.units[i];
    let on = |j: Option<usize>| j.map(|j| format!(" {}", who(j))).unwrap_or_default();
    let target = if a == t.next { t.target } else { w.aim(i, a) };
    match a {
        sim::ACT_IDLE => "Wait".into(),
        1..=8 => {
            // Say what it walks toward (or away from), when that's clear.
            let (dx, dy) = DIRS[(a - 1) as usize];
            let mut best: Option<(f32, String)> = None;
            let mut things: Vec<((f32, f32), String)> = Vec::new();
            for s in t.sees.iter().chain(&t.remembers) {
                things.push(((s.x, s.y), who(s.j)));
            }
            if let Some(p) = t.hears {
                things.push((p, "the fight it hears".into()));
            }
            for &(k, x, y) in &t.calls {
                things.push(((x, y), format!("the \"{}\" call", call_words(k))));
            }
            for ((x, y), name) in things {
                let (ox, oy) = (x - me.x, y - me.y);
                let d = (ox * ox + oy * oy).sqrt().max(1e-3);
                let c = (ox * dx + oy * dy) / d;
                if c.abs() > 0.8 && best.as_ref().map_or(true, |(bc, _)| c.abs() / d > bc.abs()) {
                    best = Some((
                        c / d,
                        if c > 0.0 {
                            format!("Walk toward {name}")
                        } else {
                            format!("Back away from {name}")
                        },
                    ));
                }
            }
            best.map(|b| b.1)
                .unwrap_or_else(|| format!("Walk {}", compass(a)))
        }
        sim::ACT_HIT_UNIT | sim::ACT_HIT_BUILDING => format!("Hit{}", on(target)),
        sim::ACT_HEAL | sim::ACT_HEAL_2 => format!("Heal{}", on(target)),
        sim::ACT_DODGE => "Dodge roll".into(),
        sim::ACT_BLOCK => "Raise its shield".into(),
        sim::ACT_SKILL => {
            let skill = me.weapon.map_or("its skill", |wp| wp.skill_name());
            match target {
                Some(j) => format!("{skill} on {}", who(j)),
                None => format!("Use {skill}"),
            }
        }
        sim::ACT_CHARGE => format!("Charge at{}", on(target)),
        sim::ACT_CLOAK => "Turn invisible".into(),
        sim::ACT_BLINK => "Teleport away".into(),
        sim::ACT_SNARE => "Lay a trap".into(),
        sim::ACT_WARD => "Shield nearby allies".into(),
        sim::ACT_CALL_FOCUS..=sim::ACT_CALL_WATCH => {
            format!("Shout \"{}\"", call_words(a - sim::ACT_CALL_FOCUS))
        }
        _ => "?".into(),
    }
}

/// Keep 2D drawing inside `r` (None lifts it), in UI coordinates.
fn clip(r: Option<Rect>) {
    let mut gl = unsafe { get_internal_gl() };
    gl.flush();
    gl.quad_gl.scissor(r.map(|r| {
        let (vx, vy, s) = crate::render::screen();
        let d = screen_dpi_scale();
        (
            ((vx + r.x * s) * d) as i32,
            ((vy + r.y * s) * d) as i32,
            (r.w * s * d) as i32,
            (r.h * s * d) as i32,
        )
    }));
}

fn arrow(a: Vec2, b: Vec2, thick: f32, col: Color) {
    let d = b - a;
    if d.length() < 4.0 {
        return;
    }
    let n = d.normalize();
    let side = vec2(-n.y, n.x);
    let head = (thick * 3.5).max(10.0);
    let base = b - n * head;
    draw_line(a.x, a.y, base.x, base.y, thick, col);
    draw_triangle(b, base + side * head * 0.55, base - side * head * 0.55, col);
}

/// Text with a dark backing so it reads over the grass.
fn tag(s: &str, at: Vec2, size: f32, col: Color) {
    let w = text_width(s, size);
    draw_rectangle(
        at.x - w / 2.0 - 5.0,
        at.y - size * 0.8,
        w + 10.0,
        size * 1.05,
        Color::new(0.05, 0.05, 0.08, 0.75),
    );
    text(s, at.x - w / 2.0, at.y, size, col);
}

/// The troop's view drawn on the map: its sight cone, what it sees,
/// remembers and hears, and an arrow for what it is about to do.
pub fn draw_on_map(v: View, w: &World, t: &Thought, who: &dyn Fn(usize) -> String) {
    let me = &w.units[t.unit];
    let s = v.s();
    clip(Some(v.r));

    // Sight cone, brighter than the everyday one.
    let half = sim::SIGHT_COS.acos();
    let base = me.facing.1.atan2(me.facing.0);
    let n = 24;
    let edge = |k: usize| {
        let a = base - half + 2.0 * half * k as f32 / n as f32;
        let (dx, dy) = (a.cos(), a.sin());
        let mut r = sim::SIGHT_RANGE;
        for (d, p) in [(dx, me.x), (dy, me.y)] {
            if d > 1e-4 {
                r = r.min((sim::ARENA - p) / d);
            } else if d < -1e-4 {
                r = r.min(-p / d);
            }
        }
        v.p(me.x + r * dx, me.y + r * dy)
    };
    let c = v.p(me.x, me.y);
    let fill = Color::new(1.0, 0.95, 0.6, 0.10);
    for k in 0..n {
        draw_triangle(c, edge(k), edge(k + 1), fill);
    }
    let line = Color::new(1.0, 0.95, 0.6, 0.7);
    for k in [0, n] {
        let e = edge(k);
        draw_line(c.x, c.y, e.x, e.y, 2.0, line);
    }
    // "Always notices" bubble close around it, even from behind.
    v.ring(
        me.x,
        me.y,
        sim::TOUCH,
        1.5,
        Color::new(1.0, 0.95, 0.6, 0.35),
    );

    // What it remembers: a ghost where it last saw it.
    for m in &t.remembers {
        let a = 0.3 + 0.6 * m.fresh;
        let col = Color::new(MEMORY.r, MEMORY.g, MEMORY.b, a);
        let r = if w.units[m.j].kind.is_building() {
            w.units[m.j].radius + 0.3
        } else {
            0.7
        };
        v.ring(m.x, m.y, r, 2.0, col);
        tag("?", v.p3(m.x, m.y, 1.6), 22.0, col);
    }
    // What it sees, with a line of sight to each.
    for e in &t.sees {
        let u = &w.units[e.j];
        let r = if u.kind.is_building() {
            u.radius + 0.3
        } else {
            0.75
        };
        v.ring(u.x, u.y, r, 2.5, SEE);
        let p = v.p(u.x, u.y);
        draw_line(
            c.x,
            c.y,
            p.x,
            p.y,
            1.2,
            Color::new(SEE.r, SEE.g, SEE.b, 0.45),
        );
    }
    // Just spotted: still reacting.
    for &j in &t.spotted {
        let u = &w.units[j];
        v.ring(u.x, u.y, 0.8, 2.5, SPOT);
        tag("!", v.p3(u.x, u.y, 1.6), 22.0, SPOT);
    }
    // A fight it can hear but not see.
    if let Some((x, y)) = t.hears {
        let pulse = (crate::record::now() as f32 * 3.0).fract();
        for k in 0..3 {
            let r = 0.4 + (k as f32 + pulse) * 0.5;
            v.ring(
                x,
                y,
                r,
                2.0,
                Color::new(
                    HEAR.r,
                    HEAR.g,
                    HEAR.b,
                    0.8 * (1.0 - (k as f32 + pulse) / 3.0),
                ),
            );
        }
        tag("hears a fight", v.p3(x, y, 1.4), 16.0, HEAR);
    }
    // Calls from teammates it has taken in.
    for &(k, x, y) in &t.calls {
        let col = call_color(k);
        v.disc(x, y, 0.35, col);
        tag(call_words(k), v.p3(x, y, 1.0), 16.0, col);
    }

    // What it is about to do.
    let a = t.next;
    match a {
        1..=8 => {
            let (dx, dy) = DIRS[(a - 1) as usize];
            arrow(c, v.p(me.x + dx * 3.0, me.y + dy * 3.0), 5.0, PLAN);
        }
        _ => {
            if let Some(j) = t.target {
                let u = &w.units[j];
                let p = v.p(u.x, u.y);
                let col = if matches!(a, sim::ACT_HEAL | sim::ACT_HEAL_2) {
                    GOOD
                } else {
                    PLAN
                };
                arrow(c, p, 5.0, col);
            }
        }
    }
    // Pick ring and the plan in words over its head.
    let pulse = 0.5 + 0.5 * (crate::record::now() as f32 * 5.0).sin();
    v.ring(me.x, me.y, 0.9 + 0.1 * pulse, 3.0, PLAN);
    let words = format!("Next: {}", action_words(w, t, a, who));
    let mut at = v.p3(me.x, me.y, 2.6) - vec2(0.0, 6.0 * s / 20.0);
    let half_w = text_width(&words, 18.0) / 2.0 + 8.0;
    at.x = at.x.clamp(v.r.x + half_w, v.r.x + v.r.w - half_w);
    at.y = at.y.max(v.r.y + 22.0);
    tag(&words, at, 18.0, PLAN);
    clip(None);
}

/// Count units by what they are, e.g. "2 guards, a tower".
fn summary(w: &World, js: &[usize]) -> String {
    if js.is_empty() {
        return "nothing".into();
    }
    let mut groups: Vec<(&str, &str, usize)> = Vec::new();
    for &j in js {
        let (one, many) = match w.units[j].kind {
            Kind::Guard => ("a guard", "guards"),
            Kind::Tower => ("a tower", "towers"),
            Kind::Core => ("the Town Hall", "Town Halls"),
            Kind::Troop => ("an enemy troop", "enemy troops"),
        };
        match groups.iter_mut().find(|g| g.0 == one) {
            Some(g) => g.2 += 1,
            None => groups.push((one, many, 1)),
        }
    }
    groups
        .into_iter()
        .map(|(one, many, k)| {
            if k > 1 {
                format!("{k} {many}")
            } else {
                one.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The side panel: the same thoughts in words, and how sure it is of each
/// option.
pub fn draw_panel(r: Rect, w: &World, t: &Thought, name: &str, who: &dyn Fn(usize) -> String) {
    draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.07, 0.07, 0.11, 0.96));
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, PLAN);
    let x = r.x + 12.0;
    let mut y = r.y + 24.0;
    text(&format!("What {name} is thinking"), x, y, 20.0, PLAN);
    y += 24.0;
    let sees: Vec<usize> = t.sees.iter().map(|s| s.j).collect();
    let mems: Vec<usize> = t.remembers.iter().map(|s| s.j).collect();
    let line = |label: &str, body: String, col: Color, y: f32| {
        text(label, x, y, 16.0, col);
        text(&body, x + 104.0, y, 16.0, TEXT);
    };
    let mut seen = summary(w, &sees);
    if !t.spotted.is_empty() {
        seen = format!(
            "{seen}  (just spotted {}, reacting)",
            summary(w, &t.spotted)
        );
    }
    line("Sees", seen, SEE, y);
    y += 20.0;
    line(
        "Remembers",
        if mems.is_empty() {
            "nothing out of sight".into()
        } else {
            format!("{} out of sight", summary(w, &mems))
        },
        MEMORY,
        y,
    );
    y += 20.0;
    let mut hears = if t.hears.is_some() {
        "a fight it can't see".to_string()
    } else {
        "nothing".to_string()
    };
    if !t.calls.is_empty() {
        let calls: Vec<String> = t
            .calls
            .iter()
            .map(|c| format!("\"{}\"", call_words(c.0)))
            .collect();
        hears = format!("{hears}; team: {}", calls.join(" "));
    }
    line("Hears", hears, HEAR, y);
    y += 26.0;
    text(
        "Its options (how much its brain leans to each)",
        x,
        y,
        16.0,
        MUTED,
    );
    y += 8.0;
    let rows = (((r.y + r.h - y - 6.0) / 22.0) as usize).clamp(1, 4);
    for (k, &(a, p)) in t.choices.iter().take(rows).enumerate() {
        let col = if k == 0 { PLAN } else { MUTED };
        let bw = r.w - 24.0;
        let b = Rect::new(x, y + 4.0, bw, 18.0);
        draw_rectangle(b.x, b.y, b.w, b.h, Color::new(1.0, 1.0, 1.0, 0.05));
        draw_rectangle(
            b.x,
            b.y,
            b.w * p.clamp(0.0, 1.0),
            b.h,
            Color::new(col.r, col.g, col.b, 0.35),
        );
        let words = action_words(w, t, a, who);
        text(
            &words,
            b.x + 6.0,
            b.y + 14.0,
            16.0,
            if k == 0 { TEXT } else { MUTED },
        );
        let pct = format!("{:.0}%", p * 100.0);
        text(
            &pct,
            b.x + b.w - text_width(&pct, 16.0) - 6.0,
            b.y + 14.0,
            16.0,
            col,
        );
        y += 22.0;
    }
}
