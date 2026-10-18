//! Recruiting: mix any number of classes by percentage, set a training plan,
//! name the troop.

use crate::plan::{normalise, plan_editor, share_bar, total_line, NumEdit};
use crate::save::RECRUIT_COST;
use crate::ui::*;
use crate::{Game, Screen};
use godcore::classes::{BaseClass, ClassSpec, Role, TrainPlan};
use godcore::troop::{random_name, Troop};
use macroquad::prelude::*;

pub struct RecruitState {
    side: Role,
    /// Percent per class of the side, in `classes(side)` order.
    mix: Vec<u32>,
    plan: TrainPlan,
    /// False while the plan just follows the class mix's own defaults.
    plan_touched: bool,
    ed: NumEdit,
    name: String,
}

fn classes(side: Role) -> Vec<BaseClass> {
    if side == Role::Attack {
        BaseClass::ALL.to_vec()
    } else {
        BaseClass::DEFENDERS.to_vec()
    }
}

impl RecruitState {
    pub fn new(name: String) -> Self {
        let mut s = RecruitState {
            side: Role::Attack,
            mix: Vec::new(),
            plan: TrainPlan::default_for(&BaseClass::Fighter.spec()),
            plan_touched: false,
            ed: NumEdit::default(),
            name,
        };
        s.set_side(Role::Attack);
        s
    }

    fn set_side(&mut self, side: Role) {
        self.side = side;
        self.mix = vec![0; classes(side).len()];
        self.mix[0] = 100;
        self.plan_touched = false;
        self.ed.focus = None;
    }

    fn parts(&self) -> Vec<(BaseClass, f32)> {
        classes(self.side)
            .into_iter()
            .zip(&self.mix)
            .filter(|(_, v)| **v > 0)
            .map(|(c, v)| (c, *v as f32))
            .collect()
    }

    /// The class mix without a plan, if any class is in it.
    fn base(&self) -> Option<ClassSpec> {
        let p = self.parts();
        (!p.is_empty()).then(|| ClassSpec::blend(&p))
    }
}

impl Game {
    /// A random name no troop in the roster already has (when possible).
    pub fn fresh_name(&mut self) -> String {
        for _ in 0..30 {
            let n = random_name(&mut self.rng);
            if !self.troops.iter().any(|t| t.name == n) {
                return n;
            }
        }
        random_name(&mut self.rng)
    }

    pub fn recruit(&mut self, mut s: RecruitState) -> Screen {
        if self.header("Recruit a troop", true) {
            return Screen::Barracks;
        }
        s.ed.begin();
        let x0 = 20.0;
        let attack = s.side == Role::Attack;
        if self.ui.button_ex(Rect::new(440.0, 64.0, 150.0, 32.0), "Attackers", true, attack) && !attack {
            s.set_side(Role::Attack);
        }
        if self.ui.button_ex(Rect::new(600.0, 64.0, 150.0, 32.0), "Defenders", true, !attack) && attack {
            s.set_side(Role::Defend);
        }
        text("1. Mix classes", x0, 90.0, 24.0, TEXT);

        // ---- Class mix: one typed percentage per class.
        let list = classes(s.side);
        for (k, c) in list.iter().enumerate() {
            let r = Rect::new(x0, 106.0 + k as f32 * 64.0, 740.0, 58.0);
            let on = s.mix[k] > 0;
            draw_rectangle(r.x, r.y, r.w, r.h, if on { PANEL_HI } else { PANEL });
            draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, if on { ACCENT } else { LINE });
            let spec = c.spec();
            crate::art::draw_showcase(vec2(r.x + 30.0, r.y + 29.0), 22.0, &spec);
            text(c.name(), r.x + 62.0, r.y + 25.0, 24.0, TEXT);
            let blurb = c.blurb().trim_start_matches("Defender. ");
            let short = blurb.split(". ").next().unwrap_or(blurb).trim_end_matches('.');
            text(short, r.x + 62.0, r.y + 47.0, 15.0, MUTED);
            s.mix[k] = s.ed.field(&self.ui, k as u32, r.x + 420.0, r.y + 12.0, s.mix[k], true);
            share_bar(r.x + 592.0, r.y + 26.0, 132.0, s.mix[k], rgb(spec.color));
        }
        let ty = 106.0 + list.len() as f32 * 64.0 + 22.0;
        let total: u32 = s.mix.iter().sum();
        total_line(x0, ty, total);
        let all = vec![true; s.mix.len()];
        if self.ui.button(Rect::new(x0 + 300.0, ty - 22.0, 140.0, 32.0), "Fix to 100") {
            let active: Vec<bool> = s.mix.iter().map(|v| *v > 0).collect();
            normalise(&mut s.mix, if active.iter().any(|a| *a) { &active } else { &all });
        }
        if self.ui.button(Rect::new(x0 + 450.0, ty - 22.0, 140.0, 32.0), "Even split") {
            let active: Vec<bool> = s.mix.iter().map(|v| *v > 0).collect();
            let active = if active.iter().filter(|a| **a).count() >= 2 { active } else { all.clone() };
            for (v, a) in s.mix.iter_mut().zip(&active) {
                *v = if *a { 1 } else { 0 };
            }
            normalise(&mut s.mix, &active);
        }
        if self.ui.button(Rect::new(x0 + 600.0, ty - 22.0, 140.0, 32.0), "Clear") {
            s.mix.iter_mut().for_each(|v| *v = 0);
        }

        // ---- Training plan
        let px = 790.0;
        text("2. Training plan", px, 90.0, 24.0, TEXT);
        let base = s.base();
        let mut plan_ok = true;
        if let Some(b) = &base {
            if !s.plan_touched {
                s.plan = TrainPlan::default_for(b);
            }
            let before = s.plan;
            plan_ok = plan_editor(&self.ui, &mut s.ed, Rect::new(px, 106.0, 470.0, 360.0), b, &mut s.plan);
            if s.plan != before {
                s.plan_touched = true;
            }
            if s.plan_touched && self.ui.button(Rect::new(px, ty - 22.0, 220.0, 32.0), "Back to class default") {
                s.plan_touched = false;
            }
            if !s.plan_touched {
                text("Follows the class mix until you change it.", px, ty, 16.0, MUTED);
            }
        } else {
            text("Pick at least one class.", px, 130.0, 20.0, MUTED);
        }

        // ---- Name
        let ny = ty + 30.0;
        text("3. Name", x0, ny + 4.0, 24.0, TEXT);
        let nr = Rect::new(x0 + 110.0, ny - 24.0, 300.0, 40.0);
        let name_focus = s.ed.focus.is_none();
        draw_rectangle(nr.x, nr.y, nr.w, nr.h, BG);
        draw_rectangle_lines(nr.x, nr.y, nr.w, nr.h, 2.0, if name_focus { LINE } else { PANEL_HI });
        for &ch in &s.ed.other {
            if (ch.is_alphanumeric() || ch == ' ' || ch == '-') && s.name.chars().count() < 16 {
                s.name.push(ch);
            }
        }
        if s.ed.other_back {
            s.name.pop();
        }
        let caret = if name_focus && (crate::record::now() * 2.0) as i64 % 2 == 0 { "|" } else { "" };
        text(&format!("{}{caret}", s.name), nr.x + 12.0, nr.y + 28.0, 24.0, TEXT);
        if self.ui.button(Rect::new(nr.x + 310.0, nr.y, 110.0, 40.0), "Random") {
            s.name = self.fresh_name();
        }
        if self.ui.hover(nr) && self.ui.clicked {
            s.ed.focus = None;
        }

        // ---- Preview
        let spec = base.as_ref().map(|b| {
            if s.plan_touched && plan_ok {
                let mut sp = b.clone();
                sp.apply_plan(s.plan);
                sp
            } else {
                b.clone()
            }
        });
        let pr = Rect::new(x0, ny + 28.0, 920.0, 800.0 - (ny + 28.0) - 12.0);
        panel(pr);
        if let Some(spec) = &spec {
            crate::art::draw_showcase(vec2(pr.x + 54.0, pr.y + pr.h / 2.0), 46.0, spec);
            let shown = if s.name.trim().is_empty() { "?" } else { s.name.trim() };
            text(&format!("{shown} the {}", spec.name), pr.x + 116.0, pr.y + 30.0, 28.0, TEXT);
            let mix = spec.mix_label();
            text(if mix.is_empty() { "Pure class" } else { &mix }, pr.x + 116.0, pr.y + 54.0, 17.0, ACCENT);
            text(&spec.describe(), pr.x + 116.0, pr.y + 76.0, 17.0, MUTED);
            let w = spec.weights;
            let reward = if spec.role == Role::Defend {
                format!(
                    "Reward: raiders x{:.1}, healing x{:.1}, building loss x{:.1}, dying x{:.1}. Self-play {:.0}%",
                    w.unit_dmg, w.heal, w.building_dmg, -w.death, spec.drills.self_play * 100.0
                )
            } else {
                let t = (spec.drills.solo + spec.drills.squad).max(1e-6);
                format!(
                    "Reward: buildings x{:.1}, troops x{:.1}, healing x{:.1}, dying x{:.1}. Drills {:.0}% solo, {:.0}% squad, self-play {:.0}%",
                    w.building_dmg,
                    w.unit_dmg,
                    w.heal,
                    -w.death,
                    spec.drills.solo / t * 100.0,
                    spec.drills.squad / t * 100.0,
                    spec.drills.self_play * 100.0
                )
            };
            text(&reward, pr.x + 116.0, pr.y + 98.0, 16.0, MUTED);
        }

        let afford = self.profile.gold >= RECRUIT_COST;
        let ok = afford && !s.name.trim().is_empty() && total == 100 && plan_ok && spec.is_some();
        let br = Rect::new(960.0, 800.0 - 68.0, 300.0, 56.0);
        if self.ui.button_ex(br, &format!("Recruit ({RECRUIT_COST}g) and train"), ok, false) {
            let spec = spec.unwrap();
            self.profile.gold -= RECRUIT_COST;
            let id = self.profile.next_id;
            self.profile.next_id += 1;
            let seed = self.rng.next_u64();
            let t = Troop::new(id, s.name.trim().to_string(), spec, seed);
            self.troops.push(t);
            let i = self.troops.len() - 1;
            self.selected = Some(i);
            self.save_all();
            return Screen::Train(crate::training::TrainState::new(i, &self.troops[i]));
        }
        let why = if !afford {
            Some("Not enough gold. Win battles to earn more.")
        } else if total != 100 {
            Some("Class mix must add up to 100%.")
        } else if !plan_ok {
            Some("Focus must add up to 100%.")
        } else {
            None
        };
        if let Some(w) = why {
            text(w, br.x, br.y - 10.0, 17.0, BAD);
        }
        s.ed.end(&self.ui);
        Screen::Recruit(s)
    }
}

/// Word-wrapped text.
pub fn wrap(s: &str, x: f32, y: f32, w: f32, size: f32, c: Color) {
    let mut line = String::new();
    let mut yy = y;
    for word in s.split_whitespace() {
        let trial = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if text_width(&trial, size) > w && !line.is_empty() {
            text(&line, x, yy, size, c);
            yy += size * 1.15;
            line = word.to_string();
        } else {
            line = trial;
        }
    }
    if !line.is_empty() {
        text(&line, x, yy, size, c);
    }
}
