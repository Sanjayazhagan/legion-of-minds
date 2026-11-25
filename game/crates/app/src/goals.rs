//! The goals board: always a clear next step. One goal from each of five
//! tracks is on the board at a time, from quick ("Win your first raid") to
//! long ("Reach level 5 with a troop"). Progress counts everything already
//! done, so most goals start partly filled, and nothing ever expires: a goal
//! waits on the board until it is done and its gold collected.

use crate::overlay::{card, Flow, Overlay};
use crate::ui::*;
use crate::Game;
use godcore::classes::Role;
use godcore::scenario::{BASES, WAVES};
use macroquad::prelude::*;

pub struct Goal {
    pub id: &'static str,
    pub track: &'static str,
    pub text: String,
    pub done: u32,
    pub need: u32,
    pub gold: u32,
}

impl Goal {
    pub fn complete(&self) -> bool {
        self.done >= self.need
    }
}

/// Every goal in the game, in order within each track.
pub fn all(g: &Game) -> Vec<Goal> {
    let p = &g.profile;
    let ts = &g.troops;
    let stars: u32 = p.best_stars.iter().sum();
    let starred = p.best_stars.iter().filter(|&&s| s > 0).count() as u32;
    let nb = BASES.len() as u32;
    let sessions: u32 = ts.iter().map(|t| t.sessions).sum();
    let max_steps = ts.iter().map(|t| t.steps).max().unwrap_or(0);
    let max_level = ts.iter().map(|t| t.level).max().unwrap_or(0);
    let max_badges = ts.iter().map(|t| t.skills.badges(&t.spec).iter().filter(|(_, ok)| *ok).count() as u32).max().unwrap_or(0);
    let big_brain = ts.iter().any(|t| t.hidden() > 256);
    let held = p.waves_held.iter().filter(|&&h| h).count() as u32;
    let th = p.base.town_hall;
    let rec_wins = g.journal.records.values().map(|r| r.wins).max().unwrap_or(0);
    let rec_battles = g.journal.records.values().map(|r| r.battles).max().unwrap_or(0);
    let titled = g.journal.records.values().any(|r| r.title().is_some());
    let mk = |id, track, text: &str, done: u32, need: u32, gold| Goal { id, track, text: text.to_string(), done: done.min(need), need, gold };
    let b = |x: bool| x as u32;
    vec![
        // Raids
        mk("raid_first", "Raids", "Win your first raid", b(stars > 0), 1, 50),
        mk("raid_outpost3", "Raids", &format!("Get 3 stars on {}", BASES[0].name), p.best_stars[0], 3, 100),
        mk("raid_every", "Raids", "Win stars on every offline base", starred, nb, 200),
        mk("raid_9", "Raids", "Earn 9 stars across the offline bases", stars, 9, 300),
        mk("raid_all3", "Raids", "3 stars on every offline base", stars, 3 * nb, 600),
        // Training
        mk("train_session", "Training", "Finish a training session", sessions.min(1), 1, 50),
        mk("train_skills", "Training", "Have a troop show 3 skills in its drills", max_badges, 3, 100),
        mk("train_500k", "Training", "Train one troop to 500k steps", (max_steps / 1000) as u32, 500, 150),
        mk("train_l3", "Training", "Reach level 3 with a troop", max_level, 3, 200),
        mk("train_l5", "Training", "Reach level 5 with a troop", max_level, 5, 400),
        mk("train_grow", "Training", "Grow a brain past 256 neurons", b(big_brain), 1, 500),
        // Army
        mk("army_4", "Army", "Recruit a fourth troop", ts.len() as u32, 4, 50),
        mk("army_hybrid", "Army", "Recruit a hybrid (mix two classes)", b(ts.iter().any(|t| t.spec.is_hybrid())), 1, 100),
        mk("army_defender", "Army", "Recruit a defender for your base", b(ts.iter().any(|t| t.spec.role == Role::Defend)), 1, 100),
        mk("army_team", "Army", "Train two troops together in Team training", b(ts.iter().any(|t| t.team_steps > 0)), 1, 150),
        mk("army_8", "Army", "Build an army of 8 troops", ts.len() as u32, 8, 200),
        // Home
        mk("home_th2", "Home", "Upgrade your Town Hall to level 2", th, 2, 100),
        mk("home_wave", "Home", "Hold off a raid wave at your base", held.min(1), 1, 100),
        mk("home_th3", "Home", "Upgrade your Town Hall to level 3", th, 3, 200),
        mk("home_waves", "Home", "Hold off every raid wave", held, WAVES.len() as u32, 300),
        mk("home_th5", "Home", "Upgrade your Town Hall to level 5", th, 5, 500),
        // Story
        mk("story_3wins", "Story", "Win 3 battles with the same troop", rec_wins, 3, 100),
        mk("story_title", "Story", "Let a troop earn a title (fight 3 battles with it)", b(titled), 1, 100),
        mk("story_challenge", "Story", "Send a challenge to a friend", g.journal.challenges_sent.min(1), 1, 100),
        mk("story_beat", "Story", "Beat a friend's challenge", g.journal.challenges_won.min(1), 1, 150),
        mk("story_veteran", "Story", "Fight 20 battles with the same troop", rec_battles, 20, 300),
    ]
}

/// The goals on the board now: the first goal in each track whose gold
/// hasn't been collected.
pub fn board(g: &Game) -> Vec<Goal> {
    let mut out: Vec<Goal> = Vec::new();
    for goal in all(g) {
        if g.journal.claimed.iter().any(|c| c == goal.id) || out.iter().any(|o| o.track == goal.track) {
            continue;
        }
        out.push(goal);
    }
    out
}

impl Game {
    /// Goals done whose gold is waiting.
    pub fn goals_ready(&self) -> usize {
        board(self).iter().filter(|g| g.complete()).count()
    }

    fn claim(&mut self, goal: &Goal) {
        if !goal.complete() || self.journal.claimed.iter().any(|c| c == goal.id) {
            return;
        }
        self.journal.claimed.push(goal.id.to_string());
        self.profile.gold += goal.gold;
        self.store.save_profile(&self.profile);
        self.journal.save();
        crate::sfx::play_ui(crate::sfx::Sfx::Victory);
        self.toast(format!("Goal done: {}. +{} gold", goal.text, goal.gold));
    }

    /// The strip at the bottom of the roster: the nearest goal, and the
    /// button that opens the board.
    pub fn goal_strip(&mut self, r: Rect) {
        panel(r);
        let goals = board(self);
        let ready = goals.iter().filter(|g| g.complete()).count();
        // The goal closest to done, finished ones first.
        let next = goals
            .iter()
            .max_by(|a, b| (a.done as f32 / a.need as f32).partial_cmp(&(b.done as f32 / b.need as f32)).unwrap());
        let bw = 104.0;
        let label = if ready > 0 { format!("Goals ({ready})") } else { "Goals".to_string() };
        let br = Rect::new(r.x + r.w - bw - 8.0, r.y + 10.0, bw, r.h - 20.0);
        crate::tour::mark("goals", br);
        if self.ui.button_ex(br, &label, true, ready > 0) {
            self.overlay = Some(Overlay::Goals);
        }
        let x = r.x + 12.0;
        let tw = r.w - bw - 32.0;
        match next {
            Some(g) => {
                text(if g.complete() { "Goal done! Collect it" } else { "Next goal" }, x, r.y + 20.0, 15.0, if g.complete() { GOOD } else { MUTED });
                let t = fit(&g.text, tw, 17.0);
                text(&t, x, r.y + 40.0, 17.0, TEXT);
                bar(Rect::new(x, r.y + 48.0, tw, 6.0), g.done as f32 / g.need as f32, if g.complete() { GOOD } else { ACCENT });
            }
            None => {
                text("Every goal done. You're a legend.", x, r.y + 36.0, 17.0, GOOD);
            }
        }
    }

    pub(crate) fn goals_board(&mut self) -> Flow {
        let goals = board(self);
        let r = card(760.0, 150.0 + goals.len().max(1) as f32 * 84.0);
        let (x, w) = (r.x + 30.0, r.w - 60.0);
        text("Goals", x, r.y + 46.0, 32.0, ACCENT);
        text("Goals never expire. Finish them in any order, at your own pace.", x, r.y + 74.0, 17.0, MUTED);
        if crate::overlay::close_button(self, r) {
            return Flow::close();
        }
        let mut y = r.y + 94.0;
        let mut claim = None;
        for (k, g) in goals.iter().enumerate() {
            let row = Rect::new(x, y, w, 74.0);
            panel(row);
            if g.complete() {
                draw_rectangle_lines(row.x, row.y, row.w, row.h, 2.0, GOOD);
            }
            text(g.track, row.x + 14.0, row.y + 22.0, 15.0, SKY);
            text(&g.text, row.x + 14.0, row.y + 46.0, 20.0, TEXT);
            let frac = g.done as f32 / g.need as f32;
            bar(Rect::new(row.x + 14.0, row.y + 56.0, 330.0, 8.0), frac, if g.complete() { GOOD } else { ACCENT });
            let count = if g.need > 1 { format!("{} / {}", g.done, g.need) } else if g.complete() { "Done".to_string() } else { "Not yet".to_string() };
            text(&count, row.x + 356.0, row.y + 65.0, 15.0, MUTED);
            let br = Rect::new(row.x + row.w - 190.0, row.y + 16.0, 176.0, 42.0);
            if g.complete() {
                if self.ui.button_ex(br, &format!("Collect {} gold", g.gold), true, true) {
                    claim = Some(k);
                }
            } else {
                let reward = format!("Reward: {} gold", g.gold);
                text(&reward, br.x + br.w - text_width(&reward, 16.0), br.y + 27.0, 16.0, ACCENT);
            }
            y += 84.0;
        }
        if goals.is_empty() {
            text("Every goal is done. More are coming in future updates.", x, y + 30.0, 20.0, GOOD);
        }
        let done = self.journal.claimed.len();
        let total = all(self).len();
        text(&format!("{done} of {total} goals done"), x, r.y + r.h - 22.0, 16.0, MUTED);
        if let Some(k) = claim {
            self.claim(&goals[k]);
        }
        Flow::stay(Overlay::Goals)
    }
}

/// `s` cut short with "..." to fit `w` at text size `size`.
pub fn fit(s: &str, w: f32, size: f32) -> String {
    if text_width(s, size) <= w {
        return s.to_string();
    }
    let mut t: String = s.to_string();
    while !t.is_empty() && text_width(&format!("{t}..."), size) > w {
        t.pop();
    }
    format!("{}...", t.trim_end())
}

#[cfg(test)]
mod tests {
    #[test]
    fn goal_ids_are_unique() {
        // The board can't be built without a Game, so check the id list in
        // the source: each `mk("id"` appears once.
        let src = include_str!("goals.rs");
        let ids: Vec<&str> = src.match_indices("mk(\"").map(|(i, _)| src[i + 4..].split('"').next().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        assert!(ids.len() >= 20);
    }
}
