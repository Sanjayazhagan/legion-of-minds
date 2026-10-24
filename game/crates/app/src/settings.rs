//! Settings: sound, battle view, graphics and training info, and the account
//! (sign in or out of online play).
//! Opened from the gear button in every screen's header; the screen underneath
//! waits (a battle pauses) until Settings is closed.

use crate::ui::*;
use crate::Game;
use macroquad::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Overall sound volume, 0 to 1.
    pub volume: f32,
    pub muted: bool,
    /// The camera shakes on big hits and collapses.
    pub shake: bool,
    /// Draw each trained troop's sight cone in battles.
    pub sight: bool,
    /// Playing on the test save, with cheats.
    pub test_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { volume: 0.8, muted: false, shake: true, sight: true, test_mode: false }
    }
}

static SHAKE_ON: AtomicBool = AtomicBool::new(true);
static SIGHT_ON: AtomicBool = AtomicBool::new(true);

pub fn shake_on() -> bool {
    SHAKE_ON.load(Ordering::Relaxed)
}

pub fn sight_on() -> bool {
    SIGHT_ON.load(Ordering::Relaxed)
}

impl Settings {
    pub fn load(dir: &std::path::Path) -> Settings {
        std::fs::read_to_string(dir.join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &std::path::Path) {
        let _ = std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(self).unwrap());
    }

    /// Make the game follow these settings.
    pub fn apply(&self) {
        crate::sfx::set_volume(self.volume);
        // BAG_MUTE (tests, recordings) always wins.
        if std::env::var_os("BAG_MUTE").is_none() {
            crate::sfx::set_muted(self.muted);
        }
        SHAKE_ON.store(self.shake, Ordering::Relaxed);
        SIGHT_ON.store(self.sight, Ordering::Relaxed);
    }
}

/// The header's gear button: a cog drawn beside the word Settings.
pub fn gear_button(g: &mut Game, r: Rect) -> bool {
    let hit = g.ui.button(r, "");
    let c = vec2(r.x + 20.0, r.y + r.h / 2.0);
    let col = if g.ui.hover(r) { TEXT } else { MUTED };
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let (dx, dy) = (a.cos(), a.sin());
        draw_line(c.x + dx * 5.0, c.y + dy * 5.0, c.x + dx * 10.0, c.y + dy * 10.0, 4.0, col);
    }
    draw_circle(c.x, c.y, 7.5, col);
    draw_circle(c.x, c.y, 3.5, PANEL);
    text("Settings", r.x + 38.0, r.y + r.h / 2.0 + 6.0, 18.0, TEXT);
    hit
}

impl Game {
    pub fn settings_screen(&mut self) {
        if self.header("Settings", true) {
            self.settings_open = false;
            return;
        }
        let mut changed = false;

        // ---- Sound
        let l = Rect::new(20.0, 70.0, 600.0, 250.0);
        panel(l);
        let (x, mut y) = (l.x + 24.0, l.y + 40.0);
        text("Sound", x, y, 26.0, TEXT);
        y += 44.0;
        let vol = self.settings.volume;
        text(&format!("Volume  {:.0}%", vol * 100.0), x, y, 20.0, if self.settings.muted { MUTED } else { TEXT });
        let nv = self.ui.slider(Rect::new(x + 180.0, y - 20.0, 360.0, 28.0), vol);
        if (nv - vol).abs() > 1e-4 {
            self.settings.volume = nv;
            changed = true;
        }
        if self.ui.released && self.ui.hover(Rect::new(x + 170.0, y - 30.0, 380.0, 48.0)) {
            crate::sfx::play_ui(crate::sfx::Sfx::Click);
        }
        y += 40.0;
        let label = if self.settings.muted { "Sound is off: turn on" } else { "Sound is on: mute" };
        if self.ui.button(Rect::new(x, y, 260.0, 40.0), label) {
            self.settings.muted = !self.settings.muted;
            changed = true;
        }
        y += 70.0;
        text("Press M anywhere to mute or unmute.", x, y, 16.0, MUTED);

        // ---- Battle view
        let b = Rect::new(20.0, 336.0, 600.0, 210.0);
        panel(b);
        let (x, mut y) = (b.x + 24.0, b.y + 40.0);
        text("Battle view", x, y, 26.0, TEXT);
        y += 30.0;
        for (label, hint, on) in [
            ("Screen shake", "The camera shakes on heavy hits and when buildings fall.", &mut self.settings.shake),
            ("Sight cones", "A faint wedge in front of each trained troop shows what it can see.", &mut self.settings.sight),
        ] {
            text(label, x, y + 26.0, 20.0, TEXT);
            text(hint, x, y + 48.0, 15.0, MUTED);
            let r = Rect::new(b.x + b.w - 140.0, y + 4.0, 110.0, 36.0);
            if self.ui.button_ex(r, if *on { "On" } else { "Off" }, true, *on) {
                *on = !*on;
                changed = true;
            }
            y += 66.0;
        }

        // ---- Account
        let a = Rect::new(640.0, 70.0, 620.0, 250.0);
        panel(a);
        let (x, mut y) = (a.x + 24.0, a.y + 40.0);
        text("Account", x, y, 26.0, TEXT);
        y += 40.0;
        let signed_in = self.account.user.clone().filter(|_| self.account.token.is_some());
        match signed_in {
            Some(u) => {
                text(&format!("Signed in as {}", u.display_name), x, y, 20.0, TEXT);
                y += 26.0;
                text(&format!("Trophies {}   Server: {}", u.trophies, self.account.server), x, y, 16.0, MUTED);
                y += 50.0;
                if self.ui.button(Rect::new(x, y, 200.0, 40.0), "Sign out") {
                    self.sign_out();
                }
            }
            None => {
                text("Playing offline", x, y, 20.0, TEXT);
                y += 26.0;
                crate::recruit::wrap(
                    "Your troops and base are saved on this computer. Sign in with Google or Discord to raid other players and climb the leaderboard.",
                    x,
                    y,
                    a.w - 48.0,
                    16.0,
                    MUTED,
                );
                y += 76.0;
                if self.ui.button(Rect::new(x, y, 200.0, 40.0), "Sign in") {
                    self.settings_open = false;
                    self.open_sign_in = true;
                }
            }
        }

        // ---- Graphics, training and saves
        let d = Rect::new(640.0, 336.0, 620.0, 210.0);
        panel(d);
        let (x, mut y) = (d.x + 24.0, d.y + 40.0);
        text("Graphics and training", x, y, 26.0, TEXT);
        y += 36.0;
        let gpu = crate::gpu_name().unwrap_or("checking...");
        crate::recruit::wrap(&format!("Drawing on: {gpu}"), x, y, d.w - 48.0, 16.0, TEXT);
        y += 40.0;
        let train = match crate::train_device() {
            Some(Some(dev)) => format!("Brains of 128+ neurons train on: {dev}"),
            Some(None) => "Training runs on: CPU".to_string(),
            None => "Training device: checking...".to_string(),
        };
        crate::recruit::wrap(&train, x, y, d.w - 48.0, 16.0, TEXT);
        y += 40.0;
        crate::recruit::wrap(
            "On a laptop with two graphics chips, if this names an Intel chip, set the game to High performance in Windows graphics settings.",
            x,
            y,
            d.w - 48.0,
            15.0,
            MUTED,
        );

        let s = Rect::new(20.0, 562.0, 1240.0, 80.0);
        panel(s);
        text("Saved games", s.x + 24.0, s.y + 32.0, 20.0, TEXT);
        text(&format!("{}", self.store.dir.display()), s.x + 24.0, s.y + 58.0, 16.0, MUTED);
        #[cfg(feature = "cheats")]
        {
            let test = self.store.is_test();
            text("Test mode", s.x + s.w - 480.0, s.y + 32.0, 20.0, TEXT);
            text("For testing: a separate save with cheats", s.x + s.w - 480.0, s.y + 56.0, 14.0, MUTED);
            if self.ui.button_ex(Rect::new(s.x + s.w - 120.0, s.y + 20.0, 96.0, 36.0), if test { "On" } else { "Off" }, true, test) {
                self.set_test_mode(!test);
                return;
            }
            if test {
                self.cheats_panel(Rect::new(20.0, 656.0, 1240.0, 130.0));
            }
        }

        if changed {
            self.settings.apply();
            self.settings.save(&self.store.home);
        }
    }

    /// Cheats, shown only in test mode. They change the test save only.
    #[cfg(feature = "cheats")]
    fn cheats_panel(&mut self, c: Rect) {
        panel(c);
        draw_rectangle_lines(c.x, c.y, c.w, c.h, 2.0, ACCENT);
        text("Cheats (test mode)", c.x + 24.0, c.y + 34.0, 22.0, ACCENT);
        crate::recruit::wrap(
            "Gold never runs out here. Online play is off while test mode is on, and your real save is untouched: turn test mode off to go back to it.",
            c.x + 260.0,
            c.y + 22.0,
            c.w - 290.0,
            15.0,
            MUTED,
        );
        let (mut x, y) = (c.x + 24.0, c.y + 66.0);
        let th = self.profile.base.town_hall;
        let mut button = |g: &mut Game, label: &str, enabled: bool| {
            let r = Rect::new(x, y, 232.0, 44.0);
            x += 244.0;
            g.ui.button_ex(r, label, enabled, false)
        };
        if button(self, "Town Hall to max", th < godcore::scenario::MAX_TOWN_HALL) {
            self.profile.base.town_hall = godcore::scenario::MAX_TOWN_HALL;
            self.save_all();
            self.toast(format!("Town Hall is now level {}", godcore::scenario::MAX_TOWN_HALL));
        }
        let low = self.troops.iter().any(|t| t.level < godcore::troop::MAX_LEVEL);
        if button(self, "All troops to level 5", low) {
            for t in &mut self.troops {
                while t.level < godcore::troop::MAX_LEVEL {
                    t.level_up();
                }
            }
            self.save_all();
            self.toast("Every troop is level 5 (training cap 3.5M steps)");
        }
        let missing = godcore::weapons::Weapon::ALL.iter().any(|w| !self.profile.weapons.contains(w));
        if button(self, "Unlock all weapons", missing) {
            self.profile.weapons = godcore::weapons::Weapon::ALL.to_vec();
            self.save_all();
            self.toast("All weapons unlocked");
        }
        if button(self, "Fresh copy of my save", true) {
            let _ = std::fs::remove_dir_all(self.store.home.join("test-save"));
            self.set_test_mode(true);
            self.toast("Test save copied fresh from your real save");
        }
    }

    /// Switch between the real save and the test save.
    #[cfg(feature = "cheats")]
    pub fn set_test_mode(&mut self, on: bool) {
        self.save_all();
        self.store = if on { self.store.test_store() } else { self.store.real_store() };
        self.profile = self.store.load_profile();
        self.troops = self.store.load_troops();
        self.selected = if self.troops.is_empty() { None } else { Some(0) };
        self.settings.test_mode = on;
        self.settings.save(&self.store.home);
        self.go_barracks = true;
        self.toast(if on { "Test mode on: unlimited gold, online play off" } else { "Test mode off: back to your real save" });
    }
}
