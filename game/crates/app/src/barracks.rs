//! The barracks: your roster, each troop's skills and training history.

use crate::ui::*;
use crate::{Game, Screen};
use godcore::scenario::max_troop_level;
use godcore::troop::{level_up_cost, MAX_LEVEL};
use macroquad::prelude::*;

impl Game {
    pub fn barracks(&mut self) -> Screen {
        self.header("Barracks", false);
        // Which chip draws the game, so players can check it is the fast one.
        if let Some(g) = crate::gpu_name() {
            text(&format!("Graphics: {g}"), 20.0, H - 4.0, 14.0, MUTED);
        }
        if let Some(d) = crate::train_device() {
            let t = match d {
                Some(d) => format!("Brains of 128+ neurons train on: {d}"),
                None => "Training on: CPU".to_string(),
            };
            let w = measure_text(&t, None, 14, 1.0).width;
            text(&t, W - 20.0 - w, H - 4.0, 14.0, MUTED);
        }
        let ui_y = 66.0;
        // Top actions
        if self
            .ui
            .button(Rect::new(20.0, ui_y, 180.0, 44.0), "+ Recruit")
        {
            self.confirm_release = false;
            return Screen::Recruit(crate::recruit::RecruitState::new(self.fresh_name()));
        }
        let has = !self.troops.is_empty();
        if self
            .ui
            .button_ex(Rect::new(210.0, ui_y, 180.0, 44.0), "Battle", has, false)
        {
            self.confirm_release = false;
            return Screen::BattleSetup(crate::battle_screen::SetupState::new(self));
        }
        if self.ui.button_ex(
            Rect::new(400.0, ui_y, 180.0, 44.0),
            "Tournament",
            has,
            false,
        ) {
            self.confirm_release = false;
            return Screen::Tournament(crate::tournament::TourState::new(self));
        }
        let th = self.profile.base.town_hall;
        if self.ui.button(
            Rect::new(590.0, ui_y, 220.0, 44.0),
            &format!("Home Base (TH{th})"),
        ) {
            self.confirm_release = false;
            return Screen::Home(crate::home::HomeState::new(self));
        }
        if self.ui.button_ex(Rect::new(820.0, ui_y, 200.0, 44.0), "Team training", self.troops.len() >= 2, false) {
            self.confirm_release = false;
            return Screen::Team(crate::teamtrain::TeamState::new(self));
        }

        // Roster list
        let list = Rect::new(20.0, 124.0, 400.0, H - 144.0);
        panel(list);
        text(
            &format!("Roster ({})", self.troops.len()),
            list.x + 12.0,
            list.y + 26.0,
            22.0,
            MUTED,
        );
        let card_h = 78.0;
        let visible = ((list.h - 44.0) / (card_h + 8.0)).floor() as usize;
        let max_scroll = self.troops.len().saturating_sub(visible) as f32;
        if self.ui.hover(list) {
            self.roster_scroll =
                (self.roster_scroll - mouse_wheel().1.signum()).clamp(0.0, max_scroll);
        }
        let first = self.roster_scroll as usize;
        for (k, i) in (first..self.troops.len()).take(visible).enumerate() {
            let t = &self.troops[i];
            let r = Rect::new(
                list.x + 10.0,
                list.y + 40.0 + k as f32 * (card_h + 8.0),
                list.w - 20.0,
                card_h,
            );
            let sel = self.selected == Some(i);
            draw_rectangle(
                r.x,
                r.y,
                r.w,
                r.h,
                if sel {
                    PANEL_HI
                } else if self.ui.hover(r) {
                    Color::new(0.15, 0.16, 0.22, 1.0)
                } else {
                    BG
                },
            );
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                if sel { 2.0 } else { 1.0 },
                if sel { ACCENT } else { LINE },
            );
            crate::art::draw_portrait(vec2(r.x + 34.0, r.y + 39.0), 26.0, &t.spec);
            text(&t.name, r.x + 68.0, r.y + 28.0, 24.0, TEXT);
            text(
                &format!("Lv {}  {}", t.level, t.spec.name),
                r.x + 68.0,
                r.y + 50.0,
                18.0,
                MUTED,
            );
            bar(
                Rect::new(r.x + 68.0, r.y + 60.0, r.w - 80.0, 6.0),
                t.steps as f32 / t.cap() as f32,
                SKY,
            );
            let wr = format!("{:.0}% wins", t.skills.win * 100.0);
            text(
                &wr,
                r.x + r.w - 12.0 - text_width(&wr, 18.0),
                r.y + 28.0,
                18.0,
                if t.skills.win >= 0.5 { GOOD } else { MUTED },
            );
            if self.ui.hover(r) && self.ui.clicked {
                self.selected = Some(i);
                self.confirm_release = false;
            }
        }
        if self.troops.is_empty() {
            text("No troops yet.", list.x + 20.0, list.y + 80.0, 22.0, TEXT);
            text(
                "Recruit one, train it, then",
                list.x + 20.0,
                list.y + 110.0,
                20.0,
                MUTED,
            );
            text(
                "send it into battle.",
                list.x + 20.0,
                list.y + 134.0,
                20.0,
                MUTED,
            );
        }

        // Detail panel
        let d = Rect::new(440.0, 124.0, W - 460.0, H - 144.0);
        panel(d);
        let Some(i) = self.selected.filter(|&i| i < self.troops.len()) else {
            text_centered(
                "Select or recruit a troop",
                d.x + d.w / 2.0,
                d.y + d.h / 2.0,
                26.0,
                MUTED,
            );
            return Screen::Barracks;
        };
        let t = &self.troops[i];
        let (x, mut y) = (d.x + 24.0, d.y + 44.0);
        crate::art::draw_showcase(vec2(x + 30.0, y - 6.0), 34.0, &t.spec);
        text(&t.name, x + 76.0, y, 36.0, TEXT);
        let kind = if t.spec.is_hybrid() {
            "hybrid"
        } else {
            "class"
        };
        text(
            &format!(
                "Level {} {}  ({kind}), area talents reach {}",
                t.level,
                t.spec.name,
                if t.spec.talents == 1 { "1 unit".to_string() } else { format!("{} units", t.spec.talents) }
            ),
            x + 76.0,
            y + 26.0,
            20.0,
            MUTED,
        );
        y += 60.0;
        text(&t.spec.describe(), x, y, 18.0, MUTED);
        y += 16.0;
        text(
            &format!(
                "Brain: {} neurons per layer ({} numbers), {} training sessions",
                t.actor.dims[1],
                t.brain_params(),
                t.sessions
            ),
            x,
            y + 16.0,
            18.0,
            MUTED,
        );
        if let Some(next) = t.next_brain() {
            let gb = Rect::new(d.x + d.w - 184.0, y - 12.0, 160.0, 28.0);
            if self.ui.button(gb, &format!("Grow to {next}")) {
                self.troops[i].set_brain(next);
                self.save_all();
                let t = &self.troops[i];
                self.toast(format!(
                    "{}'s brain grew to {next} neurons per layer. It keeps what it learned, trains slower, and needs more practice to use the extra room.",
                    t.name
                ));
                return Screen::Barracks;
            }
        }
        y += 36.0;
        // Weapon and mastery
        let w = t.weapon();
        let ms = t.mastery_steps(w);
        let (ml, mf) = crate::armoury::mastery_progress(ms);
        text(&format!("{}  ·  {} mastery  ·  skill: {}", w.name(), godcore::weapons::MASTERY_TITLES[ml as usize], w.skill_name()), x, y + 8.0, 20.0, ACCENT);
        bar(Rect::new(x, y + 16.0, 300.0, 6.0), mf, ACCENT);
        let open_armoury = self.ui.button(Rect::new(d.x + d.w - 184.0, y - 14.0, 160.0, 34.0), "Armoury");
        if open_armoury {
            self.confirm_release = false;
            return Screen::Armoury(crate::armoury::ArmouryState { idx: i });
        }
        y += 26.0;
        let kit = t.spec.moves();
        let moves: Vec<String> = kit.iter().map(|m| format!("{} {:.0}%", m.name(), t.move_practice(*m) * 100.0)).collect();
        text(&format!("Moves practised: {}", if moves.is_empty() { "none".to_string() } else { moves.join(", ") }), x, y + 8.0, 18.0, TEXT);
        y += 30.0;
        text(
            &format!(
                "Training {} / {} steps (level {} cap)",
                fmt_steps(t.steps),
                fmt_steps(t.cap()),
                t.level
            ),
            x,
            y + 8.0,
            20.0,
            TEXT,
        );
        bar(
            Rect::new(x, y + 16.0, d.w - 48.0, 14.0),
            t.steps as f32 / t.cap() as f32,
            SKY,
        );
        y += 52.0;

        // Skill badges
        text("Skills seen in drills", x, y, 20.0, TEXT);
        y += 10.0;
        let mut bx = x;
        if t.skills.episodes == 0 {
            text(
                "Not trained yet. A fresh brain acts at random.",
                x,
                y + 26.0,
                18.0,
                MUTED,
            );
        } else {
            for (label, ok) in t.skills.badges(&t.spec) {
                let w = text_width(label, 18.0) + 40.0;
                let r = Rect::new(bx, y + 6.0, w, 30.0);
                draw_rectangle(
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    if ok {
                        Color::new(0.15, 0.35, 0.2, 1.0)
                    } else {
                        BG
                    },
                );
                draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, if ok { GOOD } else { LINE });
                text(
                    if ok { "+" } else { "-" },
                    r.x + 10.0,
                    r.y + 21.0,
                    20.0,
                    if ok { GOOD } else { MUTED },
                );
                text(
                    label,
                    r.x + 28.0,
                    r.y + 21.0,
                    18.0,
                    if ok { TEXT } else { MUTED },
                );
                bx += w + 8.0;
            }
        }
        y += 52.0;

        // Learning curve
        let win: Vec<f32> = t.curve.iter().map(|p| p.win).collect();
        let surv: Vec<f32> = t.curve.iter().map(|p| 1.0 - p.died).collect();
        chart(
            Rect::new(x, y, d.w - 48.0, 144.0),
            &[
                (
                    &win,
                    GOOD,
                    if t.spec.role == godcore::classes::Role::Defend {
                        "base held"
                    } else {
                        "drill wins"
                    },
                ),
                (&surv, SKY, "survived"),
            ],
            &format!("{} steps", fmt_steps(t.steps)),
        );
        y += 164.0;

        // Actions
        let can_train = t.steps_left() > 0;
        let train_label = if can_train { "Train" } else { "At level cap" };
        let id = t.id;
        let level = t.level;
        if self
            .ui
            .button_ex(Rect::new(x, y, 170.0, 44.0), train_label, can_train, false)
        {
            self.confirm_release = false;
            return Screen::Train(crate::training::TrainState::new(i, &self.troops[i]));
        }
        let cost = level_up_cost(level);
        let th_cap = max_troop_level(self.profile.base.town_hall);
        let can_level = level < MAX_LEVEL && level < th_cap && self.profile.gold >= cost;
        let lv_label = if level >= MAX_LEVEL {
            "Max level".to_string()
        } else if level >= th_cap {
            "Needs Town Hall".to_string()
        } else {
            format!("Level up ({cost}g)")
        };
        if self.ui.button_ex(
            Rect::new(x + 180.0, y, 200.0, 44.0),
            &lv_label,
            can_level,
            false,
        ) {
            self.profile.gold -= cost;
            let before = self.troops[i].hidden();
            let reach = self.troops[i].spec.talents;
            self.troops[i].level_up();
            self.save_all();
            let cap = fmt_steps(self.troops[i].cap());
            let n = self.troops[i].hidden();
            let mut grew = if n > before { format!(", brain grew to {n} neurons per layer") } else { String::new() };
            let r = self.troops[i].spec.talents;
            if r > reach {
                grew += &format!(", area talents now reach {r} units");
            }
            self.toast(format!("{} reached level {}: trains up to {cap} steps{grew}", self.troops[i].name, level + 1));
        }
        if self
            .ui
            .button(Rect::new(x + 390.0, y, 130.0, 44.0), "Export")
        {
            let p = self.store.export(&self.troops[i]);
            self.toast(format!("Saved to {}", p.display()));
        }
        let rl = if self.confirm_release {
            "Really release?"
        } else {
            "Release"
        };
        if self.ui.button(Rect::new(x + 530.0, y, 170.0, 44.0), rl) {
            if self.confirm_release {
                self.store.delete_troop(id);
                let name = self.troops.remove(i).name;
                self.selected = if self.troops.is_empty() {
                    None
                } else {
                    Some(i.min(self.troops.len() - 1))
                };
                self.confirm_release = false;
                self.toast(format!("{name} left the barracks"));
            } else {
                self.confirm_release = true;
            }
        }
        if level < MAX_LEVEL && level >= th_cap {
            text(
                &format!(
                    "Upgrade your Town Hall to {} in Home Base to level up",
                    level
                ),
                x + 180.0,
                y + 64.0,
                16.0,
                MUTED,
            );
        } else if level < MAX_LEVEL && !can_level {
            text(
                &format!("Earn gold in battles to level up (need {cost})"),
                x + 180.0,
                y + 64.0,
                16.0,
                MUTED,
            );
        }
        Screen::Barracks
    }
}

pub fn fmt_steps(s: u64) -> String {
    if s >= 1_000_000 {
        format!("{:.1}M", s as f32 / 1e6)
    } else if s >= 1000 {
        format!("{}k", s / 1000)
    } else {
        s.to_string()
    }
}
