//! The troop stat panel: one card with everything about a troop, opened from
//! the Barracks. Battle numbers come from the real battle rules (the same
//! code that puts the troop on the field), so weapon and mastery are counted.

use crate::barracks::fmt_steps;
use crate::ui::*;
use crate::Game;
use godcore::classes::{BaseClass, ClassSpec, Role, TALENT_MAX};
use godcore::sim::{Control, Unit, World};
use godcore::troop::Troop;
use macroquad::prelude::*;
use std::cell::Cell;

thread_local! {
    // Screenshot hook: BAG_STATS=<n> opens the panel on roster troop n.
    static OPEN: Cell<bool> = Cell::new(std::env::var_os("BAG_STATS").is_some());
    static HOOK: Cell<Option<usize>> = Cell::new(std::env::var("BAG_STATS").ok().and_then(|v| v.parse().ok()));
}

pub fn is_open() -> bool {
    OPEN.with(|o| o.get())
}

pub fn set_open(v: bool) {
    OPEN.with(|o| o.set(v));
}

/// The troop as it stands on the battlefield.
pub fn battle_unit(spec: &ClassSpec) -> Unit {
    let mut sim = World::new(1);
    let i = sim.add_troop(spec, 10.0, 10.0, Control::External, 0);
    sim.units[i].clone()
}

/// Damage a unit deals over 10 ticks of steady hitting.
fn output(u: &Unit) -> f32 {
    u.dmg * 10.0 / u.cd_max.max(1) as f32
}

/// The best value of each battle number across every class, for the bars.
struct Tops {
    hp: f32,
    dmg: f32,
    out: f32,
    rate: f32,
    range: f32,
    speed: f32,
    heal: f32,
}

fn tops() -> Tops {
    let mut t = Tops { hp: 1.0, dmg: 1.0, out: 1.0, rate: 1.0, range: 1.0, speed: 0.01, heal: 1.0 };
    for c in BaseClass::ALL.iter().chain(BaseClass::DEFENDERS.iter()) {
        let u = battle_unit(&c.spec());
        t.hp = t.hp.max(u.max_hp);
        t.dmg = t.dmg.max(u.dmg);
        t.out = t.out.max(output(&u));
        t.rate = t.rate.max(10.0 / u.cd_max.max(1) as f32);
        t.range = t.range.max(u.range);
        t.speed = t.speed.max(u.speed);
        t.heal = t.heal.max(u.heal);
    }
    t
}

impl Game {
    /// Draw the stat panel over the Barracks for the selected troop.
    pub fn stat_panel(&mut self) {
        if let Some(n) = HOOK.with(|h| h.take()) {
            self.selected = Some(n);
        }
        let Some(i) = self.selected.filter(|&i| i < self.troops.len()) else {
            set_open(false);
            return;
        };
        draw_rectangle(0.0, 0.0, W, H, Color::new(0.0, 0.0, 0.0, 0.6));
        let p = Rect::new(110.0, 52.0, W - 220.0, H - 84.0);
        draw_rectangle(p.x, p.y, p.w, p.h, BG);
        draw_rectangle_lines(p.x, p.y, p.w, p.h, 2.0, ACCENT);

        // Flip through the roster without closing.
        let n = self.troops.len();
        let mut flip = 0i32;
        if n > 1 {
            if self.ui.button(Rect::new(p.x + p.w - 290.0, p.y + 16.0, 44.0, 36.0), "<") || is_key_pressed(KeyCode::Left) {
                flip = -1;
            }
            if self.ui.button(Rect::new(p.x + p.w - 238.0, p.y + 16.0, 44.0, 36.0), ">") || is_key_pressed(KeyCode::Right) {
                flip = 1;
            }
        }
        if self.ui.button(Rect::new(p.x + p.w - 182.0, p.y + 16.0, 166.0, 36.0), "Close (Esc)")
            || is_key_pressed(KeyCode::Escape)
            || (self.ui.clicked && !p.contains(self.ui.mouse))
        {
            set_open(false);
            return;
        }
        if flip != 0 {
            self.selected = Some((i as i32 + flip).rem_euclid(n as i32) as usize);
            return;
        }

        let t = &self.troops[i];
        draw_troop_stats(t, p);
        let help = "Bars compare with the strongest class.  Green or red % is what the weapon and its mastery change.  Left/right arrows: next troop.";
        text(help, (p.x + (p.w - text_width(help, 16.0)) / 2.0).round(), p.y + p.h - 18.0, 16.0, MUTED);
    }
}

fn draw_troop_stats(t: &Troop, p: Rect) {
    let spec = &t.spec;
    let u = battle_unit(spec);
    // The same troop with its class weapon and no mastery: what gear adds.
    let mut plain = spec.clone();
    plain.weapon = None;
    plain.mastery = 0;
    let b = battle_unit(&plain);
    let top = tops();

    // ---- Header
    let (x, y) = (p.x + 24.0, p.y + 46.0);
    crate::art::draw_portrait(vec2(x + 30.0, y - 8.0), 30.0, spec);
    text(&t.name, x + 72.0, y, 34.0, TEXT);
    let role = if spec.role == Role::Defend { "defender" } else { "attacker" };
    let kind = if spec.is_hybrid() { "hybrid" } else { "class" };
    let mut sub = format!("Level {} {}  ·  {kind}  ·  {role}", t.level, spec.name);
    let mix = spec.mix_label();
    if !mix.is_empty() {
        sub += &format!("  ·  {mix}");
    }
    text(&sub, x + 72.0, y + 24.0, 18.0, MUTED);

    // ---- Left: battle numbers
    let col = 440.0;
    let (lx, mut ly) = (p.x + 24.0, p.y + 112.0);
    let w = spec.weapon();
    let ml = spec.mastery as usize;
    text("In battle", lx, ly, 24.0, TEXT);
    let gear = format!("with {} ({})", w.name(), godcore::weapons::MASTERY_TITLES[ml]);
    text(&gear, lx + col - text_width(&gear, 16.0), ly, 16.0, MUTED);
    ly += 16.0;
    let rate = |u: &Unit| 10.0 / u.cd_max.max(1) as f32;
    let mut rows: Vec<(&str, f32, f32, f32, String)> = vec![
        ("Health", u.max_hp, b.max_hp, top.hp, format!("{:.0}", u.max_hp)),
        ("Damage per hit", u.dmg, b.dmg, top.dmg, format!("{:.1}", u.dmg)),
        ("Attack speed", rate(&u), rate(&b), top.rate, format!("every {} ticks", u.cd_max)),
        ("Damage output", output(&u), output(&b), top.out, format!("{:.0} / 10 ticks", output(&u))),
        ("Range", u.range, b.range, top.range, format!("{:.1}", u.range)),
        ("Move speed", u.speed, b.speed, top.speed, format!("{:.2}", u.speed)),
    ];
    if u.heal > 0.0 {
        rows.push(("Healing", u.heal, b.heal, top.heal, format!("{:.1} within {:.1}", u.heal, u.heal_range)));
    }
    // Battle stats: add a row to `rows` above to show a new one.
    for (label, v, base, max, shown) in rows {
        ly += 30.0;
        stat_row(lx, ly, col, label, v / max, &shown, v / base.max(1e-6) - 1.0);
    }
    ly += 34.0;
    let mut extra = Vec::new();
    if u.splash > 0.0 {
        extra.push(format!("Splash {:.1} (half damage)", u.splash));
    }
    if u.block_taken < 0.995 {
        extra.push(format!("Blocking stops {:.0}%", (1.0 - u.block_taken) * 100.0));
    }
    extra.push(format!("Area talents reach {} of {TALENT_MAX}", u.talents));
    text(&extra.join("  ·  "), lx, ly, 17.0, MUTED);
    ly += 24.0;
    let power = if u.skill_power > 1.0 { ", 30% stronger" } else { "" };
    text(
        &format!("Skill: {}, recharges in {} ticks{power}", w.skill_name(), u.skill_cd_max),
        lx,
        ly,
        17.0,
        ACCENT,
    );

    // Moves
    ly += 36.0;
    text("Class moves", lx, ly, 22.0, TEXT);
    let kit = spec.moves();
    if kit.is_empty() {
        text("none", lx + 140.0, ly, 18.0, MUTED);
    }
    for m in kit {
        ly += 26.0;
        let f = t.move_practice(m);
        text(m.name(), lx, ly, 18.0, TEXT);
        bar(Rect::new(lx + 150.0, ly - 12.0, col - 300.0, 10.0), f, SKY);
        let s = format!("{:.0}% practised", f * 100.0);
        text(&s, lx + col - text_width(&s, 16.0), ly, 16.0, MUTED);
    }

    // ---- Right: shape, brain and record
    let rx = p.x + 24.0 + col + 50.0;
    let rw = p.x + p.w - 24.0 - rx;
    let mut ry = p.y + 112.0;
    text("Shape", rx, ry, 24.0, TEXT);
    let axes = [
        ("Health", u.max_hp / top.hp),
        ("Power", output(&u) / top.out),
        ("Range", u.range / top.range),
        ("Speed", u.speed / top.speed),
        ("Support", u.heal / top.heal),
        ("Training", (t.steps as f32 / godcore::troop::level_cap(godcore::troop::MAX_LEVEL) as f32).sqrt()),
    ];
    radar(vec2(rx + rw * 0.5, ry + 112.0), 84.0, &axes, rgb(spec.color));
    ry += 236.0;

    text("Brain and training", rx, ry, 22.0, TEXT);
    ry += 26.0;
    let ms = t.mastery_steps(w);
    let (_, mf) = crate::armoury::mastery_progress(ms);
    let next = if ml < godcore::weapons::MAX_MASTERY as usize {
        format!(", {:.0}% to {}", mf * 100.0, godcore::weapons::MASTERY_TITLES[ml + 1])
    } else {
        String::new()
    };
    // Add new troop stats here: (label, value, optional 0..1 progress bar).
    let lines: Vec<(&str, String, Option<f32>)> = vec![
        ("Brain", format!("{} neurons per layer, {} numbers", t.actor.dims[1], t.brain_params()), None),
        ("Trained", format!("{} of {} steps", fmt_steps(t.steps), fmt_steps(t.cap())), Some(t.steps as f32 / t.cap() as f32)),
        ("Sessions", format!("{}  ·  team training {}", t.sessions, fmt_steps(t.team_steps)), None),
        ("Weapon mastery", format!("{}{next}", godcore::weapons::MASTERY_TITLES[ml]), None),
    ];
    for (k, v, progress) in lines {
        kv(rx, ry, rw, k, &v);
        if let Some(f) = progress {
            bar(Rect::new(rx, ry + 6.0, rw, 6.0), f, SKY);
            ry += 6.0;
        }
        ry += 24.0;
    }
    ry += 12.0;

    let sk = &t.skills;
    text("Drill record", rx, ry, 22.0, TEXT);
    if sk.episodes == 0 {
        text("Not trained yet. A fresh brain acts at random.", rx, ry + 28.0, 17.0, MUTED);
        return;
    }
    let s = format!("last {} drills", sk.episodes);
    text(&s, rx + rw - text_width(&s, 16.0), ry, 16.0, MUTED);
    let won = if spec.role == Role::Defend { "Base held" } else { "Drills won" };
    let mut rec: Vec<(&str, f32, Color)> = vec![
        (won, sk.win, GOOD),
        ("Survived", sk.survival, SKY),
        ("Hits on guards", sk.guard_share, ACCENT),
        ("Hits on buildings", sk.building_share, ACCENT),
    ];
    if spec.role == Role::Defend {
        rec[2].0 = "Hits on raiders";
        rec[3].0 = "Buildings lost";
    }
    for (label, v, c) in rec {
        ry += 24.0;
        text(label, rx, ry, 17.0, TEXT);
        bar(Rect::new(rx + 150.0, ry - 11.0, rw - 210.0, 10.0), v, c);
        let s = format!("{:.0}%", v * 100.0);
        text(&s, rx + rw - text_width(&s, 17.0), ry, 17.0, MUTED);
    }
    ry += 24.0;
    let mut tail = Vec::new();
    if spec.can_heal() {
        tail.push(format!("heals {:.0} HP a drill", sk.healed));
    }
    if let Some(best) = t.curve.iter().map(|c| c.win).chain([sk.win]).reduce(f32::max) {
        tail.push(format!("best win rate {:.0}%", best * 100.0));
    }
    if !tail.is_empty() {
        let mut s = tail.join("  ·  ");
        s[..1].make_ascii_uppercase();
        text(&s, rx, ry, 16.0, MUTED);
    }
}

/// Label, bar against the best class, the value, and what gear adds.
fn stat_row(x: f32, y: f32, w: f32, label: &str, frac: f32, shown: &str, gain: f32) {
    text(label, x, y, 18.0, TEXT);
    let bw = w - 300.0;
    bar(Rect::new(x + 150.0, y - 12.0, bw, 10.0), frac, rgb([0.55, 0.75, 0.98]));
    text(shown, x + 160.0 + bw, y, 17.0, TEXT);
    if gain.abs() >= 0.005 {
        let s = format!("{:+.0}%", gain * 100.0);
        text(&s, x + w - text_width(&s, 16.0), y, 16.0, if gain > 0.0 { GOOD } else { BAD });
    }
}

fn kv(x: f32, y: f32, w: f32, k: &str, v: &str) {
    text(k, x, y, 17.0, MUTED);
    text(v, x + w - text_width(v, 17.0), y, 17.0, TEXT);
}

/// A spider chart of 0..1 values, one axis each.
fn radar(c: Vec2, r: f32, axes: &[(&str, f32)], fill: Color) {
    let n = axes.len();
    let at = |k: usize, f: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::TAU / n as f32;
        c + vec2(a.cos(), a.sin()) * r * f
    };
    let grid = Color::new(0.25, 0.27, 0.35, 1.0);
    for ring in 1..=4 {
        let f = ring as f32 / 4.0;
        for k in 0..n {
            let (a, b) = (at(k, f), at((k + 1) % n, f));
            draw_line(a.x, a.y, b.x, b.y, 1.0, grid);
        }
    }
    let shape: Vec<Vec2> = (0..n).map(|k| at(k, axes[k].1.clamp(0.04, 1.0))).collect();
    let soft = Color::new(fill.r, fill.g, fill.b, 0.35);
    for k in 0..n {
        draw_triangle(c, shape[k], shape[(k + 1) % n], soft);
    }
    for k in 0..n {
        let e = at(k, 1.0);
        draw_line(c.x, c.y, e.x, e.y, 1.0, grid);
        let (a, b) = (shape[k], shape[(k + 1) % n]);
        draw_line(a.x, a.y, b.x, b.y, 2.0, fill);
        draw_circle(a.x, a.y, 3.0, fill);
        let l = at(k, 1.22);
        let tw = text_width(axes[k].0, 16.0);
        text(axes[k].0, l.x - tw / 2.0, l.y + 5.0, 16.0, MUTED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battle_numbers_match_the_rules() {
        let spec = BaseClass::Fighter.spec();
        let u = battle_unit(&spec);
        assert_eq!(u.max_hp, spec.hp);
        assert_eq!(u.dmg, spec.dmg);
        assert_eq!(u.cd_max, spec.cooldown);
        // A crossbow turns a fighter into a shooter.
        let mut s = spec.clone();
        s.weapon = Some(godcore::weapons::Weapon::Crossbow);
        assert!(battle_unit(&s).range > 2.5);
        // Mastery adds damage.
        let mut m = spec.clone();
        m.mastery = 3;
        assert!(battle_unit(&m).dmg > u.dmg);
    }
}
