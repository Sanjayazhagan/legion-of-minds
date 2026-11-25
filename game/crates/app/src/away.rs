//! "While you were away": when the player comes back (opening the game, or
//! returning after ten minutes or more), a short card says what training
//! taught each troop since they last looked. It only ever brings good news
//! about effort already made; nothing is lost by being away.

use crate::barracks::fmt_steps;
use crate::journal::Seen;
use crate::overlay::{card, Flow, Overlay};
use crate::ui::*;
use crate::{Game, Screen};
use macroquad::prelude::*;

/// Less practice than this since last time isn't worth a card.
const MIN_STEPS: u64 = 2_000;
/// Lines shown on the card.
const SHOWN: usize = 5;

pub struct Report {
    pub lines: Vec<Line>,
    /// Troops still training right now.
    pub still: usize,
}

pub struct Line {
    pub id: u64,
    pub steps: u64,
    pub win: (f32, f32),
    pub survival: (f32, f32),
    pub new_badges: Vec<String>,
    pub sessions: u32,
    pub mastery: Option<u8>,
}

impl Game {
    /// What training taught since the player last looked, if anything.
    pub fn away_report(&self) -> Option<Report> {
        let mut lines: Vec<Line> = self
            .troops
            .iter()
            .filter_map(|t| {
                let was = self.journal.seen.get(&t.id)?;
                let now = Seen::of(t);
                let steps = now.steps.saturating_sub(was.steps);
                (steps >= MIN_STEPS).then(|| Line {
                    id: t.id,
                    steps,
                    win: (was.win, now.win),
                    survival: (was.survival, now.survival),
                    new_badges: now.badges.iter().filter(|b| !was.badges.contains(b)).cloned().collect(),
                    sessions: now.sessions.saturating_sub(was.sessions),
                    mastery: (now.mastery > was.mastery).then_some(now.mastery),
                })
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        // Biggest news first: new skills, then the biggest jump in wins.
        lines.sort_by(|a, b| {
            (b.new_badges.len(), b.win.1 - b.win.0)
                .partial_cmp(&(a.new_badges.len(), a.win.1 - a.win.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Some(Report { lines, still: self.training_count() })
    }

    /// The player has seen what every troop knows now.
    pub fn mark_all_seen(&mut self) {
        for t in &self.troops {
            self.journal.seen.insert(t.id, Seen::of(t));
        }
        self.journal.save();
    }

    pub(crate) fn away_card(&mut self, rep: Report) -> Flow {
        let shown = rep.lines.len().min(SHOWN);
        let r = card(760.0, 170.0 + shown as f32 * 92.0);
        let (x, w) = (r.x + 30.0, r.w - 60.0);
        text("While you were away", x, r.y + 48.0, 32.0, ACCENT);
        let total: u64 = rep.lines.iter().map(|l| l.steps).sum();
        text(
            &format!("Your troops practised {} steps. Here's what it taught them.", fmt_steps(total)),
            x,
            r.y + 76.0,
            18.0,
            MUTED,
        );
        let mut go = None;
        let mut y = r.y + 96.0;
        for l in rep.lines.iter().take(SHOWN) {
            let Some(i) = self.index_of(l.id) else { continue };
            let t = &self.troops[i];
            let row = Rect::new(x, y, w, 82.0);
            panel(row);
            crate::art::draw_portrait(vec2(row.x + 34.0, row.y + 41.0), 24.0, &t.spec);
            text(&t.name, row.x + 70.0, row.y + 28.0, 22.0, TEXT);
            text(&format!("+{} steps of practice", fmt_steps(l.steps)), row.x + 70.0 + text_width(&t.name, 22.0) + 14.0, row.y + 28.0, 16.0, SKY);
            let (a, b) = l.win;
            let label = if t.spec.role == godcore::classes::Role::Defend { "Base held" } else { "Drill wins" };
            let col = if b > a + 0.005 { GOOD } else { MUTED };
            let mut line = format!("{label} {:.0}% -> {:.0}%", a * 100.0, b * 100.0);
            let (sa, sb) = l.survival;
            if (sb - sa).abs() >= 0.05 {
                line += &format!("   Survives {:.0}% -> {:.0}%", sa * 100.0, sb * 100.0);
            }
            text(&line, row.x + 70.0, row.y + 52.0, 17.0, col);
            let mut news = Vec::new();
            if !l.new_badges.is_empty() {
                news.push(format!("New: {}", l.new_badges.join(", ")));
            }
            if let Some(m) = l.mastery {
                news.push(format!("{} mastery: {}", t.weapon().name(), godcore::weapons::MASTERY_TITLES[m as usize]));
            }
            if news.is_empty() && b <= a + 0.005 {
                news.push("Still working it out. Some skills take a while to click.".to_string());
            }
            if !news.is_empty() {
                text(&news.join("  ·  "), row.x + 70.0, row.y + 72.0, 15.0, if l.new_badges.is_empty() { MUTED } else { ACCENT });
            }
            if self.ui.button(Rect::new(row.x + row.w - 150.0, row.y + 22.0, 136.0, 36.0), "Watch a drill") {
                go = Some(Screen::Train(crate::training::TrainState::new(t)));
            }
            y += 92.0;
        }
        if rep.lines.len() > SHOWN {
            text(&format!("and {} more", rep.lines.len() - SHOWN), x, y + 14.0, 16.0, MUTED);
        }
        let by = r.y + r.h - 62.0;
        if rep.still > 0 {
            let s = format!("Still training: {} troop{}", rep.still, if rep.still == 1 { "" } else { "s" });
            text(&s, x, by + 30.0, 16.0, SKY);
        }
        let done = self.ui.button(Rect::new(r.x + r.w - 190.0, by, 160.0, 44.0), "Nice!");
        if done || go.is_some() {
            self.mark_all_seen();
            return Flow { keep: None, go };
        }
        Flow::stay(Overlay::Away(rep))
    }
}
