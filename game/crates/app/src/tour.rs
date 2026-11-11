//! The new player tour: a short walk through the game the first time it runs.
//!
//! New players get three ready-trained troops (see `starter_troops`), so the
//! tour can take them straight into a raid they win, then show training,
//! recruiting, the home base and the rest of the barracks. Each step is a card
//! with a short text; some point at a button and wait for the player to press
//! it, and while they do, clicks anywhere else are ignored. The tour can be
//! skipped at any time and replayed from Settings.
//!
//! Screens tell the tour where their buttons are with `mark` each frame, so
//! the tour never has to copy their layout.

use crate::ui::*;
use crate::{Game, Screen};
use godcore::classes::Role;
use godcore::troop::Troop;
use macroquad::prelude::*;
use std::cell::RefCell;

/// Starter troops shipped with the game: trained with
/// `godctl train --class <class> --steps 250000 --level 1 --seed 7`, a little
/// under the level 1 cap so the tour can show a short training session.
const STARTERS: [(&str, &str); 3] = [
    ("Brakka", include_str!("../../../assets/starters/fighter.troop.json")),
    ("Wren", include_str!("../../../assets/starters/archer.troop.json")),
    ("Sol", include_str!("../../../assets/starters/healer.troop.json")),
];

/// The ready-trained troops a new player starts with, numbered from `first_id`.
pub fn starter_troops(first_id: u64) -> Vec<Troop> {
    STARTERS
        .iter()
        .enumerate()
        .map(|(k, (name, json))| {
            let mut t = Troop::load_json(json).expect("starter troop file loads");
            t.id = first_id + k as u64;
            t.name = name.to_string();
            t
        })
        .collect()
}

thread_local! {
    /// Buttons the screens drew this frame, by name.
    static MARKS: RefCell<Vec<(&'static str, Rect)>> = const { RefCell::new(Vec::new()) };
    /// What the tour is pointing at and its card, from the last frame.
    static LAST: RefCell<(Option<Rect>, Option<Rect>, bool)> = const { RefCell::new((None, None, false)) };
}

/// A screen says where one of its buttons is this frame.
pub fn mark(name: &'static str, r: Rect) {
    MARKS.with(|m| m.borrow_mut().push((name, r)));
}

fn marked(name: &str) -> Option<Rect> {
    MARKS.with(|m| m.borrow().iter().rev().find(|(n, _)| *n == name).map(|(_, r)| *r))
}

/// Which screen a step belongs on.
#[derive(Clone, Copy, PartialEq)]
enum On {
    Barracks,
    Setup,
    Deploy,
    Battle,
    Train,
}

fn on(s: &Screen) -> Option<On> {
    Some(match s {
        Screen::Barracks => On::Barracks,
        Screen::BattleSetup(_) => On::Setup,
        Screen::Deploy(_) => On::Deploy,
        Screen::Battle(_) => On::Battle,
        Screen::Train(_) => On::Train,
        _ => return None,
    })
}

/// What moves the tour on from a step.
#[derive(Clone, Copy, PartialEq)]
enum Wait {
    /// The card's Next button.
    Next,
    /// The player reaching this screen (by pressing the button pointed at).
    Reach(On),
    /// The battle on screen finishing.
    BattleOver,
}

/// Where the card sits.
#[derive(Clone, Copy)]
enum Place {
    /// Next to what it points at (below, or above if there is no room).
    Near,
    Center,
    At(f32, f32),
}

struct Step {
    on: On,
    title: &'static str,
    body: String,
    /// The `mark` name of what the step points at.
    target: Option<&'static str>,
    wait: Wait,
    place: Place,
    /// Clicks outside the target and the card are ignored.
    lock: bool,
}

// Step numbers, so the raid can be restarted or skipped.
const RAID: usize = 2;
const AFTER_RAID: usize = 7;

fn step(k: usize, g: &Game, screen: &Screen) -> Option<Step> {
    // The first time, the player has the starter troops; a replay may not.
    let first = !g.profile.tour_done;
    let s = |on, title, body: &str, target, wait, place, lock| Step { on, title, body: body.to_string(), target, wait, place, lock };
    use On::*;
    use Place::*;
    use Wait::*;
    Some(match k {
        0 if first => s(Barracks, "Welcome to Legion of Minds", "In this game you don't steer your troops in battle. You train their brains, and then they fight on their own. We gave you three troops that have already had some training, so let's win your first raid.", None, Next, Center, true),
        0 => s(Barracks, "Welcome to Legion of Minds", "In this game you don't steer your troops in battle. You train their brains, and then they fight on their own. Let's walk through a raid and the main screens.", None, Next, Center, true),
        1 if first => s(Barracks, "Your troops", "These are your troops: a Fighter, an Archer and a Healer. Each one has its own small brain. The blue bar shows how much it has practised, and the number on the right is how often it wins its practice drills.", Some("roster"), Next, Near, true),
        1 => s(Barracks, "Your troops", "These are your troops. Each one has its own small brain. The blue bar shows how much it has practised, and the number on the right is how often it wins its practice drills.", Some("roster"), Next, Near, true),
        2 => s(Barracks, "Your first raid", "Press Battle to choose a base to attack.", Some("battle"), Reach(Setup), Near, true),
        3 if first => s(Setup, "Pick a base and a squad", "Outpost, the easiest base, is already picked, and all three of your troops are in the squad (the green cards). Press Deploy troops.", Some("deploy"), Reach(Deploy), At(20.0, 560.0), true),
        3 => s(Setup, "Pick a base and a squad", "Click a base at the top, and click troops to add them to your squad (up to 5). Then press Deploy troops.", Some("deploy"), Reach(Deploy), At(20.0, 560.0), false),
        4 => s(Deploy, "Place your troops", "Click the grass outside the red zone to drop each troop, or press Auto-place rest to put them all down. Then press FIGHT!", Some("deploy_next"), Reach(Battle), Near, false),
        5 => s(Battle, "Now just watch", "Nobody is steering. Every moment, each troop decides what to do using what it learned in practice. Press 2x or 4x to speed it up.", Some("speed"), BattleOver, At(30.0, 640.0), false),
        6 => {
            let stars = match screen {
                Screen::Battle(b) => b.stars(),
                _ => 0,
            };
            let body = if stars > 0 {
                format!("You won {stars} star{}! Stars pay gold, and gold lets you recruit new troops, level them up and build your base. Press Barracks to go on.", if stars == 1 { "" } else { "s" })
            } else {
                "Not this time. That happens: troops get better with more training. Press Barracks to go on.".to_string()
            };
            Step { on: Battle, title: if stars > 0 { "Victory!" } else { "So close" }, body, target: Some("to_barracks"), wait: Reach(Barracks), place: At(294.0, 510.0), lock: true }
        }
        7 => {
            let at_cap = g.selected.and_then(|i| g.troops.get(i)).is_none_or(|t| t.steps_left() == 0);
            if at_cap {
                s(Barracks, "Training", "Training is how a troop gets smarter. Pick a troop, press Train and choose how many practice steps to give it. More steps make a better brain. Levelling up with gold lets a troop train longer.", Some("train"), Next, Near, true)
            } else {
                s(Barracks, "Training", "Training is how a troop gets smarter. Let's give this troop a little more practice: press Train.", Some("train"), Reach(Train), Near, true)
            }
        }
        8 => s(Train, "Practice drills", "On the left the troop plays practice drills with its newest brain, so you can watch it learn. Press Start training. It runs on your own computer, takes a few seconds here, and you can stop at any time and keep the progress. Press < Back when it is done.", Some("start_training"), Reach(Barracks), At(30.0, 560.0), false),
        9 => s(Barracks, "Recruit more troops", "Recruiting costs 50 gold. Pick a class, or mix classes by percentage to make a hybrid, like Mage 50 + Healer 50. Each new troop starts with an untrained brain.", Some("recruit"), Next, Near, true),
        10 => s(Barracks, "Your home base", "Upgrade your Town Hall, place towers and cannons, and train defender troops to protect it against raids.", Some("home"), Next, Near, true),
        11 => s(Barracks, "Tournaments and team training", "Tournament runs your troops through the same raids and ranks them. Team training teaches 2 to 5 troops to fight together and call out to each other.", Some("more"), Next, Near, true),
        12 => s(Barracks, "Play online", "Sign in to raid other players' bases and defend your own.", Some("online"), Next, Near, true),
        13 => s(Barracks, "You're ready", "That's the tour. Train your troops, win stars and build your base. You can play this tour again any time from Settings.", Some("settings"), Next, Near, true),
        _ => return None,
    })
}

const CARD_W: f32 = 440.0;

/// Lines of `s` wrapped to `w` at text size `size`.
fn lines(s: &str, w: f32, size: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if text_width(&trial, size) > w && !line.is_empty() {
            out.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = trial;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn has_attackers(g: &Game) -> bool {
    g.troops.iter().any(|t| t.spec.role == Role::Attack)
}

impl Game {
    /// Start (or restart) the tour from the beginning.
    pub fn start_tour(&mut self) {
        self.tour = Some(0);
    }

    fn end_tour(&mut self) {
        self.tour = None;
        if !self.profile.tour_done {
            self.profile.tour_done = true;
            self.store.save_profile(&self.profile);
        }
    }

    /// Before the screen runs: while a step waits for one button, clicks
    /// anywhere else are ignored, and clicks on the tour's card never reach
    /// the screen. Returns the real click, for `tour_frame`.
    pub fn tour_input(&mut self, screen: &Screen) -> bool {
        let click = self.ui.clicked;
        MARKS.with(|m| m.borrow_mut().clear());
        if self.tour.is_none() || self.settings_open {
            return click;
        }
        let (target, card, lock) = LAST.with(|l| *l.borrow());
        let in_card = card.is_some_and(|c| c.contains(self.ui.mouse));
        let in_target = target.is_some_and(|t| t.contains(self.ui.mouse));
        let paused = self.tour.and_then(|k| step(k, self, screen)).is_none_or(|s| on(screen) != Some(s.on));
        if in_card || (lock && !paused && !in_target) {
            self.ui.clicked = false;
            self.ui.right_clicked = false;
        }
        click
    }

    /// After the screen has drawn: move the tour along and draw its card.
    pub fn tour_frame(&mut self, screen: &Screen, click: bool) {
        LAST.with(|l| *l.borrow_mut() = (None, None, false));
        let Some(mut k) = self.tour else { return };
        if self.settings_open {
            return;
        }
        // Keep the raid steps in order: backing out of a raid starts it again
        // from the Battle button, and with no attackers the raid is skipped.
        if (RAID..AFTER_RAID - 1).contains(&k) && matches!(screen, Screen::Barracks) {
            k = RAID;
        }
        if k == RAID && !has_attackers(self) {
            k = AFTER_RAID;
        }
        let Some(st) = step(k, self, screen) else {
            self.end_tour();
            return;
        };
        let here = on(screen) == Some(st.on);
        // Has the player done what the step asked?
        let done = match st.wait {
            Wait::Reach(w) => on(screen) == Some(w) && w != st.on,
            Wait::BattleOver => matches!(screen, Screen::Battle(b) if b.finished()),
            Wait::Next => false,
        };
        if done {
            self.tour = Some(k + 1);
            return;
        }
        self.tour = Some(k);
        self.ui.clicked = click;

        if !here {
            // Before the first card (the sign-in screen) there is nothing to say.
            if k > 0 {
                self.tour_paused(st.on, click);
            }
            return;
        }
        let target = st.target.and_then(marked);
        // Dim everything but the target, and ring it.
        let dim = Color::new(0.0, 0.0, 0.0, 0.5);
        match target {
            Some(t) if st.lock => {
                let t = Rect::new(t.x - 6.0, t.y - 6.0, t.w + 12.0, t.h + 12.0);
                draw_rectangle(0.0, 0.0, W, t.y, dim);
                draw_rectangle(0.0, t.y + t.h, W, H - t.y - t.h, dim);
                draw_rectangle(0.0, t.y, t.x, t.h, dim);
                draw_rectangle(t.x + t.w, t.y, W - t.x - t.w, t.h, dim);
            }
            None if st.lock => draw_rectangle(0.0, 0.0, W, H, dim),
            _ => {}
        }
        if let Some(t) = target {
            let pulse = 0.5 + 0.5 * (crate::record::now() as f32 * 5.0).sin();
            let pad = 6.0 + 3.0 * pulse;
            draw_rectangle_lines(t.x - pad, t.y - pad, t.w + 2.0 * pad, t.h + 2.0 * pad, 4.0, ACCENT);
        }

        // The card.
        let body = lines(&st.body, CARD_W - 40.0, 19.0);
        let h = 64.0 + body.len() as f32 * 22.0 + 64.0;
        let (x, y) = match (st.place, target) {
            (Place::At(x, y), _) => (x, y.min(H - h - 10.0)),
            (Place::Near, Some(t)) => {
                let x = t.x.clamp(20.0, W - CARD_W - 20.0);
                let below = t.y + t.h + 18.0;
                if below + h < H - 10.0 {
                    (x, below)
                } else if t.y - 18.0 - h > 56.0 {
                    (x, t.y - 18.0 - h)
                } else if t.x + t.w + 18.0 + CARD_W < W {
                    (t.x + t.w + 18.0, (t.y).min(H - h - 10.0))
                } else {
                    (t.x - 18.0 - CARD_W, (t.y).min(H - h - 10.0))
                }
            }
            _ => ((W - CARD_W) / 2.0, (H - h) / 2.0),
        };
        let card = Rect::new(x, y, CARD_W, h);
        draw_rectangle(card.x + 4.0, card.y + 6.0, card.w, card.h, Color::new(0.0, 0.0, 0.0, 0.35));
        draw_rectangle(card.x, card.y, card.w, card.h, Color::new(0.07, 0.075, 0.11, 0.97));
        draw_rectangle_lines(card.x, card.y, card.w, card.h, 2.0, ACCENT);
        let total = 14;
        text(&format!("{} / {total}", k + 1), card.x + card.w - 64.0, card.y + 30.0, 16.0, MUTED);
        text(st.title, card.x + 20.0, card.y + 36.0, 26.0, ACCENT);
        let mut ly = card.y + 68.0;
        for l in &body {
            text(l, card.x + 20.0, ly, 19.0, TEXT);
            ly += 22.0;
        }
        let by = card.y + card.h - 52.0;
        let in_card = card.contains(self.ui.mouse);
        let pressed = |g: &Game, r: Rect, label: &str| g.ui.button(r, label) && in_card;
        let last = step(k + 1, self, screen).is_none();
        if !last && pressed(self, Rect::new(card.x + 20.0, by, 120.0, 36.0), "Skip tour") {
            self.end_tour();
            self.toast("Tour skipped. Replay it any time from Settings.");
            return;
        }
        if st.wait == Wait::Next {
            let label = match k {
                0 => "Let's go",
                _ if last => "Finish",
                _ => "Next",
            };
            if pressed(self, Rect::new(card.x + card.w - 160.0, by, 140.0, 36.0), label) {
                self.tour = Some(k + 1);
                if last {
                    self.end_tour();
                    self.toast("Have fun! Settings > Replay the tour shows this again.");
                }
            }
        } else {
            let hint = if target.is_some() && st.lock { "Press the highlighted button" } else { "" };
            text(hint, card.x + card.w - 20.0 - text_width(hint, 16.0), by + 24.0, 16.0, MUTED);
        }
        LAST.with(|l| *l.borrow_mut() = (target, Some(card), st.lock));
    }

    /// The player left the screen the tour is on: a small bar says where to go.
    fn tour_paused(&mut self, want: On, _click: bool) {
        let where_ = match want {
            On::Barracks => "Go back to the Barracks to carry on with the tour.",
            On::Train => "Open Train in the Barracks to carry on with the tour.",
            _ => "Go back to the Barracks to carry on with the tour.",
        };
        let card = Rect::new(20.0, H - 64.0, 600.0, 48.0);
        draw_rectangle(card.x, card.y, card.w, card.h, Color::new(0.07, 0.075, 0.11, 0.95));
        draw_rectangle_lines(card.x, card.y, card.w, card.h, 2.0, ACCENT);
        text(where_, card.x + 16.0, card.y + 30.0, 18.0, TEXT);
        let r = Rect::new(card.x + card.w - 128.0, card.y + 8.0, 116.0, 32.0);
        if self.ui.button(r, "Skip tour") && card.contains(self.ui.mouse) {
            self.end_tour();
            self.toast("Tour skipped. Replay it any time from Settings.");
            return;
        }
        LAST.with(|l| *l.borrow_mut() = (None, Some(card), false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use godcore::battle::Battle;
    use godcore::scenario::BASES;
    use godcore::sim::Outcome;

    /// The quick start promise: the starter squad, auto-placed, wins its
    /// first raid on the Outpost in under a minute at normal speed.
    #[test]
    fn starter_squad_wins_the_first_raid_quickly() {
        let troops = starter_troops(1);
        assert_eq!(troops.len(), 3);
        assert!(troops.iter().all(|t| t.steps_left() > 0), "starters leave room for the tour's training step");
        let spots = crate::battle_screen::auto_place(&[], troops.len());
        let deploy: Vec<_> = troops.into_iter().zip(spots).collect();
        let mut b = Battle::new(&BASES[0], deploy);
        b.run();
        let secs = b.world.tick as f32 / crate::battle_screen::WATCH_TPS;
        println!("{:?}, {} stars, {:.0}s at 1x", b.outcome(), b.world.stars(), secs);
        assert!(b.world.stars() >= 2, "starters must win the first raid");
        assert!(b.outcome() == Outcome::Victory || b.world.stars() >= 2);
        assert!(secs < 60.0, "first raid took {secs:.0}s");
    }
}
