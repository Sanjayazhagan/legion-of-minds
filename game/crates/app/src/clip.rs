//! Shareable videos made in one click: a battle clip, or a "before vs after
//! training" reel of one troop.
//!
//! Battles are deterministic, so a clip never records the screen: it rebuilds
//! the battle from scratch and plays it again into an off-screen picture at a
//! fixed size (1080x1920 tall for Shorts, Reels and TikTok, or 1920x1080
//! wide), on a frame-locked 30 fps clock. The video looks the same however
//! fast the computer is and whatever size the window has. `video.rs` turns
//! the frames into an .mp4.

use crate::art::Figure;
use crate::barracks::fmt_steps;
use crate::render::{draw_ground, draw_world, Effects, Look, View};
use crate::ui::*;
use crate::video::{Frame, VideoWriter, FPS};
use crate::{Game, Screen};
use godcore::battle::Battle;
use godcore::classes::Role;
use godcore::scenario::{BASES, DEPLOY_RADIUS};
use godcore::sim::{Outcome, Team, ARENA};
use godcore::troop::Troop;
use macroquad::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;

/// Battle seconds a clip lasts at most, before the result card. Short
/// enough for YouTube Shorts (60 s) with room to spare.
const MAX_ACTION_SECS: f32 = 36.0;
/// Slowest playback: a bit quicker than watching in the game (6 ticks/s).
const MIN_TPS: f32 = 9.0;
const INTRO_SECS: f32 = 0.8;
const RESULT_SECS: f32 = 2.2;
const END_CARD_SECS: f32 = 2.0;
/// In a reel, how long a battle keeps going after the other one has ended.
const CUT_SECS: f32 = 1.5;
pub const PITCH: &str = "You don't control your troops. You train their brains.";

#[derive(Clone, Copy, PartialEq)]
pub enum Shape {
    /// 9:16, for Shorts, Reels and TikTok.
    Tall,
    /// 16:9, for YouTube and Reddit.
    Wide,
}

impl Shape {
    /// Layout canvas size; the video is twice this in pixels.
    fn canvas(self) -> (f32, f32) {
        match self {
            Shape::Tall => (540.0, 960.0),
            Shape::Wide => (960.0, 540.0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::Tall => "Tall (Shorts, Reels, TikTok)",
            Shape::Wide => "Wide (YouTube, Reddit)",
        }
    }
}

const PX: f32 = 2.0;

/// One battle shown in the clip.
struct Lane {
    battle: Battle,
    fx: Effects,
    acc: f32,
    /// Shown over the map ("0 training steps").
    label: Option<(String, Color)>,
    defending: bool,
    /// Clip frame the battle ended on.
    ended: Option<u64>,
}

impl Lane {
    fn new(battle: Battle, defending: bool, label: Option<(String, Color)>) -> Lane {
        Lane { battle, fx: Effects::new(), acc: 0.0, label, defending, ended: None }
    }

    fn won(&self) -> bool {
        let stars = self.battle.world.stars();
        if self.defending {
            stars == 0
        } else {
            stars > 0
        }
    }

    fn result_title(&self) -> (&'static str, bool) {
        let stars = self.battle.world.stars();
        if self.defending {
            match self.battle.outcome() {
                Outcome::Victory => ("BASE FALLEN", false),
                _ if stars > 0 => ("RAIDERS SCORED", false),
                _ if self.battle.world.destruction() < 0.01 => ("FLAWLESS DEFENCE", true),
                _ => ("RAID REPELLED", true),
            }
        } else {
            match self.battle.outcome() {
                Outcome::Victory => ("TOTAL VICTORY", true),
                _ if stars > 0 => (["", "1 STAR", "2 STARS", "3 STARS"][stars.min(3) as usize], true),
                _ => ("DEFEAT", false),
            }
        }
    }
}

enum Kind {
    /// A battle the player just fought.
    Battle { title: String, sub: String },
    /// One troop with a blank brain (top) and its trained brain (bottom).
    Reel { name: String, class: String },
}

pub struct ClipState {
    kind: Kind,
    lanes: Vec<Lane>,
    pub shape: Shape,
    tps: f32,
    /// Sim ticks the longest battle lasts.
    ticks: u32,
    frame: u64,
    rt: Option<RenderTarget>,
    writer: Option<VideoWriter>,
    saving: Option<std::thread::JoinHandle<Result<(), String>>>,
    pub out: PathBuf,
    done: Option<Result<PathBuf, String>>,
    zoom: f32,
    /// The screen to go back to.
    back: Option<Box<Screen>>,
    remake: Vec<Rc<dyn Fn() -> Battle>>,
    started: bool,
}

/// How a unit in battle `b` is drawn (the player's troops in class colours).
fn look_in(b: &Battle, defending: bool, u: &godcore::sim::Unit) -> Look {
    if u.tag < b.troops.len() {
        let mut l = Game::look_for(&b.troops[u.tag], true);
        l.hostile = defending && u.team == Team::Attack;
        l
    } else {
        Look { fig: Figure::raider(), name: None, hostile: true }
    }
}

impl ClipState {
    /// A clip of a battle: `remake` rebuilds it from the start.
    pub fn battle(title: String, defending: bool, remake: Rc<dyn Fn() -> Battle>, back: Screen) -> ClipState {
        let b = remake();
        let total: u64 = b.troops.iter().filter(|t| (t.spec.role == Role::Defend) == defending).map(|t| t.steps).sum();
        let n = b.troops.iter().filter(|t| (t.spec.role == Role::Defend) == defending).count();
        let sub = if total > 0 {
            if n == 1 {
                format!("1 troop, {} training steps", fmt_steps(total))
            } else {
                format!("{n} troops, {} training steps in all", fmt_steps(total))
            }
        } else {
            String::new()
        };
        let lanes = vec![Lane::new(b, defending, None)];
        ClipState::new(Kind::Battle { title, sub }, lanes, vec![remake], back)
    }

    /// Before vs after: the same troop with a blank brain and with its own,
    /// raiding the same base from the same spot. Only the brain differs.
    pub fn reel(t: &Troop, back: Screen) -> ClipState {
        let mut blank = t.clone();
        let mut rng = godcore::rng::Rng::new(t.seed ^ 0x0b1a_4e);
        blank.actor = godcore::nn::Mlp::new(&t.actor.dims, 0.01, &mut rng);
        blank.critic = godcore::nn::Mlp::new(&t.critic.dims, 1.0, &mut rng);
        blank.steps = 0;
        let (base, spot) = best_stage(t);
        let make = |troop: Troop| -> Rc<dyn Fn() -> Battle> { Rc::new(move || Battle::new(&BASES[base], vec![(troop.clone(), spot)])) };
        let remake = vec![make(blank), make(t.clone())];
        let lanes = vec![
            Lane::new(remake[0](), false, Some(("0 training steps".into(), BAD))),
            Lane::new(remake[1](), false, Some((format!("{} training steps", fmt_steps(t.steps)), GOOD))),
        ];
        let kind = Kind::Reel { name: t.name.clone(), class: t.spec.name.clone() };
        ClipState::new(kind, lanes, remake, back)
    }

    fn new(kind: Kind, lanes: Vec<Lane>, remake: Vec<Rc<dyn Fn() -> Battle>>, back: Screen) -> ClipState {
        // Play fast enough that the battle fits. In a reel the clip follows
        // the shortest battle (the trained troop's, usually): the other one
        // is cut a moment after it, so nobody watches a blank brain idle.
        let lens = remake.iter().map(|m| {
            let mut b = m();
            b.run();
            b.world.tick
        });
        let ticks = if remake.len() > 1 { lens.min() } else { lens.max() }.unwrap_or(1);
        let tps = (ticks as f32 / MAX_ACTION_SECS).max(MIN_TPS);
        ClipState {
            kind,
            lanes,
            shape: Shape::Tall,
            tps,
            ticks,
            frame: 0,
            rt: None,
            writer: None,
            saving: None,
            out: PathBuf::new(),
            done: None,
            zoom: 0.8,
            back: Some(Box::new(back)),
            remake,
            started: false,
        }
    }

    /// The finished file, or why it failed.
    pub fn result(&self) -> Option<&Result<PathBuf, String>> {
        self.done.as_ref()
    }

    fn secs(&self) -> f32 {
        self.frame as f32 / FPS as f32
    }

    /// Starts writing the video file.
    fn begin(&mut self, out: PathBuf) -> Result<(), String> {
        let (cw, ch) = self.shape.canvas();
        let (w, h) = ((cw * PX) as u32, (ch * PX) as u32);
        let rt = render_target_ex(w, h, RenderTargetParams { sample_count: 4, depth: true });
        rt.texture.set_filter(FilterMode::Linear);
        self.rt = Some(rt);
        self.writer = Some(VideoWriter::start(out.clone(), w, h)?);
        self.out = out;
        self.zoom = crate::render::zoom();
        // Rebuild the battles: the clip always starts from the first tick.
        for (l, m) in self.lanes.iter_mut().zip(&self.remake) {
            *l = Lane::new(m(), l.defending, l.label.take());
        }
        crate::sfx::clear_log();
        self.frame = 0;
        self.started = true;
        Ok(())
    }

    /// Plays one frame of the clip into the off-screen picture and hands it
    /// to the encoder. Returns false once the clip is over.
    fn render_frame(&mut self) -> bool {
        crate::record::set_clip_clock(Some(self.frame));
        let dt = 1.0 / FPS as f32;
        let t = self.secs();
        let first_end = self.lanes.iter().filter_map(|l| l.ended).min();
        let cut = first_end.is_some_and(|e| self.frame >= e + (CUT_SECS * FPS as f32) as u64);
        if t >= INTRO_SECS {
            for l in &mut self.lanes {
                if l.ended.is_some() {
                    continue;
                }
                if cut {
                    // Still running when the other battle ended a while ago.
                    l.ended = Some(self.frame);
                    crate::sfx::play_ui(crate::sfx::Sfx::Defeat);
                    continue;
                }
                l.acc += dt * self.tps;
                while l.acc >= 1.0 && l.battle.outcome() == Outcome::Running {
                    l.acc -= 1.0;
                    l.battle.step();
                    l.fx.absorb(&l.battle.world);
                }
                if l.battle.outcome() != Outcome::Running {
                    l.ended = Some(self.frame);
                    crate::sfx::play_ui(if l.won() { crate::sfx::Sfx::Victory } else { crate::sfx::Sfx::Defeat });
                }
            }
        }
        for l in &mut self.lanes {
            l.fx.age(dt);
        }
        let all_done = self.lanes.iter().all(|l| l.ended.is_some());
        let end_at = self.lanes.iter().filter_map(|l| l.ended).max().unwrap_or(0);
        let since_end = if all_done { (self.frame - end_at) as f32 * dt } else { 0.0 };

        let rt = self.rt.clone().unwrap();
        let (cw, ch) = self.shape.canvas();
        // Off-screen pictures are stored the other way up from the window,
        // so unlike the UI camera this one has a positive height. Rows read
        // back come bottom row first, like a screenshot.
        let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, cw, ch));
        cam.render_target = Some(rt.clone());
        cam.viewport = Some((0, 0, (cw * PX) as i32, (ch * PX) as i32));
        crate::render::set_target(Some(rt.clone()));
        set_camera(&cam);
        crate::render::set_screen(0.0, 0.0, PX, &cam);
        clear_background(BG);
        let _guard = TextScale::set(PX);
        self.draw(since_end);
        unsafe { get_internal_gl() }.flush();
        let img = rt.texture.get_texture_data();
        // Test hook: BAG_CLIP_PNG=prefix saves every 60th frame as a picture.
        if let Some(p) = std::env::var_os("BAG_CLIP_PNG") {
            if self.frame % 60 == 0 {
                img.export_png(&format!("{}{:05}.png", p.to_string_lossy(), self.frame));
            }
        }
        if let Some(w) = &mut self.writer {
            w.push(Frame(img.bytes));
        }
        crate::render::set_target(None);
        crate::record::set_clip_clock(None);
        crate::render::set_zoom(self.zoom);
        self.frame += 1;
        if self.frame % 150 == 0 && std::env::var_os("BAG_CLIP_OUT").is_some() {
            eprintln!("clip: frame {} of about {}", self.frame, self.estimate_frames());
        }
        !(all_done && since_end >= RESULT_SECS + END_CARD_SECS)
    }

    /// Lays out one frame on the clip canvas.
    fn draw(&self, since_end: f32) {
        let (cw, ch) = self.shape.canvas();
        let tall = self.shape == Shape::Tall;
        match &self.kind {
            Kind::Battle { title, sub } => {
                let l = &self.lanes[0];
                let (map, hud) = if tall {
                    (Rect::new(0.0, 150.0, cw, 620.0), Rect::new(0.0, 770.0, cw, 190.0))
                } else {
                    (Rect::new(0.0, 0.0, 600.0, ch), Rect::new(600.0, 0.0, cw - 600.0, ch))
                };
                draw_lane(l, map);
                // Title.
                if tall {
                    ctext("LEGION OF MINDS", cw / 2.0, 42.0, 22.0, ACCENT);
                    fit_text(title, cw / 2.0, 92.0, 38.0, cw - 30.0, TEXT);
                    ctext(sub, cw / 2.0, 128.0, 20.0, MUTED);
                } else {
                    let x = hud.x + hud.w / 2.0;
                    ctext("LEGION OF MINDS", x, 44.0, 22.0, ACCENT);
                    fit_text(title, x, 90.0, 32.0, hud.w - 30.0, TEXT);
                    ctext(sub, x, 120.0, 18.0, MUTED);
                }
                let stats = if tall { hud } else { Rect::new(hud.x, 140.0, hud.w, hud.h - 140.0) };
                draw_stats(l, stats, tall);
                if since_end > 0.0 {
                    let (t, good) = l.result_title();
                    result_card(map, t, good, l.battle.world.stars(), l.defending, since_end);
                }
            }
            Kind::Reel { name, class } => {
                let (a, b) = if tall {
                    (Rect::new(0.0, 130.0, cw, 405.0), Rect::new(0.0, 545.0, cw, 405.0))
                } else {
                    (Rect::new(0.0, 90.0, 475.0, 450.0), Rect::new(485.0, 90.0, 475.0, 450.0))
                };
                if tall {
                    ctext("SAME TROOP. SAME RAID.", cw / 2.0, 46.0, 34.0, TEXT);
                    ctext("Only the brain is different.", cw / 2.0, 82.0, 24.0, ACCENT);
                    ctext(&format!("{name} the {class}  ·  {}", "Legion of Minds"), cw / 2.0, 114.0, 18.0, MUTED);
                } else {
                    ctext("SAME TROOP. SAME RAID. ONLY THE BRAIN IS DIFFERENT.", cw / 2.0, 40.0, 28.0, TEXT);
                    ctext(&format!("{name} the {class}  ·  Legion of Minds"), cw / 2.0, 72.0, 18.0, MUTED);
                }
                for (l, r) in self.lanes.iter().zip([a, b]) {
                    draw_lane(l, r);
                    if let Some((label, col)) = &l.label {
                        pill(label, r.x + 12.0, r.y + 12.0, 24.0, *col);
                    }
                    let d = l.battle.world.destruction();
                    let line = format!("{:.0}% destroyed", d * 100.0);
                    let tw = twidth(&line, 22.0);
                    pill(&line, r.x + r.w - tw - 36.0, r.y + 12.0, 22.0, Color::new(0.1, 0.1, 0.14, 1.0));
                    if let Some(e) = l.ended {
                        let k = ((self.frame - e) as f32 / FPS as f32 * 4.0).min(1.0);
                        let (t, good) = l.result_title();
                        stamp(r, t, good, l.battle.world.stars(), k);
                    }
                }
            }
        }
        if since_end > RESULT_SECS {
            end_card(cw, ch, ((since_end - RESULT_SECS) * 3.0).min(1.0));
        }
        // A short fade in from black.
        let fade = 1.0 - (self.secs() * 4.0).min(1.0);
        if fade > 0.0 {
            draw_rectangle(0.0, 0.0, cw, ch, Color::new(0.0, 0.0, 0.0, fade));
        }
    }
}

fn draw_lane(l: &Lane, r: Rect) {
    // Pull the camera back on narrow maps so the whole arena fits across.
    let aspect = r.w / r.h;
    crate::render::set_zoom(if aspect < 1.0 { 0.84 / aspect } else { 0.7 });
    let v = View::new(r);
    draw_ground(v, false, false, &l.battle.world);
    let b = &l.battle;
    let defending = l.defending;
    draw_world(v, &b.world, &l.fx, &|_, u| look_in(b, defending, u));
}

/// Destruction, stars, time and the squad's health.
fn draw_stats(l: &Lane, r: Rect, tall: bool) {
    let w = &l.battle.world;
    let pad = 22.0;
    let (x, mut y) = (r.x + pad, r.y + 34.0);
    let d = w.destruction();
    let label = if l.defending { "of base lost" } else { "destroyed" };
    tleft(&format!("{:.0}% {label}", d * 100.0), x, y, 28.0, ACCENT);
    for k in 0..3 {
        star(r.x + r.w - pad - 22.0 - (2 - k) as f32 * 44.0, y - 10.0, 18.0, k < w.stars() as usize);
    }
    y += 12.0;
    bar(Rect::new(x, y, r.w - 2.0 * pad, 12.0), d, ACCENT);
    y += 36.0;
    let mine: Vec<(usize, &godcore::sim::Unit)> = w
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == godcore::sim::Kind::Troop && u.tag < l.battle.troops.len())
        .collect();
    let n = mine.len().max(1);
    if tall {
        let cell = (r.w - 2.0 * pad) / n as f32;
        for (k, (_, u)) in mine.iter().enumerate() {
            let t = &l.battle.troops[u.tag];
            let cx = x + cell * (k as f32 + 0.5);
            if u.alive {
                crate::art::draw_portrait(vec2(cx, y + 18.0), 18.0, &t.spec);
            } else {
                draw_circle(cx, y + 18.0, 18.0, Color::new(0.3, 0.3, 0.3, 1.0));
            }
            fit_text(&t.name, cx, y + 58.0, 18.0, cell - 6.0, if u.alive { TEXT } else { MUTED });
            bar(Rect::new(cx - cell / 2.0 + 8.0, y + 66.0, cell - 16.0, 7.0), u.hp / u.max_hp, GOOD);
        }
        ctext(PITCH, r.x + r.w / 2.0, r.y + r.h - 14.0, 16.0, MUTED);
    } else {
        for (_, u) in mine.iter().take(6) {
            let t = &l.battle.troops[u.tag];
            if u.alive {
                crate::art::draw_portrait(vec2(x + 18.0, y + 18.0), 16.0, &t.spec);
            } else {
                draw_circle(x + 18.0, y + 18.0, 16.0, Color::new(0.3, 0.3, 0.3, 1.0));
            }
            tleft(&t.name, x + 44.0, y + 16.0, 20.0, if u.alive { TEXT } else { MUTED });
            bar(Rect::new(x + 44.0, y + 24.0, r.w - 2.0 * pad - 44.0, 7.0), u.hp / u.max_hp, GOOD);
            y += 50.0;
        }
        ctext(PITCH, r.x + r.w / 2.0, r.y + r.h - 18.0, 14.0, MUTED);
    }
}

/// The big result over the map once a battle ends.
fn result_card(map: Rect, title: &str, good: bool, stars: u32, defending: bool, t: f32) {
    let k = (t * 4.0).min(1.0);
    let pop = 1.0 + (1.0 - k) * 0.4;
    let cy = map.y + map.h * 0.42;
    draw_rectangle(map.x, cy - 80.0, map.w, 160.0, Color::new(0.03, 0.03, 0.06, 0.78 * k));
    ctext(title, map.x + map.w / 2.0, cy - 10.0, 52.0 * pop, if good { ACCENT } else { BAD });
    if !defending {
        for s in 0..3 {
            let appear = ((t - 0.25 - s as f32 * 0.22) * 5.0).clamp(0.0, 1.0);
            star(map.x + map.w / 2.0 - 60.0 + s as f32 * 60.0, cy + 42.0, 22.0 * (0.5 + appear * 0.5), s < stars as usize && appear > 0.0);
        }
    }
}

/// A result stamp on one half of a reel.
fn stamp(r: Rect, title: &str, good: bool, stars: u32, k: f32) {
    let cy = r.y + r.h * 0.55;
    draw_rectangle(r.x, cy - 46.0, r.w, 92.0, Color::new(0.03, 0.03, 0.06, 0.72 * k));
    ctext(title, r.x + r.w / 2.0, cy + 2.0, 40.0 * (1.0 + (1.0 - k) * 0.4), if good { ACCENT } else { BAD });
    for s in 0..3 {
        star(r.x + r.w / 2.0 - 40.0 + s as f32 * 40.0, cy + 28.0, 13.0, s < stars as usize);
    }
}

/// The closing card: name and pitch.
fn end_card(cw: f32, ch: f32, k: f32) {
    draw_rectangle(0.0, 0.0, cw, ch, Color::new(0.04, 0.045, 0.07, 0.94 * k));
    let c = Color::new(ACCENT.r, ACCENT.g, ACCENT.b, k);
    let tx = Color::new(TEXT.r, TEXT.g, TEXT.b, k);
    ctext("LEGION OF MINDS", cw / 2.0, ch / 2.0 - 20.0, 54.0, c);
    fit_text(PITCH, cw / 2.0, ch / 2.0 + 28.0, 24.0, cw - 40.0, tx);
    ctext("Train your own army on your PC", cw / 2.0, ch / 2.0 + 70.0, 20.0, Color::new(MUTED.r, MUTED.g, MUTED.b, k));
}

fn pill(s: &str, x: f32, y: f32, size: f32, bg: Color) {
    let w = twidth(s, size) + 24.0;
    draw_rectangle(x, y, w, size + 14.0, Color::new(bg.r, bg.g, bg.b, 0.92));
    tleft(s, x + 12.0, y + size + 2.0, size, WHITE);
}

// Text drawn at the video's real size, so it stays sharp (the game's own
// text helpers draw at canvas size and get stretched).
thread_local! {
    static TSCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

struct TextScale(f32);

impl TextScale {
    fn set(s: f32) -> TextScale {
        let old = TSCALE.with(|c| c.replace(s));
        TextScale(old)
    }
}

impl Drop for TextScale {
    fn drop(&mut self) {
        TSCALE.with(|c| c.set(self.0));
    }
}

fn params(size: f32, c: Color) -> TextParams<'static> {
    let s = TSCALE.with(|c| c.get());
    TextParams { font_size: (size * s).round() as u16, font_scale: 1.0 / s, color: c, ..Default::default() }
}

fn tleft(s: &str, x: f32, y: f32, size: f32, c: Color) {
    draw_text_ex(s, x, y, params(size, c));
}

fn twidth(s: &str, size: f32) -> f32 {
    let p = params(size, WHITE);
    measure_text(s, None, p.font_size, p.font_scale).width
}

fn ctext(s: &str, cx: f32, y: f32, size: f32, c: Color) {
    tleft(s, cx - twidth(s, size) / 2.0, y, size, c);
}

/// Centred text, shrunk to fit `max_w`.
fn fit_text(s: &str, cx: f32, y: f32, size: f32, max_w: f32, c: Color) {
    let w = twidth(s, size);
    let size = if w > max_w { size * max_w / w } else { size };
    ctext(s, cx, y, size, c);
}

/// The base and starting spot where `t` does best (most stars, then most
/// destruction), so the reel shows what its training is good for.
fn best_stage(t: &Troop) -> (usize, (f32, f32)) {
    let c = ARENA / 2.0;
    let r = DEPLOY_RADIUS + 1.0;
    // Only spots on the camera's side of the base, which stay in frame.
    let spots: Vec<(f32, f32)> = [230.0f32, 250.0, 270.0, 290.0, 310.0]
        .iter()
        .map(|deg| {
            let a = deg.to_radians();
            ((c + r * a.cos()).clamp(1.0, ARENA - 1.0), (c + r * a.sin()).clamp(1.0, ARENA - 1.0))
        })
        .filter(|&(x, y)| godcore::scenario::can_deploy(x, y))
        .collect();
    let mut best = (0, spots.first().copied().unwrap_or((4.0, 4.0)), (0u32, 0.0f32));
    for b in 0..BASES.len() {
        for &s in &spots {
            let mut battle = Battle::new(&BASES[b], vec![(t.clone(), s)]);
            battle.run();
            let score = (battle.world.stars(), battle.world.destruction());
            // Harder bases come later in the list: ties go to them.
            if score.0 > best.2 .0 || (score.0 == best.2 .0 && score.1 >= best.2 .1) {
                best = (b, s, score);
            }
        }
    }
    (best.0, best.1)
}

/// Where clips are saved: the Videos folder, under "Legion of Minds".
pub fn clip_dir(g: &Game) -> PathBuf {
    dirs::video_dir().unwrap_or_else(|| g.store.home.join("clips")).join("Legion of Minds")
}

/// `<dir>/<stem> 1.mp4`, or the first number not taken.
fn free_name(dir: &std::path::Path, stem: &str) -> PathBuf {
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { ' ' }).collect();
    let stem = stem.split_whitespace().collect::<Vec<_>>().join(" ");
    (1..)
        .map(|n| dir.join(format!("{stem} {n}.mp4")))
        .find(|p| !p.exists())
        .unwrap()
}

impl Game {
    pub fn clip(&mut self, mut s: ClipState) -> Screen {
        self.header("Save a clip", false);
        let (pw, ph) = match s.shape {
            Shape::Tall => (300.0, 533.0),
            Shape::Wide => (640.0, 360.0),
        };
        let preview = Rect::new(40.0, 120.0, pw, ph);

        if !s.started {
            // Pick a shape, then go.
            text("Pick a shape, then press Save. The battle is replayed into a video file.", 40.0, 96.0, 20.0, TEXT);
            let mut y = 140.0;
            for shape in [Shape::Tall, Shape::Wide] {
                if self.ui.button_ex(Rect::new(40.0, y, 380.0, 50.0), shape.name(), true, s.shape == shape) {
                    s.shape = shape;
                }
                y += 62.0;
            }
            let dir = clip_dir(self);
            text(&format!("Saved in {}", dir.display()), 40.0, y + 20.0, 16.0, MUTED);
            if self.ui.button(Rect::new(40.0, y + 44.0, 180.0, 50.0), "Save clip") || std::env::var_os("BAG_CLIP_OUT").is_some() {
                let stem = match &s.kind {
                    Kind::Battle { title, .. } => title.clone(),
                    Kind::Reel { name, .. } => format!("{name} before and after"),
                };
                let out = match std::env::var_os("BAG_CLIP_OUT") {
                    Some(p) => PathBuf::from(p),
                    None => {
                        let _ = std::fs::create_dir_all(&dir);
                        free_name(&dir, &stem)
                    }
                };
                if std::env::var("BAG_CLIP_SHAPE").as_deref() == Ok("wide") {
                    s.shape = Shape::Wide;
                }
                if let Err(e) = s.begin(out) {
                    s.done = Some(Err(e));
                    s.started = true;
                }
            }
            if self.ui.button(Rect::new(230.0, y + 44.0, 150.0, 50.0), "Back") {
                return *s.back.take().unwrap();
            }
            return Screen::Clip(s);
        }

        if s.writer.is_some() {
            // A few clip frames per window frame keeps saving quick.
            let t0 = std::time::Instant::now();
            loop {
                if !s.render_frame() {
                    let seconds = s.frame as f64 / FPS as f64;
                    let sound = crate::sfx::soundtrack(seconds);
                    let w = s.writer.take().unwrap();
                    let video = w.finish();
                    let out = s.out.clone();
                    s.saving = Some(std::thread::spawn(move || {
                        video.join().map_err(|_| "the video encoder stopped".to_string())??;
                        if let Some(sound) = sound {
                            add_sound(&out, &sound);
                        }
                        Ok(())
                    }));
                    break;
                }
                if t0.elapsed().as_secs_f32() > 1.0 / 40.0 {
                    break;
                }
            }
            // Back to drawing the window.
            self.ui.begin();
            self.header("Save a clip", false);
        }
        if let Some(h) = &s.saving {
            if h.is_finished() {
                let h = s.saving.take().unwrap();
                s.done = Some(match h.join() {
                    Ok(Ok(())) => Ok(s.out.clone()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("saving stopped".into()),
                });
            }
        }

        if let Some(rt) = &s.rt {
            draw_texture_ex(&rt.texture, preview.x, preview.y, WHITE, DrawTextureParams { dest_size: Some(vec2(preview.w, preview.h)), flip_y: true, ..Default::default() });
            draw_rectangle_lines(preview.x, preview.y, preview.w, preview.h, 2.0, LINE);
        }
        let x = preview.x + preview.w + 40.0;
        match &s.done {
            None => {
                let est = s.estimate_frames();
                text("Saving your clip...", x, 150.0, 30.0, TEXT);
                bar(Rect::new(x, 172.0, 400.0, 14.0), s.frame as f32 / est.max(1) as f32, ACCENT);
                text(if s.saving.is_some() { "Finishing the file" } else { "Replaying the battle into the video" }, x, 214.0, 18.0, MUTED);
                if s.saving.is_none() && self.ui.button(Rect::new(x, 250.0, 150.0, 46.0), "Cancel") {
                    // Dropping the writer stops the encoder; nothing is kept.
                    s.writer = None;
                    let _ = std::fs::remove_file(s.out.with_extension("part"));
                    return *s.back.take().unwrap();
                }
            }
            Some(Ok(p)) => {
                text("Clip saved!", x, 150.0, 34.0, GOOD);
                text(&p.file_name().unwrap_or_default().to_string_lossy(), x, 186.0, 20.0, TEXT);
                text(&format!("in {}", p.parent().map(|d| d.display().to_string()).unwrap_or_default()), x, 212.0, 16.0, MUTED);
                text("Post it to YouTube Shorts, Reels, TikTok or Reddit.", x, 250.0, 18.0, MUTED);
                if self.ui.button(Rect::new(x, 280.0, 200.0, 46.0), "Show in folder") {
                    show_in_folder(p);
                }
                if self.ui.button(Rect::new(x + 210.0, 280.0, 150.0, 46.0), "Done") {
                    return *s.back.take().unwrap();
                }
            }
            Some(Err(e)) => {
                text("Could not save the clip", x, 150.0, 30.0, BAD);
                text(e, x, 186.0, 18.0, MUTED);
                if self.ui.button(Rect::new(x, 220.0, 150.0, 46.0), "Back") {
                    return *s.back.take().unwrap();
                }
            }
        }
        Screen::Clip(s)
    }
}

impl ClipState {
    /// Frames the finished clip will have (for the progress bar).
    fn estimate_frames(&self) -> u64 {
        let cut = if self.lanes.len() > 1 { CUT_SECS } else { 0.0 };
        ((INTRO_SECS + self.ticks as f32 / self.tps + cut + RESULT_SECS + END_CARD_SECS) * FPS as f32) as u64
    }
}

/// Adds the battle's sounds to the video, when ffmpeg is installed (the
/// video is complete without it, just silent).
fn add_sound(out: &std::path::Path, sound: &[f32]) {
    let wav = out.with_extension("sound.wav");
    if crate::sfx::write_wav(&wav, sound).is_err() {
        return;
    }
    let tmp = out.with_extension("tmp.mp4");
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(out).arg("-i").arg(&wav);
    cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest", "-movflags", "+faststart"]).arg(&tmp);
    no_window(&mut cmd);
    if cmd.status().is_ok_and(|s| s.success()) {
        let _ = std::fs::rename(&tmp, out);
    }
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&wav);
}

fn no_window(_cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up.
        _cmd.creation_flags(0x0800_0000);
    }
}

fn show_in_folder(p: &std::path::Path) {
    #[cfg(windows)]
    let r = {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", p.display()));
        c.spawn()
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p.parent().unwrap_or(p)).spawn();
    if let Err(e) = r {
        eprintln!("could not open the folder: {e}");
    }
}
