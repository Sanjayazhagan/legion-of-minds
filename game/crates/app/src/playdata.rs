//! Anonymous play data and player feedback.
//!
//! Off until the player says yes: a card asks once (first launch, or the
//! first launch after this update), and Settings can turn it on or off,
//! show exactly what is sent, and delete everything sent so far.
//!
//! Nothing personal goes out: no account, name, troop brain or file. Events
//! carry a random id made when the player said yes, a random id for this run
//! of the game, and small details listed in `bag_protocol::PLAY_EVENTS` (the
//! server refuses anything else). They are sent in the background about once
//! a minute to the game's own server; what can't be sent waits on disk in
//! `play-data.json` for next time.
//!
//! Also here: the one-tap rating after a player's first raid and first
//! training, and the link to the Discord server.

use crate::ui::*;
use crate::Game;
use bag_client::bag_protocol::{PlayBatch, PlayEvent, PLAY_BATCH_MAX, PLAY_EVENTS};
use bag_client::{Client, Uuid};
use macroquad::prelude::*;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// How often waiting events are uploaded, in seconds.
const SEND_EVERY: f32 = 60.0;
/// Most events kept waiting (on disk too); older ones are dropped first.
const QUEUE_MAX: usize = 1000;
/// Events kept while the player hasn't answered yet (sent only on yes).
const UNASKED_MAX: usize = 300;

/// The game's Discord invite: `BAG_DISCORD` when the game runs, else when
/// it was built. No link, no Discord button.
pub fn discord_url() -> Option<String> {
    std::env::var("BAG_DISCORD")
        .ok()
        .or_else(|| option_env!("BAG_DISCORD").map(String::from))
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("https://"))
}

struct State {
    /// None until the player answers.
    consent: Option<bool>,
    install: Option<Uuid>,
    session: Uuid,
    started: Instant,
    queue: Vec<PlayEvent>,
    file: PathBuf,
    server: String,
    since_send: f32,
    /// Frame times, for the session's frame rate.
    frames: u64,
    frame_secs: f64,
    slow: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static SENDING: AtomicBool = AtomicBool::new(false);

fn os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn props(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Start for this run of the game. `dir` holds the waiting events.
pub fn init(dir: &std::path::Path, server: &str, consent: Option<bool>, install: Option<&str>) {
    let file = dir.join("play-data.json");
    let install = install.and_then(|s| Uuid::parse_str(s).ok());
    // Events from earlier runs that didn't get sent (or a crash report).
    let queue = if consent == Some(true) {
        std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    } else {
        let _ = std::fs::remove_file(&file);
        Vec::new()
    };
    *STATE.lock().unwrap() = Some(State {
        consent,
        install,
        session: Uuid::new_v4(),
        started: Instant::now(),
        queue,
        file,
        server: server.to_string(),
        // The first upload goes out soon after the game opens.
        since_send: SEND_EVERY - 10.0,
        frames: 0,
        frame_secs: 0.0,
        slow: 0,
    });
    // A crash is written to disk straight away, to be sent next time.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        crash(info);
    }));
}

/// Something happened. Dropped if the player said no.
pub fn event(name: &'static str, details: Value) {
    debug_assert!(PLAY_EVENTS.iter().any(|(n, _)| *n == name), "{name} is not in PLAY_EVENTS");
    let Ok(mut g) = STATE.lock() else { return };
    let Some(s) = g.as_mut() else { return };
    let cap = match s.consent {
        Some(false) => return,
        Some(true) => QUEUE_MAX,
        None => UNASKED_MAX,
    };
    if s.queue.len() >= cap {
        s.queue.remove(0);
    }
    let secs = s.started.elapsed().as_secs().min(u32::MAX as u64) as u32;
    s.queue.push(PlayEvent { name: name.to_string(), secs, props: props(details) });
}

/// Is sharing on?
pub fn sharing() -> bool {
    STATE.lock().ok().and_then(|g| g.as_ref().and_then(|s| s.consent)) == Some(true)
}

/// The player answered (or switched it in Settings). Returns the play data
/// id to save: a new one the first time it is switched on.
pub fn set_consent(on: bool, install: Option<&str>) -> Option<String> {
    let mut g = STATE.lock().unwrap();
    let s = g.as_mut()?;
    s.consent = Some(on);
    if !on {
        s.queue.clear();
        let _ = std::fs::remove_file(&s.file);
        return install.map(String::from);
    }
    let id = install.and_then(|i| Uuid::parse_str(i).ok()).unwrap_or_else(Uuid::new_v4);
    s.install = Some(id);
    s.since_send = SEND_EVERY - 5.0;
    Some(id.to_string())
}

/// Delete everything sent under this id from the server, and stop sharing.
/// The answer comes back on the returned channel.
pub fn forget(install: &str) -> std::sync::mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let server = STATE.lock().unwrap().as_ref().map(|s| s.server.clone()).unwrap_or_default();
    let id = Uuid::parse_str(install).ok();
    set_consent(false, None);
    std::thread::spawn(move || {
        let r = match id {
            Some(id) => Client::new(&server).forget_play_data(id).map_err(|e| e.to_string()),
            None => Ok(()),
        };
        let _ = tx.send(r);
    });
    rx
}

/// Every frame: count the frame, and upload now and then.
pub fn tick() {
    let dt = get_frame_time();
    let Ok(mut g) = STATE.lock() else { return };
    let Some(s) = g.as_mut() else { return };
    // Skip the huge gaps of a minimised or dragged window.
    if dt > 0.0 && dt < 1.0 {
        s.frames += 1;
        s.frame_secs += dt as f64;
        if dt > 1.0 / 30.0 {
            s.slow += 1;
        }
    }
    s.since_send += dt.min(1.0);
    if s.consent != Some(true) || s.queue.is_empty() || s.since_send < SEND_EVERY || SENDING.load(Ordering::Relaxed) {
        return;
    }
    s.since_send = 0.0;
    send(s);
}

fn batch(s: &mut State) -> Option<PlayBatch> {
    let n = s.queue.len().min(PLAY_BATCH_MAX);
    (n > 0).then(|| PlayBatch {
        install: s.install.unwrap_or_else(Uuid::nil),
        session: s.session,
        game_version: env!("CARGO_PKG_VERSION").to_string(),
        sim_version: godcore::sim::SIM_VERSION,
        os: os().to_string(),
        events: s.queue.drain(..n).collect(),
    })
}

fn send(s: &mut State) {
    let Some(b) = batch(s) else { return };
    let server = s.server.clone();
    SENDING.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let r = Client::new(&server).send_play_data(&b);
        if let Err(e) = r {
            // Server down: put them back for next time. A refused batch
            // (bad request) would be refused again, so it goes.
            let refused = matches!(e, bag_client::Error::Api { status: 400, .. });
            if !refused {
                if let Ok(mut g) = STATE.lock() {
                    if let Some(s) = g.as_mut().filter(|s| s.consent == Some(true)) {
                        let mut back = b.events;
                        back.append(&mut s.queue);
                        let over = back.len().saturating_sub(QUEUE_MAX);
                        s.queue = back.split_off(over);
                    }
                }
            }
        }
        SENDING.store(false, Ordering::Relaxed);
    });
}

fn save_queue(s: &State) {
    if s.consent == Some(true) && !s.queue.is_empty() {
        let _ = std::fs::write(&s.file, serde_json::to_string(&s.queue).unwrap_or_default());
    } else {
        let _ = std::fs::remove_file(&s.file);
    }
}

/// The game is closing: note the session's length and frame rate, try a
/// last upload for a moment, and keep what's left on disk.
pub fn shutdown() {
    let (fps, slow) = {
        let g = STATE.lock().unwrap();
        let Some(s) = g.as_ref() else { return };
        let fps = if s.frame_secs > 0.0 { s.frames as f64 / s.frame_secs } else { 0.0 };
        (fps, s.slow as f64 / s.frames.max(1) as f64)
    };
    let mins = STATE.lock().unwrap().as_ref().map_or(0.0, |s| s.started.elapsed().as_secs_f64() / 60.0);
    event("session_end", json!({ "mins": round1(mins), "fps": round1(fps), "slow": (slow * 1000.0).round() / 1000.0 }));
    let mut g = STATE.lock().unwrap();
    let Some(s) = g.as_mut() else { return };
    if s.consent == Some(true) && !SENDING.load(Ordering::Relaxed) {
        if let Some(b) = batch(s) {
            let server = s.server.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let ok = Client::new(&server).send_play_data(&b).is_ok();
                let _ = tx.send((ok, b));
            });
            // Don't hold the window open for long on a slow network.
            drop(g);
            let r = rx.recv_timeout(std::time::Duration::from_secs(2));
            g = STATE.lock().unwrap();
            let Some(s) = g.as_mut() else { return };
            match r {
                Ok((true, _)) => {}
                Ok((false, b)) => s.queue.splice(0..0, b.events).for_each(drop),
                // Still going: if it fails, those events are lost.
                Err(_) => {}
            }
            save_queue(s);
            return;
        }
    }
    save_queue(s);
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A crash: keep the message and where in the game's code it happened.
fn crash(info: &std::panic::PanicHookInfo) {
    let msg = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_default();
    // Never send the player's folders (a file name in the message).
    let msg = match dirs::home_dir() {
        Some(h) => msg.replace(&*h.to_string_lossy(), "~"),
        None => msg,
    };
    let msg: String = msg.chars().take(200).collect();
    let at = info.location().map_or(String::new(), |l| {
        let parts: Vec<&str> = l.file().split(['/', '\\']).collect();
        format!("{}:{}", parts[parts.len().saturating_sub(2)..].join("/"), l.line())
    });
    // Another thread may hold the lock (or have crashed holding it).
    let Ok(mut g) = STATE.try_lock() else { return };
    let Some(s) = g.as_mut() else { return };
    if s.consent != Some(true) {
        return;
    }
    let secs = s.started.elapsed().as_secs() as u32;
    s.queue.push(PlayEvent { name: "crash".into(), secs, props: props(json!({ "message": msg, "at": at })) });
    save_queue(s);
}

/// A troop's class for the data: its base class, or a hybrid's mix.
pub fn class_of(t: &godcore::troop::Troop) -> String {
    let mut p = t.spec.parents.clone();
    p.sort_by(|a, b| b.1.total_cmp(&a.1));
    let names: Vec<String> = p.iter().map(|(c, _)| format!("{c:?}")).collect();
    if names.is_empty() {
        t.spec.name.clone()
    } else {
        names.join("+")
    }
}

/// What a squad was, for `battle_end`.
pub fn squad(troops: &[godcore::troop::Troop]) -> Value {
    let mut classes: Vec<String> = troops.iter().map(class_of).collect();
    classes.sort();
    classes.dedup();
    json!({
        "classes": classes,
        "levels": troops.iter().map(|t| t.level).collect::<Vec<_>>(),
        "brains": troops.iter().map(|t| t.hidden()).collect::<Vec<_>>(),
    })
}

/// Merge two JSON objects.
pub fn with(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

// ------------------------------------------------------------------ cards

/// What the rating card asks about.
pub fn rating_question(after: &str) -> &'static str {
    match after {
        "first_raid" => "How was your first raid?",
        _ => "How was training your first troop?",
    }
}


fn card_bg(r: Rect) {
    draw_rectangle(r.x + 4.0, r.y + 6.0, r.w, r.h, Color::new(0.0, 0.0, 0.0, 0.35));
    draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.07, 0.075, 0.11, 0.97));
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
}

const CONSENT: Rect = Rect { x: (W - 660.0) / 2.0, y: 190.0, w: 660.0, h: 350.0 };
const RATING: Rect = Rect { x: 20.0, y: H - 230.0, w: 400.0, h: 196.0 };

impl Game {
    /// The yes/no card is waiting for an answer.
    fn asking_consent(&self) -> bool {
        self.settings.share_play_data.is_none() && !self.scripted && !self.privacy_open
    }

    /// Rating card showing on this screen?
    fn rating_showing(&self, screen: &crate::Screen) -> bool {
        self.rate_now.is_some() && self.tour.is_none() && !self.settings_open && matches!(screen, crate::Screen::Barracks)
    }

    /// Before the screen runs: the consent card takes every click, and the
    /// rating card the clicks on it. Returns the real click for `cards_frame`.
    pub fn cards_input(&mut self, screen: &crate::Screen) -> bool {
        let click = self.ui.clicked;
        let on_rating = self.rating_showing(screen) && RATING.contains(self.ui.mouse);
        if self.asking_consent() || on_rating {
            self.ui.clicked = false;
            self.ui.right_clicked = false;
        }
        click
    }

    /// After everything else has drawn: the consent card, or the rating card.
    pub fn cards_frame(&mut self, screen: &crate::Screen, click: bool) {
        if self.asking_consent() {
            self.ui.clicked = click;
            self.consent_card();
        } else if self.rating_showing(screen) {
            self.ui.clicked = click && RATING.contains(self.ui.mouse);
            self.rating_card();
        }
    }

    fn consent_card(&mut self) {
        draw_rectangle(0.0, 0.0, W, H, Color::new(0.0, 0.0, 0.0, 0.55));
        let r = CONSENT;
        card_bg(r);
        let (x, mut y) = (r.x + 28.0, r.y + 46.0);
        text("Help make Legion of Minds better?", x, y, 28.0, ACCENT);
        y += 34.0;
        crate::recruit::wrap(
            "Share anonymous play data with us: where new players get stuck, how long training takes, which classes win, crashes and frame rate. It is how a small game gets better.",
            x,
            y,
            r.w - 56.0,
            18.0,
            TEXT,
        );
        y += 92.0;
        crate::recruit::wrap(
            "Never sent: your name, account, email, troops' brains or files. Each event only carries a random id made on this computer. Turn it off or delete what was sent any time in Settings.",
            x,
            y,
            r.w - 56.0,
            16.0,
            MUTED,
        );
        let by = r.y + r.h - 70.0;
        if self.ui.button_ex(Rect::new(x, by, 290.0, 46.0), "Yes, share anonymous data", true, true) {
            self.set_sharing(true);
            self.toast("Thank you! Change this any time in Settings.");
        }
        if self.ui.button(Rect::new(x + 302.0, by, 140.0, 46.0), "No thanks") {
            self.set_sharing(false);
        }
        if self.ui.button(Rect::new(r.x + r.w - 188.0, by, 160.0, 46.0), "What is sent?") {
            self.privacy_open = true;
            self.settings_open = true;
        }
    }

    /// Turn sharing on or off, and remember it.
    pub fn set_sharing(&mut self, on: bool) {
        let id = set_consent(on, self.settings.play_id.as_deref());
        self.settings.share_play_data = Some(on);
        self.settings.play_id = if on { id } else { self.settings.play_id.take() };
        self.settings.save(&self.store.home);
    }

    fn rating_card(&mut self) {
        let Some(after) = self.rate_now else { return };
        let r = RATING;
        card_bg(r);
        text(rating_question(after), r.x + 18.0, r.y + 32.0, 20.0, ACCENT);
        text("One tap, anonymous: 1 is bad, 5 is great.", r.x + 18.0, r.y + 56.0, 15.0, MUTED);
        let mut done = None;
        for k in 0..5 {
            let b = Rect::new(r.x + 18.0 + k as f32 * 74.0, r.y + 72.0, 66.0, 44.0);
            if self.ui.button(b, &(k + 1).to_string()) {
                done = Some(k as u32 + 1);
            }
        }
        let by = r.y + r.h - 54.0;
        if self.ui.button(Rect::new(r.x + 18.0, by, 110.0, 36.0), "Not now") {
            done = Some(0);
        }
        if let Some(url) = discord_url() {
            if self.ui.button(Rect::new(r.x + 140.0, by, r.w - 158.0, 36.0), "More on Discord") {
                crate::online::open_browser(&url);
            }
        }
        if let Some(stars) = done {
            if stars > 0 {
                event("rating", json!({ "after": after, "stars": stars }));
                self.toast("Thanks for the rating!");
            }
            self.rate_now = None;
        }
    }

    /// Ask for a rating once, after the first raid or first training (only
    /// players who share play data are asked).
    pub fn ask_rating(&mut self, after: &'static str) {
        if !sharing() || self.settings.asked.iter().any(|a| a == after) {
            return;
        }
        self.settings.asked.push(after.to_string());
        self.settings.save(&self.store.home);
        self.rate_now = Some(after);
    }

    /// The Settings panel: sharing on or off, what is sent, Discord.
    pub fn play_data_panel(&mut self, p: Rect) {
        panel(p);
        let (x, y) = (p.x + 24.0, p.y + 30.0);
        text("Share anonymous play data", x, y, 20.0, TEXT);
        let hint = match self.settings.share_play_data {
            Some(true) => "On: anonymous, no name or account.",
            _ => "Off: nothing about your play leaves this PC.",
        };
        text(hint, x, y + 22.0, 15.0, MUTED);
        let on = self.settings.share_play_data == Some(true);
        if self.ui.button_ex(Rect::new(p.x + p.w - 104.0, p.y + 14.0, 80.0, 36.0), if on { "On" } else { "Off" }, true, on) {
            self.set_sharing(!on);
        }
        if self.ui.button(Rect::new(p.x + p.w - 240.0, p.y + 14.0, 126.0, 36.0), "What is sent") {
            self.privacy_open = true;
        }
    }

    /// Everything that may be sent, in words, with Delete and Discord.
    pub fn privacy_screen(&mut self) {
        if self.header("Play data and feedback", true) {
            self.privacy_open = false;
            if self.settings.share_play_data.is_none() {
                // Back to the question it was opened from.
                self.settings_open = false;
            }
            return;
        }
        if let Some(rx) = &self.forgetting {
            if let Ok(r) = rx.try_recv() {
                self.forgetting = None;
                match r {
                    Ok(()) => self.toast("Deleted: the server has no play data from you now"),
                    Err(e) => self.toast(format!("Couldn't delete yet ({e}). Try again when online.")),
                }
            }
        }
        let l = Rect::new(20.0, 70.0, 820.0, H - 90.0);
        panel(l);
        let (x, mut y) = (l.x + 24.0, l.y + 38.0);
        text("What the game may send", x, y, 24.0, TEXT);
        y += 12.0;
        for (name, what) in PLAY_EVENTS {
            text(&name.replace('_', " "), x, y + 20.0, 15.0, ACCENT);
            let h = crate::recruit::wrap_h(what, x + 140.0, y + 20.0, l.w - 188.0, 15.0, TEXT);
            y += h.max(18.0) + 8.0;
        }
        let r = Rect::new(860.0, 70.0, W - 880.0, H - 90.0);
        panel(r);
        let (x, mut y) = (r.x + 24.0, r.y + 38.0);
        text("Never sent", x, y, 24.0, TEXT);
        y += 14.0;
        y += crate::recruit::wrap_h(
            "Your name, account, email, sign-in, troop names, troops' brains, saved games or any file. The server keeps no internet address with the data, and deletes it after 180 days.",
            x,
            y + 12.0,
            r.w - 48.0,
            16.0,
            MUTED,
        );
        y += 30.0;
        let on = self.settings.share_play_data == Some(true);
        text(if on { "Sharing is on" } else { "Sharing is off" }, x, y, 20.0, if on { GOOD } else { TEXT });
        y += 14.0;
        if self.ui.button_ex(Rect::new(x, y, r.w - 48.0, 40.0), if on { "Turn sharing off" } else { "Turn sharing on" }, true, false) {
            self.set_sharing(!on);
        }
        y += 54.0;
        let id = self.settings.play_id.clone();
        if let Some(id) = &id {
            text(&format!("Your play data id: {}", &id[..8]), x, y + 14.0, 15.0, MUTED);
            y += 28.0;
        }
        let can_delete = id.is_some() && self.forgetting.is_none();
        if self.ui.button_ex(Rect::new(x, y, r.w - 48.0, 40.0), "Delete my play data", can_delete, false) {
            if let Some(id) = id {
                self.forgetting = Some(forget(&id));
                self.settings.share_play_data = Some(false);
                self.settings.play_id = None;
                self.settings.save(&self.store.home);
            }
        }
        y += 50.0;
        y += crate::recruit::wrap_h(
            "Deletes everything sent from this computer and turns sharing off. Turning it on again starts a new id.",
            x,
            y + 12.0,
            r.w - 48.0,
            15.0,
            MUTED,
        );
        y += 36.0;
        text("Feedback", x, y, 24.0, TEXT);
        y += 14.0;
        match discord_url() {
            Some(url) => {
                if self.ui.button(Rect::new(x, y, r.w - 48.0, 40.0), "Talk to us on Discord") {
                    crate::online::open_browser(&url);
                }
            }
            None => {
                crate::recruit::wrap("A Discord link comes with the next release.", x, y + 14.0, r.w - 48.0, 15.0, MUTED);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_the_game_sends_is_listed() {
        let src = [
            include_str!("main.rs"),
            include_str!("playdata.rs"),
            include_str!("tour.rs"),
            include_str!("battle_screen.rs"),
            include_str!("training.rs"),
            include_str!("teamtrain.rs"),
            include_str!("online.rs"),
            include_str!("stat_panel.rs"),
        ]
        .concat();
        let mut found = 0;
        for part in src.split("event(\"").skip(1) {
            let name = part.split('"').next().unwrap();
            assert!(PLAY_EVENTS.iter().any(|(n, _)| *n == name), "{name} is sent but not in PLAY_EVENTS");
            found += 1;
        }
        assert!(found > 10);
    }

    #[test]
    fn nothing_is_kept_without_a_yes() {
        let dir = std::env::temp_dir().join(format!("lom-pd-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        init(&dir, "http://127.0.0.1:9", None, None);
        event("screen", json!({ "name": "barracks" }));
        assert_eq!(STATE.lock().unwrap().as_ref().unwrap().queue.len(), 1);
        set_consent(false, None);
        event("screen", json!({ "name": "home" }));
        assert!(STATE.lock().unwrap().as_ref().unwrap().queue.is_empty());
        let id = set_consent(true, None).unwrap();
        assert!(Uuid::parse_str(&id).is_ok());
        event("screen", json!({ "name": "home" }));
        shutdown();
        // The server isn't there: what's left waits on disk for next time.
        let saved: Vec<PlayEvent> = serde_json::from_str(&std::fs::read_to_string(dir.join("play-data.json")).unwrap()).unwrap();
        assert_eq!(saved.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["screen", "session_end"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
