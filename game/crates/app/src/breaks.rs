//! Healthy play: a gentle reminder to take a break after a long stretch of
//! play (off, or every 30, 60 or 90 minutes, from Settings), and a count of
//! time played today and this week.
//!
//! Only active play counts: a minute with no mouse or keyboard input is
//! not play, and ten minutes away counts as a break. The game never nags:
//! one card, one press to close it, and it is easy to switch off.
//!
//! Coming back after a while also queues the "while you were away" report.

use crate::overlay::{card, Flow, Overlay};
use crate::ui::*;
use crate::Game;
use macroquad::prelude::*;

/// No input for this long and the player isn't playing right now.
const IDLE_SECS: f64 = 60.0;
/// Away this long counts as a break (and shows the away report on return).
const BREAK_SECS: f64 = 600.0;
/// "Remind me later" waits this long.
const SNOOZE_SECS: f64 = 15.0 * 60.0;

/// Break reminder choices in Settings, in minutes (0 = off).
pub const CHOICES: [u32; 4] = [0, 30, 60, 90];

pub struct Rhythm {
    last_input: f64,
    last_mouse: Vec2,
    /// Play seconds not yet added to the journal.
    unsaved: f64,
    last_tick: f64,
    /// Seconds of play since the last break.
    pub stretch: f64,
    /// Show the away report the next time the Barracks is on screen.
    pub away_due: bool,
}

impl Rhythm {
    pub fn new() -> Rhythm {
        let now = get_time();
        Rhythm { last_input: now, last_mouse: Vec2::ZERO, unsaved: 0.0, last_tick: now, stretch: 0.0, away_due: true }
    }
}

impl Game {
    /// Count this frame as play if the player is here. Called every frame.
    pub(crate) fn tick_play_time(&mut self) {
        let now = get_time();
        let dt = (now - self.rhythm.last_tick).clamp(0.0, 1.0);
        self.rhythm.last_tick = now;
        let (mx, my) = mouse_position();
        let mouse = vec2(mx, my);
        let input = mouse.distance(self.rhythm.last_mouse) > 2.0
            || is_mouse_button_down(MouseButton::Left)
            || mouse_wheel().1 != 0.0
            || get_last_key_pressed().is_some();
        self.rhythm.last_mouse = mouse;
        if input {
            if now - self.rhythm.last_input >= BREAK_SECS {
                // Back from a break: start a fresh stretch, and tell them
                // what training did meanwhile.
                self.rhythm.stretch = 0.0;
                self.rhythm.away_due = true;
            }
            self.rhythm.last_input = now;
        }
        if now - self.rhythm.last_input < IDLE_SECS {
            self.rhythm.unsaved += dt;
            self.rhythm.stretch += dt;
        }
        if self.rhythm.unsaved >= 60.0 {
            let s = self.rhythm.unsaved.floor();
            self.rhythm.unsaved -= s;
            self.journal.add_play(s as u64);
            self.journal.touch();
        }
    }

    /// A break reminder is due now.
    pub fn break_due(&self) -> bool {
        let m = self.settings.break_minutes;
        m > 0 && self.rhythm.stretch >= m as f64 * 60.0
    }

    pub(crate) fn break_card(&mut self) -> Flow {
        let r = card(620.0, 330.0);
        let (x, w) = (r.x + 32.0, r.w - 64.0);
        let mins = (self.rhythm.stretch / 60.0).round() as u64;
        let hm = fmt_minutes(mins);
        text("Time for a break?", x, r.y + 52.0, 32.0, ACCENT);
        crate::recruit::wrap(
            &format!(
                "You've been playing for {hm}. It's a good moment to stand up, stretch, drink some water and rest your eyes. {}",
                if self.training_count() > 0 {
                    "Your troops keep training while the game stays open."
                } else {
                    "Your troops will be right here when you get back."
                }
            ),
            x,
            r.y + 92.0,
            w,
            20.0,
            TEXT,
        );
        let today = fmt_minutes(self.journal.played_last(1) / 60);
        text(&format!("Played today: {today}"), x, r.y + 196.0, 17.0, MUTED);
        let by = r.y + r.h - 76.0;
        if self.ui.button(Rect::new(x, by, 230.0, 48.0), "Taking a break") {
            self.rhythm.stretch = 0.0;
            self.toast("Enjoy your break!");
            return Flow::close();
        }
        if self.ui.button(Rect::new(x + 244.0, by, 190.0, 48.0), "Remind me later") {
            self.rhythm.stretch = (self.settings.break_minutes as f64 * 60.0 - SNOOZE_SECS).max(0.0);
            return Flow::close();
        }
        if self.ui.button(Rect::new(x + 448.0, by, 108.0, 48.0), "Turn off") {
            self.settings.break_minutes = 0;
            self.settings.save(&self.store.home);
            self.toast("Break reminders off. Turn them back on in Settings.");
            return Flow::close();
        }
        Flow::stay(Overlay::Break)
    }

    /// The Settings row: break reminder choice and time played.
    pub fn breaks_setting(&mut self, x: f32, y: f32, w: f32) -> bool {
        let mut changed = false;
        text("Break reminder", x, y + 22.0, 20.0, TEXT);
        let mut bx = x + w - 4.0 * 78.0;
        for m in CHOICES {
            let label = if m == 0 { "Off".to_string() } else { format!("{m} min") };
            let on = self.settings.break_minutes == m;
            if self.ui.button_ex(Rect::new(bx, y, 72.0, 32.0), &label, true, on) {
                self.settings.break_minutes = m;
                changed = true;
            }
            bx += 78.0;
        }
        let played = format!(
            "Played today {}, this week {}",
            fmt_minutes(self.journal.played_last(1) / 60),
            fmt_minutes(self.journal.played_last(7) / 60)
        );
        text(&played, x, y + 44.0, 15.0, MUTED);
        changed
    }
}

/// "45 min", "1 h 20 min".
pub fn fmt_minutes(m: u64) -> String {
    if m < 60 {
        format!("{m} min")
    } else if m % 60 == 0 {
        format!("{} h", m / 60)
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn minutes_read_naturally() {
        assert_eq!(super::fmt_minutes(5), "5 min");
        assert_eq!(super::fmt_minutes(60), "1 h");
        assert_eq!(super::fmt_minutes(95), "1 h 35 min");
    }
}
