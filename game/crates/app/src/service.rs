//! A troop's service record: its life story in one card. How long it has
//! been with you, its battles and best moment, what training taught it, and
//! the title it earned from how it really fights (see `journal::Record`).

use crate::barracks::fmt_steps;
use crate::journal::{age_words, Record, TITLE_AFTER};
use crate::overlay::{card, close_button, Flow, Overlay};
use crate::ui::*;
use crate::Game;
use godcore::troop::unix_now;
use macroquad::prelude::*;

impl Game {
    /// The troop's title and one line about it, for the Barracks.
    pub fn title_line(&self, id: u64) -> (String, Color) {
        match self.journal.record(id) {
            Some(r) => match r.title() {
                Some((t, _)) => (format!("{t}  ·  {}", r.summary()), ACCENT),
                None if r.battles == 0 => ("No battles yet".to_string(), MUTED),
                None => (
                    format!("{}  ·  earns a title after {} more", r.summary(), TITLE_AFTER - r.battles),
                    MUTED,
                ),
            },
            None => (String::new(), MUTED),
        }
    }

    pub(crate) fn record_card(&mut self, id: u64) -> Flow {
        let Some(i) = self.index_of(id) else { return Flow::close() };
        let now = unix_now();
        let rec = self.journal.record(id).cloned().unwrap_or(Record { since: now, ..Default::default() });
        let r = card(900.0, 660.0);
        if close_button(self, r) {
            return Flow::close();
        }
        let t = &self.troops[i];
        let (x, w) = (r.x + 32.0, r.w - 64.0);
        crate::art::draw_showcase(vec2(x + 34.0, r.y + 54.0), 36.0, &t.spec);
        text(&t.name, x + 84.0, r.y + 50.0, 36.0, TEXT);
        text(&format!("Level {} {}  ·  with you for {}", t.level, t.spec.name, age_words(rec.since, now)), x + 84.0, r.y + 76.0, 18.0, MUTED);
        let mut y = r.y + 118.0;
        match rec.title() {
            Some((title, why)) => {
                text(title, x, y, 30.0, ACCENT);
                text(&format!("Earned in battle: {why}"), x, y + 24.0, 17.0, MUTED);
            }
            None => {
                text("No title yet", x, y, 28.0, MUTED);
                let left = TITLE_AFTER.saturating_sub(rec.battles);
                text(
                    &format!("After {left} more battle{} it earns a title from how it fights: a healer, a hunter, a wall breaker...", if left == 1 { "" } else { "s" }),
                    x,
                    y + 24.0,
                    17.0,
                    MUTED,
                );
            }
        }
        y += 50.0;

        // Battle numbers.
        let n = rec.battles.max(1) as f32;
        let cells = [
            ("Battles", rec.battles.to_string()),
            ("Won", rec.wins.to_string()),
            ("Came home", format!("{:.0}%", if rec.battles == 0 { 0.0 } else { (1.0 - rec.fell as f32 / n) * 100.0 })),
            ("Enemies down", rec.kills.to_string()),
            ("Damage", format!("{:.0}", rec.unit_dmg + rec.building_dmg)),
            ("Healed", format!("{:.0}", rec.heal)),
            ("Dodges", (rec.dodges + rec.evaded).to_string()),
            ("Blocked", format!("{:.0}", rec.blocked)),
            ("Team calls", rec.calls.to_string()),
        ];
        let cw = w / cells.len() as f32;
        for (k, (label, v)) in cells.iter().enumerate() {
            let cx = x + k as f32 * cw;
            text(v, cx, y + 26.0, 26.0, TEXT);
            text(label, cx, y + 46.0, 14.0, MUTED);
        }
        y += 70.0;

        // Training.
        let badges: Vec<&str> = t.skills.badges(&t.spec).into_iter().filter(|(_, ok)| *ok).map(|(l, _)| l).collect();
        text("Training", x, y, 20.0, TEXT);
        text(
            &format!(
                "{} steps over {} sessions  ·  drill wins {:.0}%  ·  {}",
                fmt_steps(t.steps),
                t.sessions,
                t.skills.win * 100.0,
                if badges.is_empty() { "no skills showing yet".to_string() } else { format!("skills: {}", badges.join(", ")) }
            ),
            x,
            y + 24.0,
            17.0,
            MUTED,
        );
        y += 52.0;

        // Best moment.
        text("Best moment", x, y, 20.0, TEXT);
        match &rec.best {
            Some(m) => {
                text(&format!("{} ({} ago)", m.what, age_words(m.when, now)), x, y + 24.0, 17.0, ACCENT);
                text(&m.line, x, y + 46.0, 17.0, TEXT);
            }
            None => text("Still to come.", x, y + 24.0, 17.0, MUTED),
        }
        y += 72.0;

        // Recent battles.
        text("Recent battles", x, y, 20.0, TEXT);
        y += 10.0;
        if rec.log.is_empty() {
            text("None yet. Take it on a raid, or defend your base with it.", x, y + 20.0, 17.0, MUTED);
        }
        for e in rec.log.iter().rev().take(6) {
            y += 24.0;
            let res = if e.won { "won" } else { "lost" };
            let mut bits = vec![format!("{} ({res})", e.what)];
            if e.kills > 0 {
                bits.push(format!("{} down", e.kills));
            }
            if e.dmg >= 1.0 {
                bits.push(format!("{:.0} damage", e.dmg));
            }
            if e.heal >= 1.0 {
                bits.push(format!("{:.0} healed", e.heal));
            }
            if e.fell {
                bits.push("fell".to_string());
            }
            text(&bits.join("  ·  "), x, y, 17.0, if e.won { TEXT } else { MUTED });
            let ago = format!("{} ago", age_words(e.when, now));
            text(&ago, x + w - text_width(&ago, 15.0), y, 15.0, MUTED);
        }

        // Before and after.
        let by = r.y + r.h - 66.0;
        if t.steps > 0 && self.ui.button(Rect::new(x, by, 260.0, 44.0), "Watch before and after") {
            return Flow::stay(Overlay::Lesson(Box::new(crate::lesson::Lesson::new(t))));
        }
        Flow::stay(Overlay::Record(id))
    }
}
