//! Learning moments in the game: while a troop trains (alone or with a
//! team, in the background), its brain is checked every so often against
//! the brain it started the session with, on a fixed set of test drills
//! (see `godcore::moments`). A clear new behaviour pops up as "<name>
//! learned to ...!" with a Watch button, and is kept in the troop's "What it
//! learned" timeline, where it can be replayed, renamed, saved as a video
//! and shared.
//!
//! The checks run on a thread of their own so training and the window never
//! wait for them. A moment keeps only the drill's seed and the two brains,
//! so a replay is the exact same drill, never a recording.

use crate::barracks::fmt_steps;
use crate::render::{draw_ground, draw_world, View};
use crate::save::Store;
use crate::ui::*;
use crate::{Game, Screen};
use godcore::classes::ClassSpec;
use godcore::moments::{compare, Found, Moment, MomentBrains, Probe, Run};
use godcore::nn::Mlp;
use godcore::troop::unix_now;
use macroquad::prelude::*;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Training steps between checks.
const CHECK_EVERY: u64 = 25_000;
/// Seconds between checks at least (big brains train slowly but probe slowly too).
const MIN_CHECK_SECS: f64 = 20.0;
/// Most moments one training session can make: only the biggest few.
pub const PER_SESSION: usize = 3;
/// Most moments a troop keeps; the oldest go first.
const KEEP: usize = 40;
/// Disk space a troop's moments may take. Each keeps two brains, and big
/// brains are big (a 1024-neuron one is about 6 MB saved, a 4096 one 90 MB),
/// so troops with big brains keep fewer moments.
const DISK_BUDGET: usize = 300 << 20;

enum Job {
    /// A session began: probe the brain it starts with.
    Start { id: u64, spec: ClassSpec, actor: Mlp, steps: u64, seed: u64, best: BTreeMap<String, f32> },
    /// Probe the newest brain and compare.
    Check { id: u64, actor: Mlp, steps: u64 },
    End { id: u64 },
}

/// A moment found by the checker thread.
struct Out {
    id: u64,
    found: Found,
    brains: MomentBrains,
    steps_before: u64,
    steps: u64,
    spec: ClassSpec,
    seed: u64,
}

struct Session {
    spec: ClassSpec,
    seed: u64,
    base: Probe,
    base_actor: Mlp,
    base_steps: u64,
    best: BTreeMap<String, f32>,
    found: usize,
    /// Kinds of change already found this session (each counts once).
    seen: Vec<String>,
}

/// Keeps track of the training sessions being watched, and the moments
/// waiting to pop up.
#[derive(Default)]
pub struct Tracker {
    link: Option<(Sender<Job>, Receiver<Out>)>,
    tracks: HashMap<u64, Track>,
    /// Moments to pop up, as (troop id, moment id).
    pub popups: VecDeque<(u64, u64)>,
}

struct Track {
    /// The troop's steps when the session began.
    base: u64,
    /// Steps trained this session when the next check is due.
    next: u64,
    last_check: f64,
}

/// The checker thread: probes brains and compares them.
fn checker(jobs: Receiver<Job>, out: Sender<Out>) {
    let mut sessions: HashMap<u64, Session> = HashMap::new();
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        // A check that a newer one for the same troop overtook is skipped.
        let n = batch.len();
        for k in 0..n {
            if let Job::Check { id, .. } = &batch[k] {
                let id = *id;
                let newer = batch[k + 1..].iter().any(|j| matches!(j, Job::Check { id: i, .. } if *i == id));
                let reset = batch[k + 1..].iter().any(|j| matches!(j, Job::Start { id: i, .. } | Job::End { id: i } if *i == id));
                if newer && !reset {
                    continue;
                }
            }
            match std::mem::replace(&mut batch[k], Job::End { id: u64::MAX }) {
                Job::Start { id, spec, actor, steps, seed, best } => {
                    let base = Probe::run(&spec, &actor, seed);
                    sessions.insert(id, Session { spec, seed, base, base_actor: actor, base_steps: steps, best, found: 0, seen: Vec::new() });
                }
                Job::End { id } => {
                    sessions.remove(&id);
                }
                Job::Check { id, actor, steps } => {
                    let Some(s) = sessions.get_mut(&id) else { continue };
                    if s.found >= PER_SESSION {
                        continue;
                    }
                    let p = Probe::run(&s.spec, &actor, s.seed);
                    for f in compare(&s.spec, &s.base, &s.base_actor, &p, &actor, &s.best) {
                        if s.found >= PER_SESSION {
                            break;
                        }
                        if s.seen.contains(&f.key) {
                            continue;
                        }
                        s.seen.push(f.key.clone());
                        s.found += 1;
                        s.best.insert(f.key.clone(), f.after);
                        let o = Out {
                            id,
                            brains: MomentBrains { before: s.base_actor.clone(), after: actor.clone() },
                            steps_before: s.base_steps,
                            steps,
                            spec: s.spec.clone(),
                            seed: s.seed,
                            found: f,
                        };
                        if out.send(o).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl Tracker {
    fn send(&mut self, j: Job) {
        let (tx, _) = self.link.get_or_insert_with(|| {
            let (jt, jr) = channel();
            let (ot, or) = channel();
            std::thread::Builder::new()
                .name("learning moments".into())
                .spawn(move || checker(jr, ot))
                .expect("start the learning moments thread");
            (jt, or)
        });
        let _ = tx.send(j);
    }
}

impl Store {
    fn moment_dir(&self, troop: u64) -> PathBuf {
        self.dir.join("moments").join(troop.to_string())
    }

    /// Saves a moment's brains on a thread of its own: big ones take a
    /// moment to write and the window shouldn't wait.
    pub fn save_moment_brains(&self, troop: u64, moment: u64, b: MomentBrains) {
        let d = self.moment_dir(troop);
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&d);
            crate::save::save_file(&d.join(format!("{moment}.json")), serde_json::to_string(&b).unwrap().as_bytes());
        });
    }

    pub fn load_moment_brains(&self, troop: u64, moment: u64) -> Option<MomentBrains> {
        let p = self.moment_dir(troop).join(format!("{moment}.json"));
        crate::save::load_file(&p, |s| serde_json::from_str(s).ok()).map(|(b, _)| b)
    }

    pub fn delete_moment_brains(&self, troop: u64, moment: u64) {
        let p = self.moment_dir(troop).join(format!("{moment}.json"));
        let _ = std::fs::remove_file(&p);
        let mut bak = p.into_os_string();
        bak.push(".bak");
        let _ = std::fs::remove_file(bak);
    }

    /// Everything kept for a troop's moments (when it is released).
    pub fn delete_moments(&self, troop: u64) {
        let _ = std::fs::remove_dir_all(self.moment_dir(troop));
    }
}

impl Game {
    /// Called every frame after training ticks: start, check and end the
    /// watched sessions, and take in moments the checker found.
    pub fn tick_moments(&mut self) {
        // Troops training now: (id, steps trained this session, where).
        let mut live: Vec<(u64, u64, Option<(usize, usize)>)> = Vec::new();
        for r in &self.solo_runs {
            live.push((r.id, r.trained(), None));
        }
        for (k, r) in self.team_runs.iter().enumerate() {
            for (m, &id) in r.ids.iter().enumerate() {
                live.push((id, r.trained(m), Some((k, m))));
            }
        }
        let now = get_time();
        // Sessions that ended: one last check with the brain they ended on.
        let ended: Vec<u64> = self.moments.tracks.keys().copied().filter(|id| !live.iter().any(|l| l.0 == *id)).collect();
        for id in ended {
            self.moments.tracks.remove(&id);
            if let Some(i) = self.index_of(id) {
                let t = &self.troops[i];
                self.moments.send(Job::Check { id, actor: t.actor.clone(), steps: t.steps });
            }
            self.moments.send(Job::End { id });
        }
        for (id, trained, team) in live {
            let Some(i) = self.index_of(id) else { continue };
            match self.moments.tracks.get_mut(&id) {
                None => {
                    // A new session: the troop still has the brain it starts with.
                    let t = &self.troops[i];
                    let job = Job::Start { id, spec: t.spec.clone(), actor: t.actor.clone(), steps: t.steps, seed: t.seed, best: t.lessons.clone() };
                    self.moments.tracks.insert(id, Track { base: t.steps.saturating_sub(trained), next: trained + CHECK_EVERY, last_check: now });
                    self.moments.send(job);
                }
                Some(tr) if trained >= tr.next && now - tr.last_check >= MIN_CHECK_SECS => {
                    tr.next = trained + CHECK_EVERY;
                    tr.last_check = now;
                    let brain = match team {
                        None => self.solo_runs.iter().find(|r| r.id == id).and_then(|r| r.newest_brain()),
                        Some((k, m)) => self.team_runs[k].newest_brain(m),
                    };
                    if let Some(actor) = brain {
                        let steps = tr.base + trained;
                        self.moments.send(Job::Check { id, actor, steps });
                    }
                }
                _ => {}
            }
        }
        let found: Vec<Out> = self.moments.link.as_ref().map(|(_, rx)| rx.try_iter().collect()).unwrap_or_default();
        for o in found {
            self.add_moment(o);
        }
    }

    fn add_moment(&mut self, o: Out) {
        let Some(i) = self.index_of(o.id) else { return };
        let t = &mut self.troops[i];
        let id = t.moments.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        let f = o.found;
        let m = Moment {
            id,
            text: f.text,
            title: None,
            key: f.key.clone(),
            before: f.before,
            after: f.after,
            size: f.size,
            steps_before: o.steps_before,
            steps: o.steps.max(o.steps_before),
            when: unix_now(),
            probe: f.probe,
            seed: godcore::moments::probe_seed(o.seed, f.probe),
            tick: f.tick,
            spec: o.spec,
            video: None,
        };
        let best = t.lessons.entry(f.key).or_insert(f.after);
        *best = best.max(f.after);
        t.moments.push(m);
        // About 5.4 bytes a number for two brains saved packed.
        let per = (o.brains.before.params.len() + o.brains.after.params.len()) * 11 / 2;
        let keep = (DISK_BUDGET / per.max(1)).clamp(1, KEEP);
        let mut dropped = Vec::new();
        while t.moments.len() > keep {
            dropped.push(t.moments.remove(0).id);
        }
        let tid = t.id;
        self.store.save_moment_brains(tid, id, o.brains);
        for d in dropped {
            self.store.delete_moment_brains(tid, d);
        }
        self.store.save_troop(&self.troops[i]);
        println!("Learning moment: {} {} (at {} steps)", self.troops[i].name, self.troops[i].moments.last().unwrap().text, self.troops[i].steps);
        self.moments.popups.push_back((tid, id));
        crate::sfx::play_ui(crate::sfx::Sfx::Shimmer);
    }

    /// The saved video of a moment, remembered for sharing.
    pub fn moment_video_saved(&mut self, troop: u64, moment: u64, p: &std::path::Path) {
        let Some(i) = self.index_of(troop) else { return };
        if let Some(m) = self.troops[i].moments.iter_mut().find(|m| m.id == moment) {
            m.video = Some(p.display().to_string());
            self.store.save_troop(&self.troops[i]);
        }
    }

    /// The moment shown in the pop-up now, if any (dropping ones whose
    /// troop has gone).
    fn popup_now(&mut self) -> Option<(u64, u64, String)> {
        while let Some(&(tid, mid)) = self.moments.popups.front() {
            let found = self.index_of(tid).and_then(|i| {
                let t = &self.troops[i];
                t.moments.iter().find(|m| m.id == mid).map(|m| format!("{} {}!", t.name, m.caption()))
            });
            match found {
                Some(line) => return Some((tid, mid, line)),
                None => {
                    self.moments.popups.pop_front();
                }
            }
        }
        None
    }

    /// The pop-up's place on screen and its two buttons.
    fn popup_rects() -> (Rect, Rect, Rect) {
        let r = Rect::new(W - 520.0, 62.0, 500.0, 118.0);
        (r, Rect::new(r.x + 18.0, r.y + 66.0, 150.0, 38.0), Rect::new(r.x + 178.0, r.y + 66.0, 110.0, 38.0))
    }

    /// Before the screen draws: a click on the pop-up is the pop-up's, not
    /// the screen's. Returns the moment to open if Watch was clicked.
    pub fn moment_popup_input(&mut self) -> Option<(u64, u64)> {
        let (tid, mid, _) = self.popup_now()?;
        let (r, watch, later) = Self::popup_rects();
        if !(self.ui.clicked && self.ui.hover(r)) {
            return None;
        }
        self.ui.clicked = false;
        if watch.contains(self.ui.mouse) {
            crate::sfx::play_ui(crate::sfx::Sfx::Click);
            self.moments.popups.pop_front();
            return Some((tid, mid));
        }
        if later.contains(self.ui.mouse) {
            crate::sfx::play_ui(crate::sfx::Sfx::Click);
            self.moments.popups.pop_front();
        }
        None
    }

    /// After the screen draws: the pop-up on top.
    pub fn draw_moment_popup(&mut self) {
        let Some((_, _, line)) = self.popup_now() else { return };
        let (r, watch, later) = Self::popup_rects();
        draw_rectangle(r.x + 4.0, r.y + 5.0, r.w, r.h, Color::new(0.0, 0.0, 0.0, 0.35));
        draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.11, 0.1, 0.07, 0.97));
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
        // A little burst so it catches the eye.
        let t = get_time() as f32;
        for k in 0..6 {
            let a = k as f32 * 1.047 + t * 0.8;
            let c = vec2(r.x + r.w - 46.0, r.y + 40.0);
            draw_line(c.x + a.cos() * 10.0, c.y + a.sin() * 10.0, c.x + a.cos() * 20.0, c.y + a.sin() * 20.0, 2.0, ACCENT);
        }
        text("LEARNING MOMENT", r.x + 18.0, r.y + 24.0, 16.0, ACCENT);
        let size = if text_width(&line, 22.0) > r.w - 90.0 { 22.0 * (r.w - 90.0) / text_width(&line, 22.0) } else { 22.0 };
        text(&line, r.x + 18.0, r.y + 52.0, size.max(14.0), TEXT);
        self.ui.button(watch, "Watch");
        self.ui.button(later, "Later");
        let more = self.moments.popups.len().saturating_sub(1);
        if more > 0 {
            text(&format!("+{more} more"), r.x + 300.0, r.y + 92.0, 16.0, MUTED);
        }
    }
}

// ------------------------------------------------------------ the screen

/// A troop's "What it learned" timeline, with a replay of the picked moment.
pub struct MomentsState {
    id: u64,
    /// The moment picked, by id.
    sel: Option<u64>,
    /// Showing the brain after the moment (true) or before it.
    after: bool,
    play: Option<Play>,
    brains: Option<(u64, MomentBrains)>,
    scroll: f32,
    /// The new name being typed, while renaming.
    rename: Option<String>,
    back: Option<Box<Screen>>,
}

struct Play {
    run: Run,
    acc: f32,
    end_hold: f32,
}

/// Replay speed, sim ticks a second (the game's own battle view runs 6).
const WATCH_TPS: f32 = 10.0;
/// How many ticks before the moment the replay starts.
const LEAD_IN: u32 = 40;

impl MomentsState {
    /// The timeline of troop `id`, with moment `sel` picked (or the newest).
    pub fn open(g: &Game, id: u64, sel: Option<u64>, back: Screen) -> MomentsState {
        let newest = g.index_of(id).and_then(|i| g.troops[i].moments.last().map(|m| m.id));
        MomentsState { id, sel: sel.or(newest), after: true, play: None, brains: None, scroll: 0.0, rename: None, back: Some(Box::new(back)) }
    }

    fn leave(&mut self) -> Screen {
        self.back.take().map(|b| *b).unwrap_or(Screen::Barracks)
    }
}

/// When a moment was found, in words.
fn ago(when: u64) -> String {
    let s = unix_now().saturating_sub(when);
    match s {
        0..=89 => "just now".into(),
        90..=5399 => format!("{} min ago", (s + 30) / 60),
        5400..=129_599 => format!("{} h ago", (s + 1800) / 3600),
        _ => format!("{} days ago", (s + 43_200) / 86_400),
    }
}

/// What changed, in numbers, for the moments that have a stat behind them.
fn change_line(m: &Moment) -> Option<String> {
    let pct = |v: f32| format!("{:.0}%", v * 100.0);
    let per = |v: f32| format!("{v:.1} a drill");
    Some(match m.key.as_str() {
        "wins" => format!("Test drills won: {} -> {}", pct(m.before), pct(m.after)),
        "survives" => format!("Came back alive: {} -> {}", pct(m.before), pct(m.after)),
        "heal_first" => format!("Heals on the more hurt ally: {} -> {}", pct(m.before), pct(m.after)),
        "distance" => format!("Time out of enemy reach in a fight: {} -> {}", pct(m.before), pct(m.after)),
        "dodges" => format!("Blows dodged: {} -> {}", per(m.before), per(m.after)),
        "blocks" => format!("Damage blocked: {:.0} -> {:.0} a drill", m.before, m.after),
        "parries" => format!("Blows parried: {} -> {}", per(m.before), per(m.after)),
        "skill" => format!("Skill used: {} -> {}", per(m.before), per(m.after)),
        "destruction" => format!("Base destroyed: {} -> {}", pct(m.before), pct(m.after)),
        "pacing" => format!("Time with breath to spare: {} -> {}", pct(m.before), pct(m.after)),
        "calls" => format!("Calls to the squad: {} -> {}", per(m.before), per(m.after)),
        "kills" => format!("Enemies finished off: {} -> {}", per(m.before), per(m.after)),
        "shift" => format!("Acts differently in {} of the same situations", pct(m.after)),
        k if k.starts_with("move:") => format!("Used in {:.0} of {} test drills, before in {:.0}", m.after, godcore::moments::PROBES, m.before),
        _ => return None,
    })
}

impl Game {
    pub fn moments_screen(&mut self, mut s: MomentsState) -> Screen {
        let Some(i) = self.index_of(s.id) else { return s.leave() };
        let title = format!("What {} learned", self.troops[i].name);
        if self.header(&title, true) {
            return s.leave();
        }
        // ---- Timeline (left), newest first.
        let list = Rect::new(20.0, 66.0, 400.0, H - 86.0);
        panel(list);
        let n = self.troops[i].moments.len();
        if n == 0 {
            crate::recruit::wrap(
                &format!("No learning moments yet. Train {} and a pop-up appears when it picks up something new: how it fights, heals, dodges or moves. Each one is kept here to watch again, save as a video and share.", self.troops[i].name),
                list.x + 20.0,
                list.y + 40.0,
                list.w - 40.0,
                18.0,
                MUTED,
            );
            return Screen::Moments(s);
        }
        let row = 96.0;
        if self.ui.hover(list) {
            s.scroll = (s.scroll - crate::record::wheel() * 40.0).clamp(0.0, (n as f32 * row - list.h + 20.0).max(0.0));
        }
        let name = self.troops[i].name.clone();
        let mut clicked = None;
        for (k, m) in self.troops[i].moments.iter().rev().enumerate() {
            let r = Rect::new(list.x + 10.0, list.y + 10.0 + k as f32 * row - s.scroll, list.w - 20.0, row - 8.0);
            if r.y + r.h < list.y || r.y > list.y + list.h {
                continue;
            }
            let on = s.sel == Some(m.id);
            draw_rectangle(r.x, r.y, r.w, r.h, if on { Color::new(0.3, 0.23, 0.1, 1.0) } else if self.ui.hover(r) { PANEL_HI } else { BG });
            draw_rectangle_lines(r.x, r.y, r.w, r.h, if on { 2.0 } else { 1.0 }, if on { ACCENT } else { LINE });
            // The timeline's line and dot.
            draw_line(r.x + 18.0, r.y - 8.0, r.x + 18.0, r.y + r.h, 2.0, LINE);
            draw_circle(r.x + 18.0, r.y + 22.0, 6.0, if on { ACCENT } else { SKY });
            text(&format!("at {} steps  ·  {}", fmt_steps(m.steps), ago(m.when)), r.x + 34.0, r.y + 22.0, 15.0, MUTED);
            crate::recruit::wrap(&format!("{name} {}", m.caption()), r.x + 34.0, r.y + 46.0, r.w - 46.0, 18.0, TEXT);
            if self.ui.hover(r) && self.ui.clicked && list.contains(self.ui.mouse) {
                clicked = Some(m.id);
            }
        }
        if let Some(id) = clicked {
            if s.sel != Some(id) {
                s.sel = Some(id);
                s.play = None;
                s.after = true;
                s.rename = None;
            }
        }
        // ---- The picked moment (right).
        let Some(m) = s.sel.and_then(|id| self.troops[i].moments.iter().find(|m| m.id == id)).cloned() else {
            s.sel = self.troops[i].moments.last().map(|m| m.id);
            return Screen::Moments(s);
        };
        if s.brains.as_ref().is_none_or(|(id, _)| *id != m.id) {
            s.brains = self.store.load_moment_brains(s.id, m.id).map(|b| (m.id, b));
            s.play = None;
        }
        let x = 440.0;
        let line = format!("{name} {}!", m.caption());
        let size = if text_width(&line, 28.0) > 800.0 { 28.0 * 800.0 / text_width(&line, 28.0) } else { 28.0 };
        text(&line, x, 92.0, size, ACCENT);
        let view = View::new(Rect::new(x, 110.0, 560.0, 560.0));
        let Some((_, brains)) = &s.brains else {
            crate::recruit::wrap("The brains saved with this moment could not be read, so it can't be replayed.", x, 140.0, 540.0, 18.0, BAD);
            return Screen::Moments(s);
        };
        let brain = if s.after { brains.after.clone() } else { brains.before.clone() };
        if s.play.is_none() {
            let mut run = m.replay();
            // Skip ahead to just before the moment.
            let start = m.tick.saturating_sub(LEAD_IN);
            while run.world.tick < start && !run.over() {
                run.step(&brain);
            }
            self.fx.clear();
            s.play = Some(Play { run, acc: 0.0, end_hold: 0.0 });
        }
        let p = s.play.as_mut().unwrap();
        let dt = crate::record::dt().min(0.1);
        if !p.run.over() {
            p.acc += dt * WATCH_TPS;
            while p.acc >= 1.0 && !p.run.over() {
                p.acc -= 1.0;
                p.run.step(&brain);
                self.fx.absorb(&p.run.world);
            }
        } else {
            p.end_hold += dt;
        }
        self.fx.age(dt);
        draw_ground(view, false, true, &p.run.world);
        let fig = crate::art::Figure::of(&m.spec);
        let mine = p.run.world.units[p.run.controlled[0]].team;
        let controlled = p.run.controlled.clone();
        draw_world(view, &p.run.world, &self.fx, &|k, u| crate::art::drill_look(k, u, &controlled, mine, fig, &name));
        // Point at the troop while the moment happens.
        let now = p.run.world.tick;
        let here = s.after && m.tick > 0 && now >= m.tick && now < m.tick + 30;
        if here {
            let u = &p.run.world.units[p.run.controlled[0]];
            if u.alive {
                let c = view.p(u.x, u.y);
                let k = (get_time() as f32 * 6.0).sin().abs();
                draw_circle_lines(c.x, c.y, view.s() * (1.1 + 0.3 * k), 3.0, ACCENT);
            }
            let tag = "It happens here";
            draw_rectangle(view.r.x + view.r.w - text_width(tag, 20.0) - 30.0, view.r.y + 8.0, text_width(tag, 20.0) + 20.0, 30.0, Color::new(0.5, 0.33, 0.06, 0.95));
            text(tag, view.r.x + view.r.w - text_width(tag, 20.0) - 20.0, view.r.y + 29.0, 20.0, TEXT);
        }
        let which = if s.after { format!("After  ·  {} steps", fmt_steps(m.steps)) } else { format!("Before  ·  {} steps", fmt_steps(m.steps_before)) };
        draw_rectangle(view.r.x, view.r.y, text_width(&which, 20.0) + 20.0, 30.0, Color::new(0.0, 0.0, 0.0, 0.55));
        text(&which, view.r.x + 10.0, view.r.y + 21.0, 20.0, if s.after { GOOD } else { BAD });
        if p.run.over() {
            let won = p.run.won();
            let msg = match (p.run.defense, won) {
                (true, true) => "Base held",
                (true, false) => "Base fallen",
                (false, true) => "Drill won",
                (false, false) if !p.run.world.units[0].alive => "Fell in battle",
                _ => "Out of time",
            };
            text_centered(msg, view.r.x + view.r.w / 2.0, view.r.y + 70.0, 36.0, if won { GOOD } else { BAD });
        }
        // Under the replay: before or after, and again.
        let by = view.r.y + view.r.h + 14.0;
        if self.ui.button_ex(Rect::new(x, by, 175.0, 44.0), "Before", true, !s.after) && s.after {
            s.after = false;
            s.play = None;
        }
        if self.ui.button_ex(Rect::new(x + 185.0, by, 175.0, 44.0), "After", true, s.after) && !s.after {
            s.after = true;
            s.play = None;
        }
        if self.ui.button(Rect::new(x + 370.0, by, 190.0, 44.0), "Watch again") {
            s.play = None;
        }
        text("The same test drill, the same start. Only the brain is different.", x, by + 70.0, 16.0, MUTED);

        // ---- Right column: what changed, and what to do with it.
        let rx = 1020.0;
        let rw = W - rx - 20.0;
        let mut y = 130.0;
        text("What changed", rx, y, 20.0, TEXT);
        y += 12.0;
        if let Some(c) = change_line(&m) {
            crate::recruit::wrap(&c, rx, y + 16.0, rw, 16.0, MUTED);
            y += 50.0;
        }
        crate::recruit::wrap(&format!("Seen in most of {} test drills, so it's not luck.", godcore::moments::PROBES), rx, y + 16.0, rw, 16.0, MUTED);
        y += 70.0;
        if self.ui.button(Rect::new(rx, y, rw, 46.0), "Save video") {
            let t = self.troops[i].clone();
            let b = brains.clone();
            let back = Screen::Moments(MomentsState { play: None, brains: None, rename: None, ..s });
            return Screen::Clip(crate::clip::ClipState::moment(&t, &m, b.before, b.after, back));
        }
        y += 56.0;
        let video = m.video.as_ref().map(PathBuf::from).filter(|p| p.exists());
        if let Some(v) = &video {
            text("Share the saved video:", rx, y + 14.0, 16.0, MUTED);
            y += 24.0;
            let words = crate::share::post_text(&line);
            // One column of buttons in the narrow space.
            for (k, label) in crate::share::ACTIONS.iter().enumerate().take(5) {
                let r = Rect::new(rx, y + k as f32 * 46.0, rw, 38.0);
                if self.ui.button(r, label) {
                    self.share_action(k, v, &words);
                }
            }
            y += 5.0 * 46.0 + 6.0;
        } else {
            crate::recruit::wrap("Save the video first, then share it from here or from the save screen.", rx, y + 14.0, rw, 15.0, MUTED);
            y += 52.0;
        }
        // Rename.
        match &mut s.rename {
            None => {
                if self.ui.button(Rect::new(rx, y, rw, 38.0), "Rename") {
                    s.rename = Some(m.caption().to_string());
                }
            }
            Some(name_now) => {
                for c in crate::record::chars() {
                    if !c.is_control() && name_now.chars().count() < 60 {
                        name_now.push(c);
                    }
                }
                if crate::record::backspace() {
                    name_now.pop();
                }
                let r = Rect::new(440.0, 100.0, 800.0, 40.0);
                draw_rectangle(r.x - 4.0, r.y - 4.0, r.w + 8.0, r.h + 8.0, BG);
                draw_rectangle(r.x, r.y, r.w, r.h, PANEL_HI);
                draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
                let caret = if (get_time() * 2.0) as i64 % 2 == 0 { "|" } else { "" };
                text(&format!("{name} {name_now}{caret}"), r.x + 10.0, r.y + 28.0, 22.0, TEXT);
                text("Type a new name, Enter to keep it, Esc to cancel. Empty goes back to the original.", 440.0, 160.0, 15.0, MUTED);
                let done = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) || self.ui.button(Rect::new(rx, y, rw, 38.0), "Keep name");
                if done {
                    let new = name_now.trim().to_string();
                    if let Some(mm) = self.troops[i].moments.iter_mut().find(|x| x.id == m.id) {
                        mm.title = (!new.is_empty() && new != mm.text).then_some(new);
                    }
                    self.store.save_troop(&self.troops[i]);
                    s.rename = None;
                } else if is_key_pressed(KeyCode::Escape) {
                    s.rename = None;
                }
            }
        }
        Screen::Moments(s)
    }
}
