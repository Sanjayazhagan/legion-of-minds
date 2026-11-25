//! Challenge a friend: send your troops as a file, your friend fights them
//! on their own computer, and their result comes back as a file you can
//! open to watch the exact battle.
//!
//! Two kinds:
//! - "Raid my base": your home base and up to 4 defenders. Your friend
//!   attacks it with their squad. They beat it by taking a star.
//! - "Stop my squad": your best 5 attackers raid your friend's home base,
//!   held by their defenders. They beat it by giving away no stars.
//!
//! A challenge is a plain `.challenge.json` file: send it in any chat app.
//! Your friend drags it onto the game window (or leaves it in Downloads).
//! No account or server is needed. Troops are copied as they are now, and
//! everyone starts fresh (no carried tiredness), so both computers build the
//! very same battle: the reply file replays it exactly.

use crate::battle_screen::BattleState;
use crate::overlay::{card, close_button, Flow, Overlay};
use crate::ui::*;
use crate::{Game, Screen};
use godcore::battle::Battle;
use godcore::classes::Role;
use godcore::scenario::{home_spots, HomeBase, RAID_ENTRY};
use godcore::troop::{unix_now, Troop};
use macroquad::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const TAG: &str = "legion-of-minds-challenge";
const EXT: &str = ".challenge.json";
const MAX_SQUAD: usize = 5;
const MAX_DEFENDERS: usize = crate::home::MAX_DEFENDERS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// The sender's base and defenders; the friend attacks.
    RaidMyBase,
    /// The sender's attackers; the friend defends their own base.
    StopMySquad,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spot {
    pub troop: Troop,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub from: String,
    pub when: u64,
    /// The friend's troops, placed.
    pub side: Vec<Spot>,
    /// The friend's base (Stop my squad).
    pub base: Option<HomeBase>,
    pub stars: u32,
    pub destruction: f32,
    /// The friend beat the challenge.
    pub beat: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Challenge {
    pub game: String,
    /// The battle rules it was made for: both games must match.
    pub sim: u32,
    pub id: u64,
    pub from: String,
    pub when: u64,
    pub kind: Kind,
    /// The sender's base (Raid my base).
    pub base: Option<HomeBase>,
    /// The sender's troops, placed.
    pub side: Vec<Spot>,
    #[serde(default)]
    pub reply: Option<Reply>,
}

impl Challenge {
    /// The battle between the sender's `side` and `other` (the friend's
    /// troops), on the right base.
    pub fn battle(&self, other: &[Spot], friend_base: Option<&HomeBase>) -> Battle {
        let pair = |v: &[Spot]| v.iter().map(|s| (s.troop.clone(), (s.x, s.y))).collect::<Vec<_>>();
        match self.kind {
            Kind::RaidMyBase => Battle::defense(self.base.as_ref().unwrap_or(&HomeBase::default()), pair(&self.side), None, pair(other)),
            Kind::StopMySquad => Battle::defense(friend_base.unwrap_or(&HomeBase::default()), pair(other), None, pair(&self.side)),
        }
    }

    pub fn what(&self) -> String {
        match self.kind {
            Kind::RaidMyBase => format!("{}'s base", self.from),
            Kind::StopMySquad => format!("{}'s squad", self.from),
        }
    }

    fn file_name(&self) -> String {
        let who: String = self.from.chars().filter(|c| c.is_alphanumeric()).take(16).collect();
        let kind = if self.kind == Kind::RaidMyBase { "raid-my-base" } else { "stop-my-squad" };
        let reply = if self.reply.is_some() { "-reply" } else { "" };
        format!("{}-{kind}-{}{reply}{EXT}", if who.is_empty() { "friend".to_string() } else { who.to_lowercase() }, self.id % 100_000)
    }
}

pub fn read(path: &Path) -> Result<Challenge, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse(&s)
}

pub fn parse(s: &str) -> Result<Challenge, String> {
    let c: Challenge = serde_json::from_str(s).map_err(|_| "This isn't a Legion of Minds challenge file.".to_string())?;
    if c.game != TAG {
        return Err("This isn't a Legion of Minds challenge file.".into());
    }
    if c.sim != godcore::sim::SIM_VERSION {
        return Err(format!(
            "This challenge was made with different battle rules (version {}, yours is {}). Both of you need the same game version.",
            c.sim,
            godcore::sim::SIM_VERSION
        ));
    }
    if c.side.is_empty() {
        return Err("This challenge has no troops in it.".into());
    }
    Ok(c)
}

/// Where attackers start on a home base.
pub fn army_positions(n: usize) -> Vec<(f32, f32)> {
    (0..n).map(|k| (RAID_ENTRY.0 + 1.4 * k as f32 - 1.0, RAID_ENTRY.1 - 1.0 + 0.6 * k as f32)).collect()
}

/// A challenge being looked at in the panel.
pub struct Open {
    pub c: Challenge,
    pub path: Option<PathBuf>,
    /// Roster indexes picked for the friend's side.
    pub picked: Vec<usize>,
}

#[derive(Default)]
pub struct Panel {
    /// The file just written, to show and share.
    pub sent: Option<PathBuf>,
    pub open: Option<Open>,
    pub error: Option<String>,
    /// Challenge files found in Downloads and the challenges folder.
    found: Option<Vec<(PathBuf, Result<Challenge, String>)>>,
}

impl Panel {
    pub fn with(c: Challenge, path: Option<PathBuf>, g: &Game) -> Panel {
        let picked = g.default_side(c.kind);
        Panel { open: Some(Open { c, path, picked }), ..Default::default() }
    }
}

/// The folder challenges are written to.
fn folder(g: &Game) -> PathBuf {
    let d = g.store.dir.join("challenges");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Challenge files in Downloads and the challenges folder, newest first.
fn find(g: &Game) -> Vec<(PathBuf, Result<Challenge, String>)> {
    let mut paths: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for dir in [dirs::download_dir(), Some(folder(g))].into_iter().flatten() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(EXT) {
                let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                paths.push((e.path(), t));
            }
        }
    }
    paths.sort_by(|a, b| b.1.cmp(&a.1));
    paths.truncate(8);
    paths.into_iter().map(|(p, _)| {
        let c = read(&p);
        (p, c)
    }).collect()
}

impl Game {
    /// The player's name on challenges: their online name, if signed in.
    fn challenger_name(&self) -> String {
        self.account.user.as_ref().map(|u| u.display_name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Your friend".to_string())
    }

    /// The troops that would answer a challenge of this kind: defenders to
    /// hold the base, or the best attackers to raid it.
    pub fn default_side(&self, kind: Kind) -> Vec<usize> {
        let role = if kind == Kind::RaidMyBase { Role::Attack } else { Role::Defend };
        let mut v: Vec<usize> = (0..self.troops.len()).filter(|&i| self.troops[i].spec.role == role).collect();
        v.sort_by(|&a, &b| self.troops[b].skills.win.partial_cmp(&self.troops[a].skills.win).unwrap());
        v.truncate(if kind == Kind::RaidMyBase { MAX_SQUAD } else { MAX_DEFENDERS });
        v
    }

    /// Place troops for a challenge: defenders around the Town Hall, or
    /// attackers at the raid entry.
    fn spots(&self, idx: &[usize], defending: bool) -> Vec<Spot> {
        let pos = if defending { home_spots(idx.len()) } else { army_positions(idx.len()) };
        idx.iter().zip(pos).map(|(&i, (x, y))| Spot { troop: self.troops[i].clone(), x, y }).collect()
    }

    /// Write a new challenge of this kind. Returns where it was saved.
    fn send_challenge(&mut self, kind: Kind) -> Result<PathBuf, String> {
        let (side, base) = match kind {
            Kind::RaidMyBase => {
                let d = self.default_side(Kind::StopMySquad);
                (self.spots(&d, true), Some(self.profile.base.clone()))
            }
            Kind::StopMySquad => {
                let a = self.default_side(Kind::RaidMyBase);
                (self.spots(&a, false), None)
            }
        };
        if side.is_empty() {
            return Err("You need troops for this challenge first.".into());
        }
        let c = Challenge {
            game: TAG.into(),
            sim: godcore::sim::SIM_VERSION,
            id: self.rng.next_u64(),
            from: self.challenger_name(),
            when: unix_now(),
            kind,
            base,
            side,
            reply: None,
        };
        let p = folder(self).join(c.file_name());
        std::fs::write(&p, serde_json::to_string(&c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        self.journal.challenges_sent += 1;
        self.journal.save();
        Ok(p)
    }

    /// Files dropped on the window: open a challenge if one is among them.
    pub fn take_dropped_challenge(&mut self) {
        for f in get_dropped_files() {
            let parsed = match (&f.bytes, &f.path) {
                (Some(b), _) => parse(&String::from_utf8_lossy(b)),
                (None, Some(p)) => read(p),
                _ => continue,
            };
            match parsed {
                Ok(c) => {
                    self.settings_open = false;
                    self.overlay = Some(Overlay::Challenges(Box::new(Panel::with(c, f.path.clone(), self))));
                }
                Err(e) => self.toast(e),
            }
            return;
        }
    }

    pub(crate) fn challenges(&mut self, mut p: Box<Panel>) -> Flow {
        if p.open.is_some() {
            return self.challenge_open(p);
        }
        let r = card(980.0, 640.0);
        if close_button(self, r) {
            return Flow::close();
        }
        let (x, w) = (r.x + 32.0, r.w - 64.0);
        text("Challenge a friend", x, r.y + 48.0, 32.0, ACCENT);
        text("Send your troops as a file. Your friend fights them on their computer, and can send the battle back for you to watch.", x, r.y + 76.0, 17.0, MUTED);

        // ---- Send
        let col = (w - 20.0) / 2.0;
        let defenders = self.default_side(Kind::StopMySquad).len();
        let attackers = self.default_side(Kind::RaidMyBase).len();
        let opts = [
            (Kind::RaidMyBase, "Raid my base", if defenders == 0 {
                format!("Your home base (Town Hall {}) and up to {MAX_DEFENDERS} of your best defenders. Your friend attacks it with their squad, and wins with one star.", self.profile.base.town_hall)
            } else {
                format!("Your home base (Town Hall {}) and your {} best defender{}. Your friend attacks it with their squad, and wins with one star.", self.profile.base.town_hall, defenders, if defenders == 1 { "" } else { "s" })
            }, defenders > 0, "Recruit a defender (Guard, Sentry...) first."),
            (Kind::StopMySquad, "Stop my squad", format!("Your {} best attacker{} raid your friend's home base. They win if your squad leaves with no stars.", attackers, if attackers == 1 { "" } else { "s" }), attackers > 0, "Recruit an attacker first."),
        ];
        for (k, (kind, title, body, ok, why)) in opts.iter().enumerate() {
            let b = Rect::new(x + k as f32 * (col + 20.0), r.y + 100.0, col, 190.0);
            panel(b);
            text(title, b.x + 18.0, b.y + 34.0, 24.0, TEXT);
            crate::recruit::wrap(body, b.x + 18.0, b.y + 62.0, b.w - 36.0, 16.0, MUTED);
            if !ok {
                text(why, b.x + 18.0, b.y + b.h - 22.0, 15.0, BAD);
            } else if self.ui.button(Rect::new(b.x + 18.0, b.y + b.h - 58.0, 220.0, 42.0), "Make challenge file") {
                match self.send_challenge(*kind) {
                    Ok(path) => {
                        p.sent = Some(path);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e),
                }
            }
        }
        let mut y = r.y + 312.0;
        if let Some(path) = p.sent.clone() {
            text("Challenge saved. Send this file to a friend in any chat app:", x, y, 17.0, GOOD);
            text(&path.file_name().unwrap_or_default().to_string_lossy(), x, y + 22.0, 17.0, TEXT);
            if self.ui.button(Rect::new(x, y + 34.0, 180.0, 36.0), "Show in folder") {
                crate::clip::show_in_folder(&path);
            }
            if self.ui.button(Rect::new(x + 190.0, y + 34.0, 230.0, 36.0), "Copy a message to send") {
                macroquad::miniquad::window::clipboard_set("I challenge you in Legion of Minds! Drag the file I sent onto your game window to take me on.");
                self.toast("Message copied. Paste it with the file.");
            }
            y += 84.0;
        } else if let Some(e) = &p.error {
            text(e, x, y, 17.0, BAD);
            y += 30.0;
        }

        // ---- Received
        y = y.max(r.y + 410.0);
        draw_line(x, y - 14.0, x + w, y - 14.0, 1.0, LINE);
        text("Got a challenge?", x, y + 10.0, 22.0, TEXT);
        text("Drag the file onto this window, or save it in your Downloads folder and it shows up here.", x, y + 32.0, 16.0, MUTED);
        if p.found.is_none() || self.ui.button(Rect::new(x + w - 120.0, y - 6.0, 120.0, 32.0), "Look again") {
            p.found = Some(find(self));
        }
        y += 46.0;
        let found = p.found.clone().unwrap_or_default();
        if found.is_empty() {
            text("No challenge files found yet.", x, y + 18.0, 17.0, MUTED);
        }
        for (path, c) in found.iter().take(4) {
            let row = Rect::new(x, y, w, 40.0);
            panel(row);
            match c {
                Ok(c) => {
                    let line = match &c.reply {
                        Some(rp) => format!("{} answered your challenge: {}", rp.from, if rp.beat { "they beat it" } else { "you held" }),
                        None => format!("{} from {}  ·  {}", if c.kind == Kind::RaidMyBase { "Raid my base" } else { "Stop my squad" }, c.from, crate::journal::age_words(c.when, unix_now()) + " ago"),
                    };
                    text(&line, row.x + 12.0, row.y + 26.0, 17.0, TEXT);
                    let label = if c.reply.is_some() { "Watch" } else { "Take it on" };
                    if self.ui.button(Rect::new(row.x + row.w - 140.0, row.y + 4.0, 132.0, 32.0), label) {
                        let pn = Panel::with(c.clone(), Some(path.clone()), self);
                        p.open = pn.open;
                    }
                }
                Err(e) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    text(&format!("{name}: {e}"), row.x + 12.0, row.y + 26.0, 15.0, MUTED);
                }
            }
            y += 46.0;
        }
        Flow::stay(Overlay::Challenges(p))
    }

    /// A challenge picked: choose who answers it and start the battle, or
    /// watch a reply.
    fn challenge_open(&mut self, mut p: Box<Panel>) -> Flow {
        let r = card(900.0, 600.0);
        let mut o = p.open.take().unwrap();
        let (x, w) = (r.x + 32.0, r.w - 64.0);
        if self.ui.button(Rect::new(r.x + r.w - 130.0, r.y + 14.0, 116.0, 32.0), "< Back") {
            return Flow::stay(Overlay::Challenges(p));
        }
        let c = &o.c;
        if let Some(rp) = &c.reply {
            // My challenge came back.
            text(&format!("{} took your challenge", rp.from), x, r.y + 50.0, 30.0, ACCENT);
            let res = match c.kind {
                Kind::RaidMyBase => format!("They raided your base: {} star{}, {:.0}% destroyed.", rp.stars, if rp.stars == 1 { "" } else { "s" }, rp.destruction * 100.0),
                Kind::StopMySquad => format!("Your squad raided their base: {} star{}, {:.0}% destroyed.", rp.stars, if rp.stars == 1 { "" } else { "s" }, rp.destruction * 100.0),
            };
            text(&res, x, r.y + 90.0, 20.0, TEXT);
            text(if rp.beat { "They beat your challenge. Time for revenge?" } else { "Your troops held them off!" }, x, r.y + 120.0, 20.0, if rp.beat { BAD } else { GOOD });
            let names: Vec<String> = rp.side.iter().map(|s| format!("{} (Lv{} {})", s.troop.name.trim(), s.troop.level, s.troop.spec.name)).collect();
            crate::recruit::wrap(&format!("Their troops: {}", names.join(", ")), x, r.y + 160.0, w, 17.0, MUTED);
            if self.ui.button(Rect::new(x, r.y + r.h - 80.0, 260.0, 50.0), "Watch the battle") {
                let b = BattleState::challenge(c.clone(), rp.side.clone(), rp.base.clone(), true, Vec::new());
                return Flow::go(Screen::Battle(b));
            }
            p.open = Some(o);
            return Flow::stay(Overlay::Challenges(p));
        }
        let me_defend = c.kind == Kind::StopMySquad;
        text(&format!("Challenge from {}", c.from), x, r.y + 50.0, 30.0, ACCENT);
        let line = match c.kind {
            Kind::RaidMyBase => format!("Raid {}'s base (Town Hall {}). Take at least one star to win.", c.from, c.base.as_ref().map_or(1, |b| b.town_hall)),
            Kind::StopMySquad => format!("{}'s squad raids your home base. Leave them with no stars to win.", c.from),
        };
        text(&line, x, r.y + 82.0, 19.0, TEXT);
        let theirs: Vec<String> = c.side.iter().map(|s| format!("{} (Lv{} {})", s.troop.name.trim(), s.troop.level, s.troop.spec.name)).collect();
        crate::recruit::wrap(&format!("Their troops: {}", theirs.join(", ")), x, r.y + 112.0, w, 16.0, MUTED);

        let role = if me_defend { Role::Defend } else { Role::Attack };
        let max = if me_defend { MAX_DEFENDERS } else { MAX_SQUAD };
        text(
            &format!("Your {} ({}/{max}), click to pick", if me_defend { "defenders" } else { "squad" }, o.picked.len()),
            x,
            r.y + 170.0,
            20.0,
            TEXT,
        );
        let eligible: Vec<usize> = (0..self.troops.len()).filter(|&i| self.troops[i].spec.role == role).collect();
        for (k, &i) in eligible.iter().enumerate().take(12) {
            let t = &self.troops[i];
            let cell = Rect::new(x + (k % 3) as f32 * ((w + 12.0) / 3.0), r.y + 184.0 + (k / 3) as f32 * 58.0, (w + 12.0) / 3.0 - 12.0, 50.0);
            let on = o.picked.contains(&i);
            draw_rectangle(cell.x, cell.y, cell.w, cell.h, if on { PANEL_HI } else { PANEL });
            draw_rectangle_lines(cell.x, cell.y, cell.w, cell.h, if on { 2.0 } else { 1.0 }, if on { GOOD } else { LINE });
            crate::art::draw_portrait(vec2(cell.x + 26.0, cell.y + 25.0), 17.0, &t.spec);
            text(&t.name, cell.x + 50.0, cell.y + 22.0, 19.0, TEXT);
            text(&format!("Lv{} {}  {:.0}% wins", t.level, t.spec.name, t.skills.win * 100.0), cell.x + 50.0, cell.y + 40.0, 14.0, MUTED);
            if self.ui.hover(cell) && self.ui.clicked {
                if on {
                    o.picked.retain(|&j| j != i);
                } else if o.picked.len() < max {
                    o.picked.push(i);
                }
            }
        }
        if eligible.is_empty() {
            let why = if me_defend {
                "You have no defenders, so only your towers will hold the line. You can still take it on."
            } else {
                "You have no attackers to raid with. Recruit one first."
            };
            crate::recruit::wrap(why, x, r.y + 210.0, w, 17.0, MUTED);
        }
        let can = me_defend || !o.picked.is_empty();
        if self.ui.button_ex(Rect::new(x, r.y + r.h - 80.0, 260.0, 50.0), "FIGHT!", can, false) {
            o.picked.sort();
            let mine = self.spots(&o.picked, me_defend);
            let base = me_defend.then(|| self.profile.base.clone());
            let ids = o.picked.iter().map(|&i| self.troops[i].id).collect();
            let b = BattleState::challenge(o.c.clone(), mine, base, false, ids);
            return Flow::go(Screen::Battle(b));
        }
        text("Everyone starts fresh: tiredness and training in progress don't count.", x + 280.0, r.y + r.h - 50.0, 15.0, MUTED);
        p.open = Some(o);
        Flow::stay(Overlay::Challenges(p))
    }

    /// Save the battle just fought as a reply file for the friend.
    pub fn save_reply(&mut self, run: &Run, stars: u32, destruction: f32, beat: bool) -> Result<PathBuf, String> {
        let mut c = run.c.clone();
        c.reply = Some(Reply {
            from: self.challenger_name(),
            when: unix_now(),
            side: run.other.clone(),
            base: run.friend_base.clone(),
            stars,
            destruction,
            beat,
        });
        let p = folder(self).join(c.file_name());
        std::fs::write(&p, serde_json::to_string(&c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        Ok(p)
    }
}

/// Test hook: a "Stop my squad" challenge made from the player's own
/// attackers, as a friend would receive it.
pub fn demo(g: &mut Game) -> Challenge {
    let a = g.default_side(Kind::RaidMyBase);
    Challenge {
        game: TAG.into(),
        sim: godcore::sim::SIM_VERSION,
        id: 7,
        from: "Asha".into(),
        when: unix_now() - 7200,
        kind: Kind::StopMySquad,
        base: None,
        side: g.spots(&a, false),
        reply: None,
    }
}

/// A challenge battle in progress (kept by the battle screen).
#[derive(Clone)]
pub struct Run {
    pub c: Challenge,
    /// The side answering the challenge, and its base (Stop my squad).
    pub other: Vec<Spot>,
    pub friend_base: Option<HomeBase>,
    /// Watching a reply: nothing is recorded.
    pub watching: bool,
    /// The player's own troop ids in this battle (none when watching).
    pub mine: Vec<u64>,
    /// The reply file, once saved.
    pub saved: Option<PathBuf>,
}

impl Run {
    /// Whether the player is the defending side.
    pub fn i_defend(&self) -> bool {
        // Answering: I defend against "Stop my squad". Watching my own
        // challenge come back: I defend my base in "Raid my base".
        (self.c.kind == Kind::StopMySquad) != self.watching
    }

    pub fn title(&self) -> String {
        match (&self.c.reply, self.watching) {
            (Some(r), true) => format!("{} takes on your challenge", r.from),
            _ => format!("Challenge: {}", self.c.what()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use godcore::classes::BaseClass;

    fn spot(class: BaseClass, id: u64, x: f32, y: f32) -> Spot {
        Spot { troop: Troop::new(id, format!("T{id}"), class.spec(), id), x, y }
    }

    #[test]
    fn a_challenge_file_round_trips_and_replays_the_same_battle() {
        let c = Challenge {
            game: TAG.into(),
            sim: godcore::sim::SIM_VERSION,
            id: 42,
            from: "Sanjay".into(),
            when: 1,
            kind: Kind::RaidMyBase,
            base: Some(HomeBase::default()),
            side: home_spots(1).into_iter().map(|(x, y)| spot(BaseClass::Guard, 1, x, y)).collect(),
            reply: None,
        };
        let back = parse(&serde_json::to_string(&c).unwrap()).unwrap();
        let mine: Vec<Spot> = army_positions(2).into_iter().enumerate().map(|(k, (x, y))| spot(BaseClass::Fighter, 10 + k as u64, x, y)).collect();
        let mut a = c.battle(&mine, None);
        let mut b = back.battle(&mine, None);
        a.run();
        b.run();
        assert_eq!(godcore::battle::fingerprint(&a.world), godcore::battle::fingerprint(&b.world));
        assert!(c.file_name().ends_with(EXT));
    }

    #[test]
    fn wrong_files_and_other_rules_are_refused_politely() {
        assert!(parse("{\"hello\": 1}").is_err());
        let mut c = Challenge {
            game: TAG.into(),
            sim: godcore::sim::SIM_VERSION + 1,
            id: 1,
            from: "x".into(),
            when: 1,
            kind: Kind::StopMySquad,
            base: None,
            side: vec![spot(BaseClass::Fighter, 1, 4.0, 4.0)],
            reply: None,
        };
        let e = parse(&serde_json::to_string(&c).unwrap()).unwrap_err();
        assert!(e.contains("battle rules"), "{e}");
        c.sim = godcore::sim::SIM_VERSION;
        c.side.clear();
        assert!(parse(&serde_json::to_string(&c).unwrap()).is_err());
    }
}
