//! Cards that open over whatever screen is showing: a troop's service
//! record, the goals board, the "while you were away" report, a break
//! reminder, the before-and-after lesson and friend challenges.
//!
//! While one is open the screen underneath still draws but gets no clicks
//! (see `hide_input_for_overlay`), so screens never need to know about them.

use crate::ui::*;
use crate::{Game, Screen};
use macroquad::prelude::*;

pub enum Overlay {
    /// A troop's service record, by troop id.
    Record(u64),
    Goals,
    Away(crate::away::Report),
    Break,
    Lesson(Box<crate::lesson::Lesson>),
    Challenges(Box<crate::challenge::Panel>),
}

/// What an overlay wants after a frame.
pub struct Flow {
    /// Stay open (with this state), or close (None).
    pub keep: Option<Overlay>,
    /// Switch the screen underneath.
    pub go: Option<Screen>,
}

impl Flow {
    pub fn stay(o: Overlay) -> Flow {
        Flow { keep: Some(o), go: None }
    }
    pub fn close() -> Flow {
        Flow { keep: None, go: None }
    }
    pub fn go(s: Screen) -> Flow {
        Flow { keep: None, go: Some(s) }
    }
}

/// Mouse and clicks taken from the screen while an overlay is open.
pub struct Hidden(Vec2, bool, bool, bool);

impl Game {
    /// Before the screen runs: if an overlay is open, the screen gets no
    /// mouse or clicks this frame. Give the result to `overlay_frame`.
    pub fn hide_input_for_overlay(&mut self) -> Option<Hidden> {
        if self.overlay.is_none() || self.settings_open {
            return None;
        }
        let h = Hidden(self.ui.mouse, self.ui.clicked, self.ui.released, self.ui.right_clicked);
        self.ui.mouse = vec2(-1000.0, -1000.0);
        self.ui.clicked = false;
        self.ui.released = false;
        self.ui.right_clicked = false;
        Some(h)
    }

    /// After the screen has drawn: open anything due (the away report, a
    /// break reminder), then draw the open overlay. Returns a screen to
    /// switch to, if the overlay asked for one.
    pub fn overlay_frame(&mut self, screen: &Screen, hidden: Option<Hidden>) -> Option<Screen> {
        if let Some(Hidden(m, c, r, rc)) = hidden {
            (self.ui.mouse, self.ui.clicked, self.ui.released, self.ui.right_clicked) = (m, c, r, rc);
        }
        if self.settings_open {
            return None;
        }
        if self.overlay.is_none() {
            self.open_due_overlay(screen);
        }
        let o = self.overlay.take()?;
        // A dim veil over the screen underneath.
        draw_rectangle(0.0, 0.0, W, H, Color::new(0.02, 0.02, 0.04, 0.72));
        let esc = is_key_pressed(KeyCode::Escape);
        let was_away = matches!(o, Overlay::Away(_));
        let flow = match o {
            Overlay::Record(id) => self.record_card(id),
            Overlay::Goals => self.goals_board(),
            Overlay::Away(r) => self.away_card(r),
            Overlay::Break => self.break_card(),
            Overlay::Lesson(l) => self.lesson(l),
            Overlay::Challenges(p) => self.challenges(p),
        };
        // Escape closes any card.
        self.overlay = if esc { None } else { flow.keep };
        if esc && was_away {
            self.mark_all_seen();
        }
        flow.go
    }

    /// Open the "while you were away" report or a break reminder when one
    /// is due and the moment is calm (no battle running, no tour card).
    fn open_due_overlay(&mut self, screen: &Screen) {
        if self.tour.is_some() {
            // Practice during the tour isn't news after it.
            self.rhythm.away_due = false;
            return;
        }
        if crate::record::active() || std::env::var_os("BAG_SCREEN").is_some() {
            return;
        }
        let calm = match screen {
            Screen::Battle(b) => b.finished(),
            Screen::Clip(_) | Screen::SignIn(_) | Screen::Deploy(_) | Screen::OnlineWatch(_) => false,
            _ => true,
        };
        if !calm {
            return;
        }
        if self.break_due() {
            self.overlay = Some(Overlay::Break);
            return;
        }
        if matches!(screen, Screen::Barracks) && self.rhythm.away_due {
            self.rhythm.away_due = false;
            if let Some(r) = self.away_report() {
                self.overlay = Some(Overlay::Away(r));
            } else {
                // Nothing new: start counting from here.
                self.mark_all_seen();
            }
        }
    }
}

/// Test hook: BAG_OVERLAY opens a card at startup (for screenshots):
/// record, goals, away, break, lesson or challenges.
pub fn demo(g: &mut Game) {
    let Ok(which) = std::env::var("BAG_OVERLAY") else { return };
    let i = g.selected.unwrap_or(0).min(g.troops.len().saturating_sub(1));
    let Some(t) = g.troops.get(i).cloned() else { return };
    // Some history to show.
    let mut w = godcore::sim::World::new(10);
    let k = w.add_troop(&t.spec, 5.0, 5.0, godcore::sim::Control::External, 0);
    for (n, won) in [(3u32, true), (1, false), (2, true), (4, true)] {
        w.units[k].stats.kills = n;
        w.units[k].stats.unit_dmg = 40.0 * n as f32;
        w.units[k].stats.building_dmg = 55.0;
        w.units[k].stats.dodges = 3;
        g.journal.records.entry(t.id).or_default().add(&w.units[k], "Raid on Fort", won, godcore::troop::unix_now() - 3600 * n as u64);
    }
    g.journal.records.entry(t.id).or_default().since = godcore::troop::unix_now() - 5 * 86_400;
    g.overlay = Some(match which.as_str() {
        "record" => Overlay::Record(t.id),
        "goals" => Overlay::Goals,
        "away" => {
            for t in &g.troops {
                let mut seen = crate::journal::Seen::of(t);
                seen.steps = seen.steps.saturating_sub(60_000);
                seen.win = (seen.win - 0.18).max(0.0);
                seen.badges.truncate(seen.badges.len().saturating_sub(1));
                g.journal.seen.insert(t.id, seen);
            }
            match g.away_report() {
                Some(r) => Overlay::Away(r),
                None => return,
            }
        }
        "break" => {
            g.rhythm.stretch = 62.0 * 60.0;
            Overlay::Break
        }
        "lesson" => Overlay::Lesson(Box::new(crate::lesson::Lesson::new(&t))),
        "challenges" => Overlay::Challenges(Box::default()),
        "challenge" => {
            let c = crate::challenge::demo(g);
            Overlay::Challenges(Box::new(crate::challenge::Panel::with(c, None, g)))
        }
        _ => return,
    });
}

/// A centred card `w` x `h`. Returns its rect.
pub fn card(w: f32, h: f32) -> Rect {
    let r = Rect::new(((W - w) / 2.0).round(), ((H - h) / 2.0).max(56.0).round(), w, h);
    draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.085, 0.092, 0.13, 0.99));
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
    r
}

/// The card's close button (top right). True when pressed.
pub fn close_button(g: &Game, r: Rect) -> bool {
    g.ui.button(Rect::new(r.x + r.w - 110.0, r.y + 14.0, 96.0, 32.0), "Close")
}
