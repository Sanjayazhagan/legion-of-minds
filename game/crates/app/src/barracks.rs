//! The barracks: your roster, each troop's skills and training history.

use crate::ui::*;
use crate::{Game, Screen};
use godcore::scenario::max_troop_level;
use godcore::ppo::brain_grow_cost;
use godcore::troop::{level_up_cost, MAX_LEVEL};
use macroquad::prelude::*;

impl Game {
    pub fn barracks(&mut self) -> Screen {
        if crate::stat_panel::is_open() {
            // The Barracks stays in view behind the stat panel, without input.
            let (mouse, clicked) = (self.ui.mouse, self.ui.clicked);
            (self.ui.mouse, self.ui.clicked) = (vec2(-1000.0, -1000.0), false);
            let next = self.barracks_screen();
            (self.ui.mouse, self.ui.clicked) = (mouse, clicked);
            self.stat_panel();
            return next;
        }
        self.barracks_screen()
    }

    fn barracks_screen(&mut self) -> Screen {
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
        crate::tour::mark("recruit", Rect::new(20.0, ui_y, 180.0, 44.0));
        crate::tour::mark("battle", Rect::new(210.0, ui_y, 180.0, 44.0));
        crate::tour::mark("home", Rect::new(590.0, ui_y, 220.0, 44.0));
        crate::tour::mark("more", Rect::new(400.0, ui_y, 620.0, 44.0));
        crate::tour::mark("online", Rect::new(1030.0, ui_y, 230.0, 44.0));
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
        let online = if self.account.signed_in() { "Online" } else { "Sign in" };
        if self.ui.button(Rect::new(1030.0, ui_y, 230.0, 44.0), online) {
            self.confirm_release = false;
            if self.store.is_test() {
                self.toast("Online play is off in test mode. Turn it off in Settings.");
                return Screen::Barracks;
            }
            return if self.account.signed_in() {
                Screen::Online(crate::online::OnlineState::new(self))
            } else {
                Screen::SignIn(crate::online::SignInState::from_game(&self.account))
            };
        }

        // Roster list
        let list = Rect::new(20.0, 124.0, 400.0, H - 144.0);
        panel(list);
        crate::tour::mark("roster", Rect::new(list.x, list.y, list.w, 40.0 + 3.0 * 86.0));
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
                &format!("Lv {}  {}", t.shown_level(), t.spec.name),
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
            // Busy troops say so; they can still be selected to watch them.
            if let Some(l) = self.busy_label(i) {
                let w = text_width(&l, 15.0) + 14.0;
                let tag = Rect::new(r.x + r.w - 10.0 - w, r.y + 36.0, w, 20.0);
                draw_rectangle(tag.x, tag.y, tag.w, tag.h, Color::new(0.12, 0.2, 0.32, 1.0));
                draw_rectangle_lines(tag.x, tag.y, tag.w, tag.h, 1.0, SKY);
                text(&l, tag.x + 7.0, tag.y + 15.0, 15.0, TEXT);
            }
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
        if self.ui.button(Rect::new(d.x + d.w - 184.0, d.y + 14.0, 160.0, 34.0), "Stats") || is_key_pressed(KeyCode::S) {
            crate::stat_panel::set_open(true);
        }
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
                t.shown_level(),
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
        if t.level < MAX_LEVEL {
            let next = godcore::ppo::hidden_for_level(t.level + 1).max(t.hidden());
            let note = if next > t.hidden() { format!("Grows to {next} at level {}", t.level + 1) } else { format!("Stays {next} at level {}", t.level + 1) };
            text(&note, d.x + d.w - 24.0 - text_width(&note, 16.0), y + 16.0, 16.0, MUTED);
        }
        // Past level 5 the brain grows up to this PC's max: said beside the
        // training line, where there is room.
        let pc_note = if t.level < MAX_LEVEL {
            None
        } else {
            match (t.next_brain(), self.pc_max_brain()) {
                (Some(next), Some(max)) if next <= max => Some(format!(
                    "Level {}: brain grows to {next} (PC max: level {})",
                    t.shown_level() + 1,
                    godcore::ppo::level_for_brain(max)
                )),
                (Some(_), Some(max)) => Some(format!("Top level on this PC: {}", godcore::ppo::level_for_brain(max))),
                (Some(_), None) => Some("Testing how high this PC can take troops...".to_string()),
                (None, _) => Some("Biggest brain there is".to_string()),
            }
        };
        y += 36.0;
        // Weapon and mastery
        let w = t.weapon();
        let ms = t.mastery_steps(w);
        let (ml, mf) = crate::armoury::mastery_progress(ms);
        text(&format!("{}  ·  {} mastery  ·  skill: {}", w.name(), godcore::weapons::MASTERY_TITLES[ml as usize], w.skill_name()), x, y + 8.0, 20.0, ACCENT);
        bar(Rect::new(x, y + 16.0, 300.0, 6.0), mf, ACCENT);
        let busy = self.busy_label(i);
        let t = &self.troops[i];
        let open_armoury = self.ui.button_ex(Rect::new(d.x + d.w - 184.0, y - 14.0, 160.0, 34.0), "Armoury", busy.is_none(), false);
        if open_armoury {
            self.confirm_release = false;
            return Screen::Armoury(crate::armoury::ArmouryState { idx: i });
        }
        y += 26.0;
        let kit = t.spec.moves();
        let moves: Vec<String> = kit.iter().map(|m| format!("{} {:.0}%", m.name(), t.move_practice(*m) * 100.0)).collect();
        text(&format!("Moves practised: {}", if moves.is_empty() { "none".to_string() } else { moves.join(", ") }), x, y + 8.0, 18.0, TEXT);
        // Human moves its brain has unlocked (rules v11).
        let have = godcore::human_moves::unlocked(t.hidden()).len();
        let human = match godcore::human_moves::next_unlock(t.hidden()) {
            Some((need, next)) => format!("Human moves: {have} of {} (next {} at {need} neurons)", godcore::sim::N_HUMAN, next.len()),
            None => format!("Human moves: all {}", godcore::sim::N_HUMAN),
        };
        text(&human, d.x + d.w - 24.0 - text_width(&human, 16.0), y + 8.0, 16.0, MUTED);
        y += 24.0;
        // The body training has built (human limits, rules v10).
        let b = godcore::body::Body::of(&t.spec);
        text(
            &format!(
                "Body {:.0}% conditioned: stamina {:.0}, recovers {:.1}/tick, armour weight {:.1}, reacts in {} ticks",
                t.spec.conditioning * 100.0,
                b.stamina,
                b.recovery,
                b.load,
                b.react
            ),
            x,
            y + 8.0,
            18.0,
            BREATH,
        );
        let born = t.traits().words();
        if !born.is_empty() {
            y += 22.0;
            let supply = if b.arrows > 0.0 {
                format!("  ·  carries {:.0} arrows", b.arrows)
            } else if b.mana > 0.0 {
                format!("  ·  {:.0} mana", b.mana)
            } else {
                String::new()
            };
            text(&format!("Born: {born}{supply}"), x, y + 8.0, 18.0, MUTED);
        }
        // Tiredness carried home from the last raid (stage C).
        let now = godcore::troop::unix_now();
        let tired = t.fatigue_at(now);
        if tired > 0.0 {
            y += 22.0;
            text(&rest_words(tired, t.rest_left(now)), x, y + 8.0, 18.0, BREATH);
        }
        y += 30.0;
        text(
            &format!(
                "Training {} / {} steps ({} cap)",
                fmt_steps(t.steps),
                fmt_steps(t.cap()),
                format!("level {}", t.shown_level())
            ),
            x,
            y + 8.0,
            20.0,
            TEXT,
        );
        if let Some(note) = &pc_note {
            text(note, d.x + d.w - 24.0 - text_width(note, 16.0), y + 8.0, 16.0, MUTED);
        }
        bar(
            Rect::new(x, y + 16.0, d.w - 48.0, 14.0),
            t.steps as f32 / t.cap() as f32,
            SKY,
        );
        if t.level < MAX_LEVEL || self.can_grow_on_this_pc(t) {
            let (line, frac, col) = plateau_line(t);
            text(&line, x, y + 50.0, 17.0, col);
            if let Some(f) = frac {
                bar(Rect::new(x + d.w - 48.0 - 160.0, y + 40.0, 160.0, 8.0), f, col);
            }
        }
        y += 74.0;

        // Skill badges
        text("Skills seen in drills", x, y, 20.0, TEXT);
        // A video of this troop raiding with a blank brain, then its own.
        if t.spec.role == godcore::classes::Role::Attack && t.steps > 0 {
            let rb = Rect::new(d.x + d.w - 254.0, y - 22.0, 230.0, 30.0);
            if self.ui.button(rb, "Before/after video") {
                return Screen::Clip(crate::clip::ClipState::reel(t, Screen::Barracks));
            }
        }
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
        let id = t.id;
        let level = t.level;
        let in_team = self.team_run_of(id).is_some();
        let can_train = t.steps_left() > 0 || busy.is_some();
        let train_label = if in_team {
            "Watch team"
        } else if busy.is_some() {
            "Watch training"
        } else if can_train {
            "Train"
        } else {
            "At level cap"
        };
        crate::tour::mark("train", Rect::new(x, y, 170.0, 44.0));
        if self
            .ui
            .button_ex(Rect::new(x, y, 170.0, 44.0), train_label, can_train, false)
        {
            self.confirm_release = false;
            if in_team {
                return Screen::Team(crate::teamtrain::TeamState::new(self));
            }
            return Screen::Train(crate::training::TrainState::new(&self.troops[i]));
        }
        let cost = level_up_cost(level);
        let th_cap = max_troop_level(self.profile.base.town_hall);
        let (lv_label, can_level) = self.level_up_label(i);
        if self.ui.button_ex(
            Rect::new(x + 180.0, y, 200.0, 44.0),
            &lv_label,
            can_level,
            false,
        ) {
            self.level_up_troop(i);
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
        if self.ui.button_ex(Rect::new(x + 530.0, y, 170.0, 44.0), rl, busy.is_none(), false) {
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
        if let Some(l) = &busy {
            text(
                &format!("{l}. It can't fight, level up or change gear until it's done; everyone else can."),
                x,
                y + 64.0,
                16.0,
                SKY,
            );
        } else if level >= MAX_LEVEL {
            if let Some(why) = self.level_up_blocker(i).filter(|_| self.troops[i].next_brain().is_some()) {
                crate::recruit::wrap(&why, x + 180.0, y + 64.0, 520.0, 15.0, MUTED);
            }
        } else if level < MAX_LEVEL && level >= th_cap {
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
        } else if level < MAX_LEVEL && !self.troops[i].plateau().ready() {
            text(
                "Level up unlocks once its training plateaus (see above)",
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

impl Game {
    /// Can troop `t` still grow its brain on this PC (level 5, a bigger
    /// size exists, and this PC can train it)?
    pub fn can_grow_on_this_pc(&self, t: &godcore::troop::Troop) -> bool {
        t.level >= MAX_LEVEL && t.next_brain().zip(self.pc_max_brain()).is_some_and(|(n, max)| n <= max)
    }

    /// The level-up button's label for troop `i`, and whether it can be
    /// pressed. At level 5 it grows the brain instead, up to this PC's max.
    pub fn level_up_label(&self, i: usize) -> (String, bool) {
        let level = self.troops[i].level;
        let cost = level_up_cost(level);
        let th_cap = max_troop_level(self.profile.base.town_hall);
        if self.is_busy(i) {
            ("Training...".to_string(), false)
        } else if level >= MAX_LEVEL {
            let t = &self.troops[i];
            match (t.next_brain(), self.pc_max_brain()) {
                (None, _) => ("Biggest brain".to_string(), false),
                (Some(_), None) => ("Testing your PC...".to_string(), false),
                (Some(_), Some(_)) if !self.can_grow_on_this_pc(t) => ("Top level for this PC".to_string(), false),
                (Some(_), _) if !t.plateau().ready() => ("Still learning".to_string(), false),
                (Some(n), _) => {
                    let cost = brain_grow_cost(n);
                    (format!("Level up ({cost}g)"), self.profile.gold >= cost)
                }
            }
        } else if level >= th_cap {
            ("Needs Town Hall".to_string(), false)
        } else if !self.troops[i].plateau().ready() {
            ("Still learning".to_string(), false)
        } else {
            (format!("Level up ({cost}g)"), self.profile.gold >= cost)
        }
    }

    /// Why troop `i` can't level up right now, in words, if it can't.
    pub fn level_up_blocker(&self, i: usize) -> Option<String> {
        let t = &self.troops[i];
        let cost = level_up_cost(t.level);
        let th = self.profile.base.town_hall;
        if self.is_busy(i) {
            Some(format!("{} is training. Level it up once that ends.", t.name))
        } else if t.level >= MAX_LEVEL {
            let Some(n) = t.next_brain() else { return Some(format!("{} has the biggest brain there is.", t.name)) };
            let cost = brain_grow_cost(n);
            Some(match self.settings.pc.as_ref().filter(|p| p.fresh()) {
                None => "Testing how high this PC can take troops (Settings > Your PC shows the result).".to_string(),
                Some(pc) if n > pc.max_brain() => {
                    use crate::pc_check::{duration_words, Limit, PcResult};
                    let why = match pc.limit(n) {
                        Some(Limit::Memory) => format!("needs {:.1} GB of memory to train", PcResult::train_mb(n) / 1024.0),
                        Some(Limit::BattleSpeed) => "would make battles stutter on this PC".to_string(),
                        _ => format!("would take this PC about {} of background training", duration_words(pc.hours_to_full(n))),
                    };
                    format!(
                        "{} is at the top level for this PC. Level {} (a {n}-neuron brain) {why}. A faster PC or a graphics card can go higher.",
                        t.name,
                        t.shown_level() + 1
                    )
                }
                Some(_) if !t.plateau().ready() => format!("{} is still improving with its current brain. Keep training until its score levels off, then level it up (brain grows to {n}).", t.name),
                Some(_) if self.profile.gold < cost => format!("Level {} (brain grows to {n}) costs {cost} gold; you have {}.", t.shown_level() + 1, self.profile.gold),
                Some(_) => return None,
            })
        } else if t.level >= max_troop_level(th) {
            Some(format!("Level {} troops need Town Hall {}. Upgrade it in Home.", t.level + 1, t.level))
        } else if !t.plateau().ready() {
            Some(format!("{} is still improving with its current brain. Keep training until its score levels off.", t.name))
        } else if self.profile.gold < cost {
            Some(format!("Levelling {} costs {cost} gold; you have {}.", t.name, self.profile.gold))
        } else {
            None
        }
    }

    /// Pay for and apply a level-up of troop `i`, and say what changed.
    pub fn level_up_troop(&mut self, i: usize) {
        let level = self.troops[i].level;
        if level >= MAX_LEVEL {
            let Some(n) = self.troops[i].next_brain() else { return };
            self.profile.gold -= brain_grow_cost(n);
            self.troops[i].grow_brain();
            self.save_all();
            let t = &self.troops[i];
            let hours = self.settings.pc.as_ref().map(|p| p.hours_to_full(n)).unwrap_or(0.0);
            self.toast(format!(
                "{} reached level {}: brain grew to {n} neurons, keeping what it learned. Trains up to {} steps (about {} on this PC)",
                t.name,
                t.shown_level(),
                fmt_steps(t.cap()),
                crate::pc_check::duration_words(hours)
            ));
            return;
        }
        self.profile.gold -= level_up_cost(level);
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
}

/// The plateau check in words for the Barracks: (line, progress bar while
/// it is still gathering training to judge, colour).
pub fn plateau_line(t: &godcore::troop::Troop) -> (String, Option<f32>, Color) {
    use godcore::troop::Plateau;
    let w = fmt_steps(t.plateau_window());
    let ready = "ready to level up";
    match t.plateau() {
        Plateau::Grandfathered => ("Older troop: its next level-up skips the plateau check".into(), None, GOOD),
        Plateau::AtCap => (format!("Used all training for this brain: {ready}"), None, GOOD),
        Plateau::Warming { done, need } => (
            format!("Plateau check: train {} more at this level to judge", fmt_steps(need - done)),
            Some(done as f32 / need as f32),
            MUTED,
        ),
        Plateau::Starting { .. } => ("Not learning much yet. Keep training: it often takes a while to click".into(), None, MUTED),
        Plateau::Improving { gain } => (
            format!("Still improving: +{:.0} points over the last {w} steps. Keep training", gain * 100.0),
            None,
            ACCENT,
        ),
        Plateau::Reached { .. } => (
            format!("Plateaued: its score stopped rising over the last {w} steps. Brain is full, {ready}"),
            None,
            GOOD,
        ),
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

/// "Resting: starts a raid 18% tired, fresh in 2h 15m".
pub fn rest_words(tired: f32, secs_left: u64) -> String {
    let (h, m) = (secs_left / 3600, (secs_left % 3600 + 59) / 60);
    let when = match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 60) => format!("{}h", h + 1),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    };
    format!("Resting: starts a raid {:.0}% tired, fresh in {when}", tired * 100.0)
}
