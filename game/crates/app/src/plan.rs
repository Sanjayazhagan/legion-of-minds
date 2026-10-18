//! Typed percentage fields and the training-plan editor, shared by the
//! recruit and training screens.

use crate::ui::*;
use godcore::classes::{ClassSpec, Role, TrainPlan, FOCUS_NAMES_ATTACK, FOCUS_NAMES_DEFEND};
use macroquad::prelude::*;

/// Which number field has the keyboard. Click a field to type into it; the
/// first digit replaces the value, later ones append.
#[derive(Default)]
pub struct NumEdit {
    pub focus: Option<u32>,
    fresh: bool,
    claimed: bool,
    digits: Vec<u32>,
    back: bool,
    /// Characters typed while no field is focused (e.g. for a name box).
    pub other: Vec<char>,
    pub other_back: bool,
}

impl NumEdit {
    /// Read this frame's keys. Call once before drawing fields.
    pub fn begin(&mut self) {
        self.claimed = false;
        self.digits.clear();
        self.other.clear();
        let chars = crate::record::chars();
        let back = crate::record::backspace();
        if self.focus.is_some() {
            self.digits = chars.iter().filter_map(|c| c.to_digit(10)).collect();
            self.back = back;
            if chars.iter().any(|c| *c == '\r' || *c == '\n' || *c == '\t') {
                self.focus = None;
            }
            self.other_back = false;
        } else {
            self.other = chars;
            self.other_back = back;
            self.back = false;
        }
    }

    /// Clicking anywhere that is not a field drops the focus.
    pub fn end(&mut self, ui: &Ui) {
        if ui.clicked && !self.claimed {
            self.focus = None;
        }
    }

    /// `[-] [ 40% ] [+]` at (x, y), 156 x 34. Returns the new value.
    pub fn field(&mut self, ui: &Ui, id: u32, x: f32, y: f32, v: u32, enabled: bool) -> u32 {
        let mut v = v.min(100);
        let minus = Rect::new(x, y, 34.0, 34.0);
        let boxr = Rect::new(x + 40.0, y, 76.0, 34.0);
        let plus = Rect::new(x + 122.0, y, 34.0, 34.0);
        if ui.button_ex(minus, "-", enabled && v > 0, false) {
            v = v.saturating_sub(5);
            self.focus = None;
            self.claimed = true;
        }
        if ui.button_ex(plus, "+", enabled && v < 100, false) {
            v = (v + 5).min(100);
            self.focus = None;
            self.claimed = true;
        }
        let focused = self.focus == Some(id);
        if enabled && ui.clicked && ui.hover(boxr) {
            self.focus = Some(id);
            self.fresh = true;
            self.claimed = true;
        }
        if focused {
            for &d in &self.digits {
                v = if self.fresh { d } else { (v * 10 + d).min(100) };
                self.fresh = false;
            }
            if self.back {
                v /= 10;
                self.fresh = false;
            }
        }
        let focused = self.focus == Some(id);
        draw_rectangle(boxr.x, boxr.y, boxr.w, boxr.h, if focused { PANEL_HI } else { BG });
        draw_rectangle_lines(boxr.x, boxr.y, boxr.w, boxr.h, if focused { 2.0 } else { 1.0 }, if focused { ACCENT } else { LINE });
        let caret = if focused && (crate::record::now() * 2.0) as i64 % 2 == 0 { "|" } else { "" };
        let label = format!("{v}%{caret}");
        let col = if enabled { TEXT } else { MUTED };
        text(&label, boxr.x + (boxr.w - text_width(&format!("{v}%"), 22.0)) / 2.0, boxr.y + 24.0, 22.0, col);
        v
    }
}

/// A thin share bar.
pub fn share_bar(x: f32, y: f32, w: f32, v: u32, col: Color) {
    draw_rectangle(x, y, w, 6.0, Color::new(0.2, 0.21, 0.28, 1.0));
    draw_rectangle(x, y, w * v.min(100) as f32 / 100.0, 6.0, col);
}

/// "Total 100%" in green, or what is missing in red.
pub fn total_line(x: f32, y: f32, total: u32) {
    if total == 100 {
        text("Total 100%", x, y, 20.0, GOOD);
    } else if total < 100 {
        text(&format!("Total {total}%: add {}% more", 100 - total), x, y, 20.0, BAD);
    } else {
        text(&format!("Total {total}%: remove {}%", total - 100), x, y, 20.0, BAD);
    }
}

/// Scale percentages so they add up to 100 (largest remainders get the rest).
pub fn normalise(vals: &mut [u32], active: &[bool]) {
    let total: u32 = vals.iter().zip(active).filter(|(_, a)| **a).map(|(v, _)| *v).sum();
    let n = active.iter().filter(|a| **a).count() as u32;
    if n == 0 {
        return;
    }
    if total == 0 {
        for (v, a) in vals.iter_mut().zip(active) {
            *v = if *a { 100 / n } else { 0 };
        }
    } else {
        for (v, a) in vals.iter_mut().zip(active) {
            *v = if *a { *v * 100 / total } else { 0 };
        }
    }
    let mut left = 100 - vals.iter().sum::<u32>().min(100);
    let mut i = 0;
    while left > 0 {
        if active[i % vals.len()] && (total == 0 || vals[i % vals.len()] > 0) {
            vals[i % vals.len()] += 1;
            left -= 1;
        }
        i += 1;
        if i > 1000 {
            break;
        }
    }
}

/// Field ids used by the plan editor (the class mix uses 0..20).
const SOLO: u32 = 100;
const SELF_PLAY: u32 = 101;
const FOCUS: u32 = 110;

/// Drill mix and reward focus, in percentages. `base` is the class mix
/// without a plan. Returns true if the plan is valid (focus sums to 100).
pub fn plan_editor(ui: &Ui, ed: &mut NumEdit, r: Rect, base: &ClassSpec, plan: &mut TrainPlan) -> bool {
    let (x, mut y) = (r.x, r.y);
    let fx = r.x + r.w - 156.0;
    let attack = base.role == Role::Attack;
    if attack {
        text("Solo raids", x, y + 23.0, 20.0, TEXT);
        text(&format!("squad raids {}%", 100 - plan.solo.min(100)), x + 120.0, y + 23.0, 16.0, MUTED);
        plan.solo = ed.field(ui, SOLO, fx, y, plan.solo, true);
        y += 44.0;
    }
    text("Vs your own troops", x, y + 23.0, 20.0, TEXT);
    plan.self_play = ed.field(ui, SELF_PLAY, fx, y, plan.self_play, true);
    y += 54.0;
    text("Focus: what the reward cares about", x, y + 14.0, 18.0, MUTED);
    y += 26.0;
    let names = if attack { FOCUS_NAMES_ATTACK } else { FOCUS_NAMES_DEFEND };
    let active = TrainPlan::active(base);
    for i in 0..4 {
        if !active[i] {
            plan.focus[i] = 0;
            continue;
        }
        text(names[i], x, y + 23.0, 20.0, TEXT);
        plan.focus[i] = ed.field(ui, FOCUS + i as u32, fx, y, plan.focus[i], true);
        share_bar(x, y + 32.0, fx - x - 16.0, plan.focus[i], ACCENT);
        y += 44.0;
    }
    let total: u32 = plan.focus.iter().sum();
    total_line(x, y + 20.0, total);
    if total != 100 && ui.button(Rect::new(fx, y, 156.0, 30.0), "Fix to 100") {
        normalise(&mut plan.focus, &active);
    }
    total == 100
}
