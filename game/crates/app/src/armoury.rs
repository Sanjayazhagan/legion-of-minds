//! The armoury: unlock weapons with gold and pick what each troop fights with.
//! A troop gets better with a weapon the longer it trains holding it.

use crate::barracks::fmt_steps;
use crate::ui::*;
use crate::{Game, Screen};
use godcore::classes::ClassSpec;
use godcore::sim::{Control, Unit, World};
use godcore::weapons::{Weapon, MASTERY_STEPS, MASTERY_TITLES, MAX_MASTERY};
use macroquad::prelude::*;

pub struct ArmouryState {
    pub idx: usize,
}

/// The troop as the sim would field it with weapon `w`.
fn preview(spec: &ClassSpec, w: Weapon, mastery: u8) -> Unit {
    let mut s = spec.clone();
    s.weapon = Some(w);
    s.mastery = mastery;
    let mut world = World::new(1);
    let i = world.add_troop_on(spec_team(spec), &s, 5.0, 5.0, Control::External, 0);
    world.units[i].clone()
}

fn spec_team(spec: &ClassSpec) -> godcore::sim::Team {
    if spec.role == godcore::classes::Role::Defend {
        godcore::sim::Team::Defend
    } else {
        godcore::sim::Team::Attack
    }
}

/// Mastery title, and progress to the next level (1.0 at max).
pub fn mastery_progress(steps: u64) -> (u8, f32) {
    let l = godcore::weapons::mastery_level(steps);
    if l >= MAX_MASTERY {
        return (l, 1.0);
    }
    let (a, b) = (MASTERY_STEPS[l as usize], MASTERY_STEPS[l as usize + 1]);
    (l, (steps - a) as f32 / (b - a) as f32)
}

pub fn stat_line(u: &Unit) -> String {
    let mut s = format!("DMG {:.1} every {}t   range {:.1}   speed {:.2}", u.dmg, u.cd_max, u.range, u.speed);
    if u.heal > 0.0 {
        s += &format!("   heal {:.1}", u.heal);
    }
    s
}

impl Game {
    pub fn owns_weapon(&self, w: Weapon, spec: &ClassSpec) -> bool {
        w == Weapon::default_for(spec.main_class()) || self.profile.weapons.contains(&w)
    }

    pub fn armoury(&mut self, s: ArmouryState) -> Screen {
        if self.header("Armoury", true) || s.idx >= self.troops.len() {
            return Screen::Barracks;
        }
        let t = &self.troops[s.idx];
        let current = t.weapon();

        // ---- The troop (left)
        let l = Rect::new(20.0, 66.0, 380.0, H - 86.0);
        panel(l);
        crate::art::draw_showcase(vec2(l.x + l.w / 2.0, l.y + 150.0), 110.0, &t.spec);
        let (x, mut y) = (l.x + 20.0, l.y + 300.0);
        text(&t.name, x, y, 32.0, TEXT);
        y += 26.0;
        text(&format!("Level {} {}", t.level, t.spec.name), x, y, 18.0, MUTED);
        y += 40.0;
        text(current.name(), x, y, 26.0, ACCENT);
        y += 28.0;
        let steps = t.mastery_steps(current);
        let (lvl, frac) = mastery_progress(steps);
        text(&format!("Mastery: {} ({} / 5)", MASTERY_TITLES[lvl as usize], lvl), x, y, 20.0, TEXT);
        y += 12.0;
        bar(Rect::new(x, y, l.w - 40.0, 10.0), frac, ACCENT);
        y += 26.0;
        let next = if lvl >= MAX_MASTERY {
            "Fully mastered".to_string()
        } else {
            format!("{} more training steps to {}", fmt_steps(MASTERY_STEPS[lvl as usize + 1] - steps), MASTERY_TITLES[lvl as usize + 1])
        };
        text(&next, x, y, 17.0, MUTED);
        y += 36.0;
        let u = preview(&t.spec, current, lvl);
        text(&format!("Skill: {} every {:.1}s", current.skill_name(), u.skill_cd_max as f32 / 30.0), x, y, 18.0, TEXT);
        y += 30.0;
        crate::recruit::wrap(
            "Every mastery level adds 5% damage and makes the skill recharge 6% faster. From Expert (level 3) the skill is 30% stronger. Mastery grows with training while holding the weapon, and each weapon keeps its own.",
            x,
            y,
            l.w - 40.0,
            16.0,
            MUTED,
        );

        // ---- Weapons (right)
        let (rx, ry, rw) = (420.0, 66.0, W - 440.0);
        let ch = (H - 86.0 - 6.0 * 8.0) / 7.0;
        let mut equip = None;
        let mut unlock = None;
        for (k, w) in Weapon::ALL.into_iter().enumerate() {
            let r = Rect::new(rx, ry + k as f32 * (ch + 8.0), rw, ch);
            let on = w == current;
            draw_rectangle(r.x, r.y, r.w, r.h, if on { PANEL_HI } else { PANEL });
            draw_rectangle_lines(r.x, r.y, r.w, r.h, if on { 2.0 } else { 1.0 }, if on { ACCENT } else { LINE });
            let own = self.owns_weapon(w, &t.spec);
            let m = t.mastery_of(w);
            text(w.name(), r.x + 16.0, r.y + 26.0, 22.0, if own { TEXT } else { MUTED });
            if w == Weapon::default_for(t.spec.main_class()) {
                text("class weapon", r.x + 26.0 + text_width(w.name(), 22.0), r.y + 26.0, 16.0, MUTED);
            }
            text(w.blurb(), r.x + 16.0, r.y + 48.0, 16.0, MUTED);
            let pu = preview(&t.spec, w, m);
            text(&stat_line(&pu), r.x + 16.0, r.y + 70.0, 16.0, TEXT);
            let ms = t.mastery_steps(w);
            let (ml, mf) = mastery_progress(ms);
            let mx = r.x + r.w - 420.0;
            text(&format!("{} {}", MASTERY_TITLES[ml as usize], if ms > 0 { format!("({})", fmt_steps(ms)) } else { String::new() }), mx, r.y + 70.0, 16.0, if ml > 0 { ACCENT } else { MUTED });
            bar(Rect::new(mx, r.y + 76.0, 200.0, 5.0), mf, ACCENT);
            let b = Rect::new(r.x + r.w - 180.0, r.y + (r.h - 40.0) / 2.0, 164.0, 40.0);
            if on {
                self.ui.button_ex(b, "Equipped", false, true);
            } else if own {
                if self.ui.button(b, "Equip") {
                    equip = Some(w);
                }
            } else {
                let cost = w.stats().cost;
                if self.ui.button_ex(b, &format!("Unlock {cost}g"), self.profile.gold >= cost, false) {
                    unlock = Some(w);
                }
            }
        }
        if let Some(w) = unlock {
            self.profile.gold -= w.stats().cost;
            self.profile.weapons.push(w);
            self.troops[s.idx].equip(w);
            self.save_all();
            self.toast(format!("Unlocked the {} for every troop", w.name()));
        }
        if let Some(w) = equip {
            self.troops[s.idx].equip(w);
            self.save_all();
            let t = &self.troops[s.idx];
            self.toast(format!("{} now fights with the {}. Train to build mastery.", t.name, w.name()));
        }
        Screen::Armoury(s)
    }
}
