//! Drawing a `World` and its effects: a 3D scene seen from an angled camera,
//! with 2D overlays (names, health bars, rings) on top. The sim knows nothing
//! about graphics; this only reads its state.

use crate::art::{self, Figure, Pose};
use crate::scene::{library, Library, Model};
use crate::ui::*;
use godcore::scenario::BuildKind;
use godcore::scenario::DEPLOY_RADIUS;
use godcore::sim::{EventKind, Kind, Team, Unit, World, ARENA};
use godcore::weapons::{Skill, Weapon};
use macroquad::models::{draw_mesh, Mesh, Vertex};
use macroquad::prelude::*;
use std::cell::Cell;

const C: f32 = ARENA / 2.0;

pub struct Effects {
    items: Vec<(godcore::sim::Event, f32)>,
    anim: Vec<Anim>,
    /// When the last sim tick arrived, and the usual time between ticks.
    /// Units are drawn sliding between their last two tick positions, so
    /// they move smoothly at any frame rate instead of hopping 20 times a
    /// second.
    last_tick: f64,
    tick_gap: f64,
    /// Per unit: the clip drawn last frame, to fade into the next one.
    blends: std::cell::RefCell<Vec<Blend>>,
}

/// The clip a character showed last frame and the one it is fading out of.
#[derive(Clone, Copy, Default)]
struct Blend {
    now: Option<(&'static str, f32, bool)>,
    from: Option<(&'static str, f32, bool)>,
    at: f64,
}

/// Seconds a character takes to fade from one animation into the next.
const BLEND_TIME: f64 = 0.14;
/// Attacks play this much faster than the clip's own speed.
const ATTACK_RATE: f32 = 1.25;
/// The sim hits many times a second; one swing covers several hits, and a
/// swing that has started is finished even if the fight ends.
const FIGHT_LINGER: f64 = 0.3;
/// Ground covered per second of each clip, so feet keep pace with the ground.
const RUN_STRIDE: f32 = 2.0;
const WALK_STRIDE: f32 = 1.3;

/// Per-unit animation memory, updated once per sim tick.
#[derive(Clone, Copy, Default)]
struct Anim {
    x: f32,
    y: f32,
    /// Position and facing one tick earlier.
    px: f32,
    py: f32,
    pheading: f32,
    /// Facing, radians around the vertical axis (0 = towards the camera).
    heading: f32,
    target_heading: f32,
    /// Seconds since the unit last moved.
    still: f32,
    /// When its current attack or cast started (real time).
    acted: f64,
    died: Option<f64>,
    /// 2D fallback art.
    face: f32,
    phase: f32,
    /// A dodge (1) or weapon skill (2) started at `acted`.
    special: u8,
    skill: Option<Skill>,
    /// The current fight: when its first swing started and the latest hit.
    fight_start: f64,
    last_hit: f64,
    /// Who or what it is hitting.
    aim: (f32, f32),
    /// When it was last hit, and which way the blow pushed it.
    hurt: f64,
    push: (f32, f32),
    /// A roll or charge moves several metres in one tick; it is drawn as a
    /// quick dash from here instead of a jump.
    slide_from: (f32, f32),
    slide_at: f64,
    slide_dur: f32,
    /// This tick it teleported (blink, shadowstep): draw it at the new spot.
    jumped: bool,
    /// Ground speed (per tick, smoothed) and distance covered, for the legs.
    gait: f32,
    dist: f32,
    pdist: f32,
}

fn heading_of(dx: f32, dy: f32) -> f32 {
    // Sim +y is away from the camera (-Z in 3D); models face +Z.
    dx.atan2(-dy)
}

impl Effects {
    pub fn new() -> Self {
        Effects { items: Vec::new(), anim: Vec::new(), last_tick: 0.0, tick_gap: 0.05, blends: Default::default() }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.anim.clear();
        self.blends.borrow_mut().clear();
    }

    /// Call once per sim tick.
    pub fn absorb(&mut self, w: &World) {
        let now = crate::record::now();
        let gap = now - self.last_tick;
        if gap > 1e-4 {
            // Several ticks in one frame (fast forward) leave the gap as it was.
            self.tick_gap = if gap > 0.25 { 0.05 } else { self.tick_gap * 0.7 + gap * 0.3 };
        }
        self.last_tick = now;
        for e in &w.events {
            self.items.push((*e, 0.0));
        }
        if self.anim.len() > w.units.len() {
            // A new battle: start over.
            self.anim.clear();
            self.blends.borrow_mut().clear();
        }
        if self.anim.len() < w.units.len() {
            // Units joining mid-battle (summons, waves) keep everyone else's state.
            let have = self.anim.len();
            let more: Vec<Anim> = w.units[have..]
                .iter()
                .map(|u| {
                    let h = heading_of(C - u.x, C - u.y);
                    Anim {
                        x: u.x,
                        y: u.y,
                        px: u.x,
                        py: u.y,
                        pheading: h,
                        heading: h,
                        target_heading: h,
                        still: 1.0,
                        acted: -10.0,
                        died: None,
                        face: if u.team == Team::Attack { 1.0 } else { -1.0 },
                        phase: 0.0,
                        special: 0,
                        skill: None,
                        fight_start: -10.0,
                        last_hit: -10.0,
                        aim: (u.x, u.y),
                        hurt: -10.0,
                        push: (0.0, 0.0),
                        slide_from: (u.x, u.y),
                        slide_at: -10.0,
                        slide_dur: 0.0,
                        jumped: false,
                        gait: 0.0,
                        dist: 0.0,
                        pdist: 0.0,
                    }
                })
                .collect();
            self.anim.extend(more);
        }
        for a in &mut self.anim {
            a.jumped = false;
        }
        // Who got hit this tick: a flash and a small push away from the blow.
        for e in &w.events {
            if !matches!(e.kind, EventKind::Melee | EventKind::GuardHit | EventKind::Arrow | EventKind::TowerShot | EventKind::Blast) {
                continue;
            }
            if let Some(i) = w.units.iter().position(|u| !u.kind.is_building() && (u.x - e.to.0).abs() < 1e-3 && (u.y - e.to.1).abs() < 1e-3) {
                let (dx, dy) = (e.to.0 - e.from.0, e.to.1 - e.from.1);
                let d = (dx * dx + dy * dy).sqrt().max(1e-3);
                let a = &mut self.anim[i];
                a.hurt = now;
                a.push = (dx / d, dy / d);
            }
        }
        // Who acted this tick: the unit standing where the event started.
        for e in &w.events {
            // Rolls and blinks end where the unit now stands.
            let moved = matches!(e.kind, EventKind::Dodge | EventKind::Blink | EventKind::Charge | EventKind::Skill(Skill::Shadowstep));
            let at = match e.kind {
                EventKind::Death | EventKind::Stun | EventKind::Blocked | EventKind::TrapHit => continue,
                _ if moved => e.to,
                _ => e.from,
            };
            if let Some(i) = w.units.iter().position(|u| (u.x - at.0).abs() < 1e-3 && (u.y - at.1).abs() < 1e-3) {
                let a = &mut self.anim[i];
                if a.special != 0 && now - a.acted < 0.3 && !matches!(e.kind, EventKind::Dodge | EventKind::Blink | EventKind::Ward | EventKind::Skill(_)) {
                    continue; // a skill's own hits don't restart its animation
                }
                if matches!(e.kind, EventKind::Blink | EventKind::Skill(Skill::Shadowstep)) {
                    a.jumped = true;
                }
                if matches!(e.kind, EventKind::Melee | EventKind::Arrow | EventKind::GuardHit | EventKind::Heal) {
                    // Basic hits keep one swing going rather than restarting it.
                    if now - a.last_hit > FIGHT_LINGER + 0.6 {
                        // Start part-way in, so the first blow lands right away.
                        a.fight_start = now - 0.28;
                    }
                    a.last_hit = now;
                    a.aim = e.to;
                    if (e.to.0 - e.from.0).abs() + (e.to.1 - e.from.1).abs() > 1e-3 {
                        a.target_heading = heading_of(e.to.0 - e.from.0, e.to.1 - e.from.1);
                    }
                    continue;
                }
                a.acted = now;
                (a.special, a.skill) = match e.kind {
                    EventKind::Dodge | EventKind::Blink => (1, None),
                    EventKind::Skill(s) => (2, Some(s)),
                    EventKind::Ward => (2, Some(Skill::HealingNova)),
                    _ => (0, None),
                };
                if !moved && (e.to.0 - e.from.0).abs() + (e.to.1 - e.from.1).abs() > 1e-3 {
                    a.target_heading = heading_of(e.to.0 - e.from.0, e.to.1 - e.from.1);
                }
            }
        }
        for (a, u) in self.anim.iter_mut().zip(&w.units) {
            let (dx, dy) = (u.x - a.x, u.y - a.y);
            let d = (dx * dx + dy * dy).sqrt();
            let dash = d > 1.0 && !a.jumped;
            if dash {
                // Dash from where it was drawn to where it now is.
                a.slide_from = (a.x, a.y);
                a.slide_at = now;
                a.slide_dur = (d * 0.06).clamp(0.14, 0.3);
            }
            a.pdist = a.dist;
            if !a.jumped {
                a.dist += d;
            }
            a.gait = a.gait * 0.6 + if a.jumped || dash { 0.0 } else { d } * 0.4;
            if d > 0.01 {
                if dx.abs() > 0.01 {
                    a.face = dx.signum();
                }
                a.phase += d * 9.0;
                a.still = 0.0;
                if now - a.acted > 0.4 && now - a.last_hit > 0.4 && !a.jumped {
                    a.target_heading = heading_of(dx, dy);
                }
            } else {
                a.still += 0.1;
            }
            // Turn smoothly, the short way round.
            let mut dh = a.target_heading - a.heading;
            while dh > std::f32::consts::PI {
                dh -= std::f32::consts::TAU;
            }
            while dh < -std::f32::consts::PI {
                dh += std::f32::consts::TAU;
            }
            a.pheading = a.heading;
            a.heading += dh * 0.45;
            if !u.alive && a.died.is_none() {
                a.died = Some(now);
            }
            a.px = a.x;
            a.py = a.y;
            a.x = u.x;
            a.y = u.y;
        }
    }

    /// How many times faster than normal the battle is running.
    fn speedup(&self) -> f64 {
        (1.0 / crate::battle_screen::WATCH_TPS as f64 / self.tick_gap).clamp(1.0, 4.0)
    }

    /// How far (0 to 1) the picture is between the last two ticks.
    fn blend(&self) -> f32 {
        ((crate::record::now() - self.last_tick) / self.tick_gap).clamp(0.0, 1.0) as f32
    }

    /// Where to draw unit `i` this frame and which way it faces.
    fn smooth(&self, i: usize, u: &Unit) -> (f32, f32, f32) {
        let Some(a) = self.anim.get(i) else {
            return (u.x, u.y, 0.0);
        };
        let k = self.blend();
        let now = crate::record::now();
        let (dx, dy) = (a.x - a.px, a.y - a.py);
        // Blinks and shadowsteps jump; don't slide across the map.
        if a.jumped || (a.x - u.x).abs() + (a.y - u.y).abs() > 1e-3 {
            return (u.x, u.y, a.heading);
        }
        let (mut x, mut y) = (a.px + dx * k, a.py + dy * k);
        let since = (now - a.slide_at) as f32;
        if since < a.slide_dur {
            // Rolls and charges: a quick dash that slows at the end.
            let f = since / a.slide_dur;
            let e = 1.0 - (1.0 - f) * (1.0 - f);
            x = a.slide_from.0 + (a.x - a.slide_from.0) * e;
            y = a.slide_from.1 + (a.y - a.slide_from.1) * e;
        }
        let hit = (now - a.hurt) as f32;
        if hit < 0.16 && u.alive {
            // Knocked back a little by the blow, then recovers.
            let r = 0.16 * (1.0 - hit / 0.16);
            x += a.push.0 * r;
            y += a.push.1 * r;
        }
        let mut dh = a.heading - a.pheading;
        while dh > std::f32::consts::PI {
            dh -= std::f32::consts::TAU;
        }
        while dh < -std::f32::consts::PI {
            dh += std::f32::consts::TAU;
        }
        (x, y, a.pheading + dh * k)
    }

    pub fn age(&mut self, dt: f32) {
        for it in &mut self.items {
            it.1 += dt;
        }
        self.items.retain(|it| it.1 < effect_life(it.0.kind));
    }
}

// ------------------------------------------------------------- 3D view

thread_local! {
    /// The UI letterbox: (offset x, offset y, scale) and the 2D camera to
    /// restore after a 3D pass.
    static SCREEN: Cell<(f32, f32, f32)> = const { Cell::new((0.0, 0.0, 1.0)) };
    static UI_CAM: std::cell::RefCell<Option<Camera2D>> = const { std::cell::RefCell::new(None) };
    /// Map camera distance; the mouse wheel zooms.
    static ZOOM: Cell<f32> = const { Cell::new(0.8) };
}

/// Mouse wheel over the map zooms the camera in and out.
fn handle_zoom(r: Rect) {
    let (vx, vy, s) = SCREEN.with(|c| c.get());
    let (mx, my) = mouse_position();
    let m = crate::record::input().map_or(vec2((mx - vx) / s, (my - vy) / s), |i| i.0);
    let wheel = crate::record::wheel();
    if r.contains(m) && wheel != 0.0 {
        ZOOM.with(|z| z.set((z.get() - wheel.signum() * 0.06).clamp(0.55, 1.05)));
    }
}

/// Called by the UI each frame with its letterbox.
pub fn set_screen(vx: f32, vy: f32, s: f32, cam: &Camera2D) {
    SCREEN.with(|c| c.set((vx, vy, s)));
    UI_CAM.with(|c| {
        *c.borrow_mut() = Some(Camera2D {
            rotation: cam.rotation,
            zoom: cam.zoom,
            target: cam.target,
            offset: cam.offset,
            render_target: None,
            viewport: cam.viewport,
        })
    });
}

fn restore_ui_camera() {
    UI_CAM.with(|c| {
        if let Some(cam) = c.borrow().as_ref() {
            set_camera(cam);
        }
    });
}

/// Pixel viewport (GL origin bottom-left) for a rect in UI coordinates.
fn viewport_px(r: Rect) -> (i32, i32, i32, i32) {
    let (vx, vy, s) = SCREEN.with(|c| c.get());
    let (sh, d) = (screen_height(), screen_dpi_scale());
    let x = vx + r.x * s;
    let y = sh - (vy + (r.y + r.h) * s);
    // In real pixels (see Ui::begin).
    ((x * d).round() as i32, (y * d).round() as i32, (r.w * s * d).round() as i32, (r.h * s * d).round() as i32)
}

/// Start drawing 3D into `r` with `cam` (its viewport and aspect are set here).
fn begin_3d(r: Rect, mut cam: Camera3D, clear_depth: bool) {
    if cam.z_near < 0.05 {
        cam.z_near = 0.1;
        cam.z_far = 100.0;
    }
    cam.viewport = Some(viewport_px(r));
    cam.aspect = Some(r.w / r.h);
    let mut gl = unsafe { get_internal_gl() };
    gl.flush();
    if clear_depth {
        gl.quad_context.clear(None, Some(1.0), None);
    }
    set_camera(&cam);
}

fn end_3d() {
    unsafe { get_internal_gl() }.flush();
    restore_ui_camera();
}

/// Sim point (x, y) at height h, in 3D world space.
pub fn w3(x: f32, y: f32, h: f32) -> Vec3 {
    vec3(x - C, h, C - y)
}

/// Maps the arena to the screen rect `r` through an angled perspective camera.
#[derive(Clone, Copy)]
pub struct View {
    pub r: Rect,
    /// Whose buildings these are when no troop says (the player's own base
    /// is drawn blue, enemy bases red).
    pub home: bool,
}

impl View {
    pub fn new(r: Rect) -> View {
        View { r, home: false }
    }

    pub fn home(r: Rect) -> View {
        View { r, home: true }
    }

    pub fn camera(&self) -> Camera3D {
        Camera3D {
            position: vec3(0.0, 1.5, 0.0) + vec3(0.0, 50.0, 43.0) * ZOOM.with(|z| z.get()),
            target: vec3(0.0, 0.0, 1.5),
            up: Vec3::Y,
            fovy: 0.45,
            aspect: Some(self.r.w / self.r.h),
            z_near: 5.0,
            z_far: 200.0,
            ..Default::default()
        }
    }

    fn vp(&self) -> Mat4 {
        // Same as Camera3D::matrix, without needing a window.
        let c = self.camera();
        Mat4::perspective_rh_gl(c.fovy, self.r.w / self.r.h, c.z_near, c.z_far) * Mat4::look_at_rh(c.position, c.target, c.up)
    }

    /// Screen position of a 3D point.
    pub fn proj(&self, p: Vec3) -> Vec2 {
        let c = self.vp() * p.extend(1.0);
        let n = c.truncate() / c.w;
        vec2(self.r.x + (n.x + 1.0) / 2.0 * self.r.w, self.r.y + (1.0 - n.y) / 2.0 * self.r.h)
    }

    /// Screen position of a ground point.
    pub fn p(&self, x: f32, y: f32) -> Vec2 {
        self.proj(w3(x, y, 0.0))
    }

    pub fn p3(&self, x: f32, y: f32, h: f32) -> Vec2 {
        self.proj(w3(x, y, h))
    }

    /// Pixels per arena unit around the middle of the map.
    pub fn s(&self) -> f32 {
        (self.p(C + 1.0, C) - self.p(C, C)).length()
    }

    /// The ground point under a screen position.
    pub fn to_world(&self, v: Vec2) -> (f32, f32) {
        let inv = self.vp().inverse();
        let nx = (v.x - self.r.x) / self.r.w * 2.0 - 1.0;
        let ny = 1.0 - (v.y - self.r.y) / self.r.h * 2.0;
        let a = inv * vec4(nx, ny, -1.0, 1.0);
        let b = inv * vec4(nx, ny, 1.0, 1.0);
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        let t = if (b.y - a.y).abs() > 1e-6 { -a.y / (b.y - a.y) } else { 0.0 };
        let g = a + (b - a) * t;
        (g.x + C, C - g.z)
    }

    /// A circle on the ground, drawn as a projected outline.
    pub fn ring(&self, x: f32, y: f32, r: f32, thick: f32, col: Color) {
        let n = 48;
        let mut prev = self.p(x + r, y);
        for k in 1..=n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            let q = self.p(x + r * a.cos(), y + r * a.sin());
            draw_line(prev.x, prev.y, q.x, q.y, thick, col);
            prev = q;
        }
    }

    /// A filled circle on the ground (projected).
    pub fn disc(&self, x: f32, y: f32, r: f32, col: Color) {
        let n = 48;
        let c = self.p(x, y);
        let mut prev = self.p(x + r, y);
        for k in 1..=n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            let q = self.p(x + r * a.cos(), y + r * a.sin());
            draw_triangle(c, prev, q, col);
            prev = q;
        }
    }
}

pub struct Look {
    pub fig: Figure,
    pub name: Option<String>,
    /// Draw an enemy (red) ground ring instead of a friendly (blue) one.
    pub hostile: bool,
}

// ------------------------------------------------------------- Ground

fn ground_mesh(lib: Option<&Library>) {
    let _ = lib;
    // Wide meadow, with the arena a little lighter and a faint build grid.
    let quad = |x0: f32, z0: f32, x1: f32, z1: f32, h: f32, c: Color| {
        let v = |x, z| Vertex { position: vec3(x, h, z), uv: Vec2::ZERO, color: c.into(), normal: Vec4::ZERO };
        draw_mesh(&Mesh { vertices: vec![v(x0, z0), v(x1, z0), v(x1, z1), v(x0, z1)], indices: vec![0, 1, 2, 0, 2, 3], texture: None });
    };
    quad(-60.0, -60.0, 60.0, 60.0, -0.02, Color::new(0.30, 0.52, 0.22, 1.0));
    for i in 0..16 {
        for j in 0..16 {
            let (x0, z0) = (-C + i as f32 * 2.0, -C + j as f32 * 2.0);
            let c = if (i + j) % 2 == 0 { Color::new(0.42, 0.66, 0.28, 1.0) } else { Color::new(0.39, 0.62, 0.26, 1.0) };
            quad(x0, z0, x0 + 2.0, z0 + 2.0, -0.01, c);
        }
    }
}

/// Fixed scenery around the arena edge.
const SCENERY: &[(&str, f32, f32, f32, f32)] = &[
    ("trees_a", -19.5, -14.0, 1.6, 0.3),
    ("trees_b", -19.0, -4.0, 1.5, 1.1),
    ("hills", -20.0, 7.0, 1.8, 2.0),
    ("trees_a", -19.0, 15.0, 1.5, 0.7),
    ("trees_b", 19.5, -12.0, 1.6, 2.2),
    ("hills", 20.0, 0.0, 1.8, 0.4),
    ("trees_a", 19.0, 10.0, 1.6, 1.5),
    ("trees_b", -10.0, -19.5, 1.5, 0.9),
    ("trees_a", 0.0, -20.0, 1.7, 2.6),
    ("hills", 11.0, -20.0, 1.8, 1.2),
    ("tree_a", -17.5, -18.0, 1.6, 0.0),
    ("tree_b", 17.5, -18.5, 1.6, 0.5),
    ("tree_a", 17.8, 17.0, 1.4, 0.2),
    ("tree_b", -17.6, 17.5, 1.5, 0.8),
    ("rock_c", -15.0, 10.0, 2.0, 0.3),
    ("rock_a", 14.5, -8.0, 2.4, 1.0),
    ("rock_c", 13.0, 14.0, 1.8, 2.0),
    ("rock_a", -12.0, -14.5, 2.2, 0.6),
];

pub fn draw_ground(v: View, show_deploy: bool, show_ranges: bool, w: &World) {
    let lib = library();
    handle_zoom(v.r);
    draw_rectangle(v.r.x, v.r.y, v.r.w, v.r.h, Color::new(0.30, 0.52, 0.22, 1.0));
    begin_3d(v.r, v.camera(), true);
    ground_mesh(lib.as_deref());
    if let Some(lib) = &lib {
        for &(name, x, z, s, rot) in SCENERY {
            if let Some(m) = lib.models.get(name) {
                m.draw_static(Mat4::from_scale_rotation_translation(Vec3::splat(s), Quat::from_rotation_y(rot), vec3(x, 0.0, z)), WHITE);
            }
        }
    }
    end_3d();
    if show_deploy {
        v.disc(C, C, DEPLOY_RADIUS, Color::new(0.6, 0.1, 0.1, 0.22));
        v.ring(C, C, DEPLOY_RADIUS, 2.0, Color::new(0.95, 0.4, 0.4, 0.8));
    }
    if show_ranges {
        for u in &w.units {
            if u.alive && u.kind == Kind::Tower {
                v.ring(u.x, u.y, u.range, 1.2, Color::new(1.0, 0.85, 0.5, 0.35));
            }
        }
    }
    draw_rectangle_lines(v.r.x, v.r.y, v.r.w, v.r.h, 2.0, LINE);
}

// ------------------------------------------------------------- Characters

/// What a character model wears and how it moves.
struct Loadout {
    model: &'static str,
    /// Accessory meshes in the model to show (skinned body parts always show).
    show: &'static [&'static str],
    idle: &'static str,
    attack: &'static str,
    /// Props from other files: (prop, bone).
    props: &'static [(&'static str, &'static str)],
}

fn loadout(c: godcore::classes::BaseClass, raider: bool) -> Loadout {
    use godcore::classes::BaseClass::*;
    if raider {
        return Loadout {
            model: "Skeleton_Minion",
            show: &[],
            idle: "Idle",
            attack: "1H_Melee_Attack_Chop",
            props: &[("sk_axe", "handslot.r")],
        };
    }
    match c {
        Fighter => Loadout {
            model: "Barbarian",
            show: &["2H_Axe", "Barbarian_Hat", "Barbarian_Cape"],
            idle: "2H_Melee_Idle",
            attack: "2H_Melee_Attack_Chop",
            props: &[],
        },
        Archer => Loadout {
            model: "Rogue_Hooded",
            show: &["2H_Crossbow", "Rogue_Cape"],
            idle: "Idle",
            attack: "2H_Ranged_Shoot",
            props: &[],
        },
        Healer => Loadout {
            model: "Mage",
            show: &["2H_Staff", "Mage_Hat", "Mage_Cape"],
            idle: "Idle",
            attack: "Spellcast_Shoot",
            props: &[],
        },
        Guard => Loadout {
            model: "Knight",
            show: &["1H_Sword", "Badge_Shield", "Knight_Helmet", "Knight_Cape"],
            idle: "Idle",
            attack: "1H_Melee_Attack_Chop",
            props: &[],
        },
        Sentry => Loadout {
            model: "Rogue",
            show: &["1H_Crossbow", "Knife_Offhand", "Rogue_Cape"],
            idle: "Idle",
            attack: "1H_Ranged_Shoot",
            props: &[],
        },
        Keeper => Loadout {
            model: "Mage",
            show: &["Spellbook_open", "1H_Wand", "Mage_Cape"],
            idle: "Idle",
            attack: "Spellcast_Raise",
            props: &[],
        },
        Mage => Loadout {
            model: "Mage",
            show: &["1H_Wand", "Spellbook", "Mage_Hat"],
            idle: "Idle",
            attack: "Spellcast_Shoot",
            props: &[],
        },
        Assassin => Loadout {
            model: "Rogue_Hooded",
            show: &["Knife", "Knife_Offhand"],
            idle: "Idle",
            attack: "1H_Melee_Attack_Stab",
            props: &[],
        },
        Paladin => Loadout {
            model: "Knight",
            show: &["2H_Sword", "Knight_Helmet", "Knight_Cape"],
            idle: "2H_Melee_Idle",
            attack: "2H_Melee_Attack_Chop",
            props: &[],
        },
        Warlock => Loadout {
            model: "Mage",
            show: &["2H_Staff", "Mage_Cape"],
            idle: "Idle",
            attack: "Spellcast_Shoot",
            props: &[],
        },
        Hunter => Loadout {
            model: "Barbarian",
            show: &["1H_Axe", "1H_Axe_Offhand", "Barbarian_Cape"],
            idle: "Idle",
            attack: "1H_Melee_Attack_Chop",
            props: &[],
        },
    }
}

/// Weapon meshes built into the character models, hidden when a troop
/// holds a weapon from the armoury instead.
const WEAPON_MESHES: &[&str] = &[
    "1H_Axe", "1H_Axe_Offhand", "2H_Axe", "Barbarian_Round_Shield", "Mug", "1H_Sword", "1H_Sword_Offhand", "2H_Sword",
    "Badge_Shield", "Rectangle_Shield", "Round_Shield", "Spike_Shield", "Spellbook", "Spellbook_open", "1H_Wand",
    "2H_Staff", "Knife", "Knife_Offhand", "1H_Crossbow", "2H_Crossbow", "Throwable",
];

/// Props held for an armoury weapon, and its (idle, attack) animations.
fn weapon_gear(w: Weapon) -> (&'static [(&'static str, &'static str)], &'static str, &'static str) {
    match w {
        Weapon::SwordShield => (&[("sword", "handslot.r"), ("shield", "handslot.l")], "Idle", "1H_Melee_Attack_Chop"),
        Weapon::Greataxe => (&[("greataxe", "handslot.r")], "2H_Melee_Idle", "2H_Melee_Attack_Chop"),
        Weapon::Greatsword => (&[("greatsword", "handslot.r")], "2H_Melee_Idle", "2H_Melee_Attack_Chop"),
        Weapon::Daggers => (&[("dagger", "handslot.r"), ("dagger", "handslot.l")], "Idle", "Dualwield_Melee_Attack_Stab"),
        Weapon::Crossbow => (&[("crossbow2h", "handslot.r")], "Idle", "2H_Ranged_Shoot"),
        Weapon::FireStaff => (&[("staff", "handslot.r")], "Idle", "Spellcast_Shoot"),
        Weapon::HolyWand => (&[("wand", "handslot.r")], "Idle", "Spellcast_Raise"),
    }
}

fn skill_clip(s: Skill) -> &'static str {
    match s {
        Skill::ShieldBash => "1H_Melee_Attack_Slice_Horizontal",
        Skill::Whirlwind => "2H_Melee_Attack_Spin",
        Skill::GroundSlam => "2H_Melee_Attack_Chop",
        Skill::Shadowstep => "Dualwield_Melee_Attack_Chop",
        Skill::Volley => "2H_Ranged_Shoot",
        Skill::Fireball => "Spellcast_Long",
        Skill::HealingNova => "Spellcast_Raise",
    }
}

/// How long an effect stays on screen, in seconds.
fn effect_life(k: EventKind) -> f32 {
    match k {
        EventKind::Skill(_) => 0.8,
        EventKind::Dodge => 0.5,
        _ => 0.45,
    }
}

/// The drill bases' own guards.
const NPC_GUARD: Loadout = Loadout {
    model: "Skeleton_Warrior",
    show: &["Skeleton_Warrior_Helmet"],
    idle: "Idle",
    attack: "1H_Melee_Attack_Chop",
    props: &[("sk_blade", "handslot.r"), ("sk_shield", "handslot.l")],
};

/// The hybrid accessory a second class adds, worn on the back.
fn back_prop(c: godcore::classes::BaseClass) -> Option<&'static str> {
    use godcore::classes::BaseClass::*;
    match c {
        Fighter => Some("axe"),
        Archer => Some("quiver"),
        Guard => Some("shield"),
        Sentry => Some("crossbow"),
        Keeper => Some("spellbook"),
        Mage => Some("tome"),
        Assassin => Some("dagger"),
        Paladin => Some("greatsword"),
        Warlock => Some("staff"),
        Hunter => Some("greataxe"),
        Healer => None,
    }
}

const CHAR_SCALE: f32 = 1.0;

#[derive(Clone, Copy)]
struct CharPose {
    clip_kind: u8, // 0 idle, 1 walk, 2 attack, 3 death, 4 dodge, 5 block, 6 skill, 7 stunned, 8 run
    t: f32,
    skill: Option<Skill>,
}

impl CharPose {
    const fn new(clip_kind: u8, t: f32) -> CharPose {
        CharPose { clip_kind, t, skill: None }
    }
}

/// How a character in a battle looks this frame, besides its clip.
#[derive(Clone, Copy, Default)]
struct Extra {
    /// 0 to 1: the red flash of a hit just taken.
    hurt: f32,
}

fn tint_of(fig: &Figure) -> Color {
    if fig.raider {
        return WHITE;
    }
    let c = fig.outfit;
    Color::new(0.7 + 0.3 * c.r, 0.7 + 0.3 * c.g, 0.7 + 0.3 * c.b, 1.0)
}

/// What this character wears, holds and plays.
fn gear(fig: &Figure, npc_guard: bool) -> (Loadout, Vec<&'static str>) {
    let mut lo = if npc_guard { NPC_GUARD } else { loadout(fig.main, fig.raider) };
    let mut show: Vec<&'static str> = lo.show.to_vec();
    if let Some(w) = fig.weapon.filter(|_| !npc_guard && !fig.raider) {
        show.retain(|n| !WEAPON_MESHES.contains(n));
        (lo.props, lo.idle, lo.attack) = weapon_gear(w);
    }
    (lo, show)
}

/// Seconds one attack swing of this character takes on screen.
fn swing_len(lib: &Library, fig: &Figure, npc_guard: bool) -> f32 {
    let (lo, _) = gear(fig, npc_guard);
    lib.models.get(lo.model).and_then(|m| m.clip_len(lo.attack)).unwrap_or(1.0).max(0.3) / ATTACK_RATE
}

/// Draw one character. Returns false if its model is not loaded.
fn draw_character(lib: &Library, fig: &Figure, npc_guard: bool, pos: Vec3, heading: f32, pose: CharPose, scale: f32) -> bool {
    draw_character_ex(lib, fig, npc_guard, pos, heading, pose, scale, Extra::default(), None)
}

#[allow(clippy::too_many_arguments)]
fn draw_character_ex(
    lib: &Library,
    fig: &Figure,
    npc_guard: bool,
    pos: Vec3,
    heading: f32,
    pose: CharPose,
    scale: f32,
    extra: Extra,
    blend: Option<&mut Blend>,
) -> bool {
    let (lo, show) = gear(fig, npc_guard);
    let Some(m) = lib.models.get(lo.model) else { return false };
    let clip = match pose.clip_kind {
        1 => "Walking_A",
        8 => "Running_A",
        2 => lo.attack,
        3 => "Death_A",
        4 => "Dodge_Backward",
        5 => "Blocking",
        6 => pose.skill.map_or(lo.attack, skill_clip),
        7 => "Hit_A",
        _ => lo.idle,
    };
    let looped = matches!(pose.clip_kind, 0 | 1 | 2 | 5 | 8);
    let speed = match pose.clip_kind {
        2 => ATTACK_RATE,
        4 => 1.6,
        _ => 1.0,
    };
    let now_clip = (clip, pose.t * speed, looped);
    let g = match blend {
        Some(b) => {
            let now = crate::record::now();
            if b.now.map(|c| c.0) != Some(clip) {
                // A new animation: fade out of the old one's last frame.
                b.from = b.now;
                b.at = now;
            }
            b.now = Some(now_clip);
            let w = ((now - b.at) / BLEND_TIME) as f32;
            match b.from {
                Some(f) if w < 1.0 => m.pose_blend((Some(f.0), f.1, f.2), (Some(clip), now_clip.1, looped), w * w * (3.0 - 2.0 * w)),
                _ => m.pose(Some(clip), now_clip.1, looped),
            }
        }
        None => m.pose(Some(clip), now_clip.1, looped),
    };
    let world = Mat4::from_scale_rotation_translation(Vec3::splat(scale), Quat::from_rotation_y(heading), pos);
    let mut tint = if npc_guard { WHITE } else { tint_of(fig) };
    if extra.hurt > 0.0 {
        let h = extra.hurt;
        tint = Color::new(tint.r, tint.g * (1.0 - 0.55 * h), tint.b * (1.0 - 0.6 * h), tint.a);
    }
    m.draw(world, &g, &|n| !is_accessory(m, n) || show.contains(&n), tint);
    for &(prop, bone) in lo.props {
        attach(lib, m, &g, world, prop, bone, Mat4::IDENTITY);
    }
    if !npc_guard && !fig.raider {
        // Blend parts beyond the first show as gear: the second slung across the
        // back over the right shoulder, the third over the left.
        for (slot, part) in [fig.second, fig.third].into_iter().enumerate() {
            let Some(part) = part else { continue };
            if let Some(p) = back_prop(part) {
                let side = if slot == 0 { 1.0 } else { -1.0 };
                let off = Mat4::from_scale_rotation_translation(
                    Vec3::splat(1.0),
                    Quat::from_rotation_y(std::f32::consts::PI) * Quat::from_rotation_z(-0.6 * side),
                    vec3(0.22 * side, 0.42, -0.4),
                );
                attach(lib, m, &g, world, p, "chest", off);
            } else if let Some(h) = m.node("head") {
                // Healer part: a golden halo, built here from a torus of quads.
                let hw = world * g[h];
                halo(hw * Mat4::from_translation(vec3(0.0, 1.05, 0.0)));
            }
        }
    }
    true
}

/// Named accessory meshes that are off unless a loadout turns them on.
fn is_accessory(m: &Model, name: &str) -> bool {
    m.is_rigid_mesh(name)
}

fn attach(lib: &Library, m: &Model, g: &[Mat4], world: Mat4, prop: &str, bone: &str, off: Mat4) {
    if let (Some(p), Some(b)) = (lib.models.get(prop), m.node(bone)) {
        p.draw_static(world * g[b] * off, WHITE);
    }
}

fn halo(at: Mat4) {
    let n = 20;
    let (r, w) = (0.32, 0.06);
    let mut vs = Vec::new();
    let mut ix = Vec::new();
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        for (rr, hh) in [(r - w, 0.0), (r + w, 0.0), (r, w * 0.8)] {
            let p = at.transform_point3(vec3(c * rr, hh, s * rr));
            vs.push(Vertex { position: p, uv: Vec2::ZERO, color: [255, 222, 90, 255], normal: Vec4::ZERO });
        }
        if k < n {
            let b = (k * 3) as u16;
            for (i, j) in [(0u16, 2u16), (2, 1)] {
                ix.extend_from_slice(&[b + i, b + j, b + 3 + i, b + j, b + 3 + j, b + 3 + i]);
            }
        }
    }
    draw_mesh(&Mesh { vertices: vs, indices: ix, texture: None });
}

/// A cannon barrel built from a lit cylinder, aimed along `dir` (3D, unit).
fn cannon_barrel(base: Vec3, dir: Vec3, recoil: f32, scale: f32) {
    let yaw = dir.x.atan2(dir.z);
    let m = Mat4::from_translation(base)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(-1.2)
        * Mat4::from_translation(vec3(0.0, -recoil * 0.3, 0.0))
        * Mat4::from_scale(Vec3::splat(scale));
    let mut vs = Vec::new();
    let mut ix = Vec::new();
    let n = 14;
    let rings = [(-0.45, 0.36), (0.0, 0.30), (0.9, 0.24), (1.0, 0.30), (1.12, 0.30)];
    for (ri, &(y, r)) in rings.iter().enumerate() {
        for k in 0..=n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = (a.cos(), a.sin());
            let p = m.transform_point3(vec3(c * r, y, s * r));
            let nrm = m.transform_vector3(vec3(c, 0.0, s));
            let l = 0.35 + 0.55 * nrm.normalize_or_zero().dot(vec3(-0.45, 0.8, 0.4).normalize()).max(0.0);
            let g = (40.0 + 150.0 * l) as u8;
            vs.push(Vertex { position: p, uv: Vec2::ZERO, color: [g, g, (g as f32 * 1.1).min(255.0) as u8, 255], normal: Vec4::ZERO });
            if ri + 1 < rings.len() && k < n {
                let b = (ri * (n + 1) + k) as u16;
                let up = b + n as u16 + 1;
                ix.extend_from_slice(&[b, b + 1, up, b + 1, up + 1, up]);
            }
        }
    }
    draw_mesh(&Mesh { vertices: vs, indices: ix, texture: None });
    // Muzzle mouth and breech knob.
    draw_sphere(m.transform_point3(vec3(0.0, 1.12, 0.0)), 0.2 * scale, None, Color::new(0.05, 0.05, 0.07, 1.0));
    draw_sphere(m.transform_point3(vec3(0.0, -0.5, 0.0)), 0.22 * scale, None, Color::new(0.25, 0.25, 0.3, 1.0));
}

// ------------------------------------------------------------- World

/// Where a cannon at `i` points: the nearest enemy troop, as a 3D direction.
fn aim_of(w: &World, i: usize) -> Vec3 {
    let me = &w.units[i];
    let mut best: Option<(f32, &Unit)> = None;
    for u in &w.units {
        if u.alive && u.kind == Kind::Troop && u.team != me.team {
            let (dx, dy) = (u.x - me.x, u.y - me.y);
            let d = dx * dx + dy * dy;
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, u));
            }
        }
    }
    best.map_or(vec3(-0.6, 0.0, 0.8), |(_, u)| vec3(u.x - me.x, 0.0, me.y - u.y).normalize_or_zero())
}

/// How far into its strike a unit is (1 = just struck).
fn swing_of(u: &Unit) -> f32 {
    if u.cd_max == 0 {
        return 0.0;
    }
    ((u.cd as f32 / u.cd_max as f32 - 0.55) / 0.45).clamp(0.0, 1.0)
}

fn building_model(u: &Unit, friendly: bool) -> (&'static str, f32, f32) {
    let col = if friendly { "blue" } else { "red" };
    let pick = |b: &'static str, r: &'static str| if friendly { b } else { r };
    let _ = col;
    match (u.kind, u.style) {
        (Kind::Core, _) => (pick("castle_blue", "castle_red"), u.radius * 1.05, 0.0),
        (Kind::Tower, 1) => (pick("towerbase_blue", "towerbase_red"), u.radius * 1.9, 0.0),
        _ => (pick("tower_blue", "tower_red"), u.radius * 1.9, 0.0),
    }
}

/// A building preview under the cursor while placing it.
pub fn draw_building_ghost(v: View, kind: BuildKind, x: f32, y: f32, ok: bool) {
    let range = match kind {
        BuildKind::Tower => 6.0,
        BuildKind::Cannon => 4.5,
    };
    v.ring(x, y, range, 1.0, Color::new(1.0, 0.75, 0.4, 0.5));
    v.disc(x, y, 1.0, if ok { Color::new(1.0, 1.0, 1.0, 0.18) } else { Color::new(1.0, 0.2, 0.2, 0.4) });
    let drawn = library().map_or(false, |lib| {
        let name = match kind {
            BuildKind::Tower => "tower_blue",
            BuildKind::Cannon => "towerbase_blue",
        };
        let Some(m) = lib.models.get(name) else { return false };
        begin_3d(v.r, v.camera(), false);
        let tint = if ok { Color::new(1.0, 1.0, 1.0, 1.0) } else { Color::new(1.0, 0.45, 0.45, 1.0) };
        m.draw_static(Mat4::from_scale_rotation_translation(Vec3::splat(1.9), Quat::IDENTITY, w3(x, y, 0.0)), tint);
        if kind == BuildKind::Cannon {
            cannon_barrel(w3(x, y, 1.5 * 1.9 + 0.2), vec3(-0.6, 0.0, 0.8).normalize(), 0.0, 0.9);
        }
        end_3d();
        true
    });
    if !drawn {
        art::draw_building_icon(v.p(x, y), v.s(), kind);
    }
}

/// A shop icon: the building model in a small 3D view.
pub fn draw_building_icon(c: Vec2, r: f32, kind: BuildKind) {
    let Some(lib) = library() else { return art::draw_building_icon(c, r, kind) };
    let name = match kind {
        BuildKind::Tower => "tower_blue",
        BuildKind::Cannon => "towerbase_blue",
    };
    let Some(m) = lib.models.get(name) else { return art::draw_building_icon(c, r, kind) };
    let rect = Rect::new(c.x - r * 1.4, c.y - r * 2.2, r * 2.8, r * 2.8);
    let cam = Camera3D { position: vec3(2.6, 3.2, 4.2), target: vec3(0.0, 1.25, 0.0), up: Vec3::Y, fovy: 0.75, ..Default::default() };
    begin_3d(rect, cam, true);
    m.draw_static(Mat4::IDENTITY, WHITE);
    if kind == BuildKind::Cannon {
        cannon_barrel(vec3(0.0, 1.6, 0.0), vec3(0.7, 0.0, 0.7).normalize(), 0.0, 0.5);
    }
    end_3d();
}

/// A character portrait: the model's head and shoulders in a round badge.
/// Returns false when no model is available (callers draw the 2D badge).
pub fn draw_portrait3d(c: Vec2, r: f32, fig: &Figure) -> bool {
    let Some(lib) = library() else { return false };
    let rect = Rect::new(c.x - r, c.y - r, r * 2.0, r * 2.0);
    let cam = Camera3D { position: vec3(0.35, 1.55, 2.3), target: vec3(0.0, 1.2, 0.0), up: Vec3::Y, fovy: 0.62, ..Default::default() };
    begin_3d(rect, cam, true);
    let t = crate::record::now() as f32;
    let ok = draw_character(&lib, fig, false, Vec3::ZERO, 0.25, CharPose { clip_kind: 0, t, skill: None }, 1.0);
    end_3d();
    ok
}

/// A full-body character in a small 3D view (recruit and barracks).
pub fn draw_showcase3d(c: Vec2, r: f32, fig: &Figure) -> bool {
    let Some(lib) = library() else { return false };
    let rect = Rect::new(c.x - r, c.y - r, r * 2.0, r * 2.0);
    let cam = Camera3D { position: vec3(1.4, 1.9, 4.0), target: vec3(0.0, 1.1, 0.0), up: Vec3::Y, fovy: 0.72, ..Default::default() };
    begin_3d(rect, cam, true);
    let t = crate::record::now() as f32;
    // Show off the attack every few seconds.
    let cyc = t % 4.0;
    let pose = if cyc > 3.1 { CharPose::new(2, cyc - 3.1) } else { CharPose::new(0, t) };
    let ok = draw_character(&lib, fig, false, Vec3::ZERO, -0.25, pose, 1.0);
    end_3d();
    ok
}

/// A character standing still on the map (placement previews).
pub fn draw_standing(v: View, x: f32, y: f32, fig: &Figure, name: Option<&str>) {
    let drawn = library().map_or(false, |lib| {
        begin_3d(v.r, v.camera(), false);
        let ok = draw_character(&lib, fig, false, w3(x, y, 0.0), heading_of(C - x, C - y), CharPose { clip_kind: 0, t: crate::record::now() as f32, skill: None }, CHAR_SCALE);
        end_3d();
        ok
    });
    if !drawn {
        art::draw_figure(v.p(x, y), v.s(), fig, &Pose { face: 1.0, t: crate::record::now() as f32, ..Default::default() });
    }
    if let Some(n) = name {
        text_centered(n, v.p(x, y).x, v.p(x, y).y + 16.0, 14.0, TEXT);
    }
}

pub fn draw_world(v: View, w: &World, fx: &Effects, look: &dyn Fn(usize, &Unit) -> Look) {
    let t0 = std::time::Instant::now();
    draw_world_inner(v, w, fx, look);
    if std::env::var_os("BAG_PROFILE").is_some() {
        eprintln!("draw_world {:.2} ms, {} units", t0.elapsed().as_secs_f64() * 1000.0, w.units.len());
    }
}

fn draw_world_inner(v: View, w: &World, fx: &Effects, look: &dyn Fn(usize, &Unit) -> Look) {
    let lib = library();
    let s = v.s();
    let now = crate::record::now();
    let t = now as f32;
    // Health bars are green for the side the player is watching over.
    let mut friendly = None;
    for (i, u) in w.units.iter().enumerate() {
        if u.kind == Kind::Troop && friendly.is_none() && !look(i, u).hostile {
            friendly = Some(u.team);
        }
    }
    let friendly = friendly.unwrap_or(if v.home { Team::Defend } else { Team::Attack });
    let looks: Vec<Option<Look>> =
        w.units.iter().enumerate().map(|(i, u)| (u.kind == Kind::Troop).then(|| look(i, u))).collect();
    // Buildings never move; everyone else is drawn between ticks.
    let sp: Vec<(f32, f32, f32)> = w
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| if u.kind.is_building() { (u.x, u.y, 0.0) } else { fx.smooth(i, u) })
        .collect();

    // ---- 3D pass
    let mut flat: Vec<usize> = Vec::new(); // units without a model: 2D fallback
    // Melee fighters mid-swing: how far through it (0 to 1) and what they hit.
    let mut swings: Vec<Option<(f32, (f32, f32))>> = vec![None; w.units.len()];
    begin_3d(v.r, v.camera(), false);
    for (i, u) in w.units.iter().enumerate() {
        let a = fx.anim.get(i).copied();
        let (sx, sy, heading) = sp[i];
        let pos = w3(sx, sy, 0.0);
        if u.kind.is_building() {
            let ours = u.team == friendly;
            let Some(lib) = &lib else {
                flat.push(i);
                continue;
            };
            if !u.alive {
                if let Some(m) = lib.models.get("rubble") {
                    m.draw_static(Mat4::from_scale_rotation_translation(Vec3::splat(u.radius * 1.3), Quat::from_rotation_y(i as f32), pos), WHITE);
                }
                continue;
            }
            let (name, sc, _) = building_model(u, ours);
            let Some(m) = lib.models.get(name) else {
                flat.push(i);
                continue;
            };
            // A hit makes the building flash briefly.
            let hit = a.map_or(0.0, |_| 0.0);
            let _ = hit;
            let rot = if u.kind == Kind::Core { 0.5 } else { (i as f32 * 1.7) % 6.28 };
            m.draw_static(Mat4::from_scale_rotation_translation(Vec3::splat(sc), Quat::from_rotation_y(rot), pos), WHITE);
            if u.style == 1 {
                cannon_barrel(pos + vec3(0.0, 1.5 * sc + 0.15, 0.0), aim_of(w, i), swing_of(u), 0.85);
            }
            continue;
        }
        let Some(lib) = &lib else {
            if u.alive {
                flat.push(i);
            }
            continue;
        };
        let (fig, npc) = if u.kind == Kind::Guard {
            (Figure::npc_guard(), true)
        } else {
            (looks[i].as_ref().map(|l| l.fig).unwrap_or_else(Figure::raider), false)
        };
        let a = a.unwrap_or_default();
        let mut pos = pos;
        let fighting = fx.anim.get(i).and_then(|_| {
            // Keep swinging while hits keep coming, and finish a swing once
            // started. On fast forward the swings speed up with the battle.
            let len = swing_len(lib, &fig, npc) as f64 / fx.speedup();
            let ft = now - a.fight_start;
            let since = now - a.last_hit;
            let swing_of_hit = ((ft - since) / len).floor();
            (since < FIGHT_LINGER || (ft / len).floor() <= swing_of_hit).then_some((ft as f32, len as f32))
        });
        let stride = a.pdist + (a.dist - a.pdist) * fx.blend();
        let pose = if let Some(d) = a.died {
            let since = (now - d) as f32;
            if since > 4.0 {
                continue;
            }
            // Fall, lie still, then sink away.
            pos.y -= (since - 3.0).max(0.0) * 0.7;
            CharPose::new(3, since)
        } else if !u.alive {
            continue;
        } else if u.stun > 0 {
            CharPose { clip_kind: 7, t: 0.15, skill: None }
        } else if a.special == 1 && (now - a.acted) < 0.6 {
            CharPose { clip_kind: 4, t: (now - a.acted) as f32, skill: None }
        } else if a.special == 2 && (now - a.acted) < 1.1 {
            CharPose { clip_kind: 6, t: (now - a.acted) as f32, skill: a.skill }
        } else if u.block_ticks > 0 {
            CharPose { clip_kind: 5, t: t + i as f32 * 0.37, skill: None }
        } else if let Some((ft, len)) = fighting {
            swings[i] = Some(((ft / len).fract(), a.aim));
            CharPose::new(2, ft * fx.speedup() as f32)
        } else if a.still < 0.25 && a.gait > 0.15 {
            CharPose::new(8, stride / RUN_STRIDE)
        } else if a.still < 0.25 {
            CharPose::new(1, stride / WALK_STRIDE)
        } else {
            CharPose { clip_kind: 0, t: t + i as f32 * 0.37, skill: None }
        };
        // Team ring under the feet.
        if u.alive {
            let hostile = looks[i].as_ref().map_or(true, |l| l.hostile);
            let col = if hostile { Color::new(0.95, 0.3, 0.3, 0.9) } else { Color::new(0.4, 0.7, 1.0, 0.9) };
            for k in 0..24 {
                let (a0, a1) = (k as f32 / 24.0 * 6.283, (k + 1) as f32 / 24.0 * 6.283);
                draw_line_3d(pos + vec3(a0.cos() * 0.45, 0.03, a0.sin() * 0.45), pos + vec3(a1.cos() * 0.45, 0.03, a1.sin() * 0.45), col);
            }
        }
        if u.cloak > 0 && looks[i].as_ref().map_or(true, |l| l.hostile) {
            continue; // an enemy in cloak is invisible (a shimmer is drawn later)
        }
        let extra = Extra { hurt: (1.0 - (now - a.hurt) as f32 / 0.15).clamp(0.0, 1.0) * u.alive as u8 as f32 };
        let mut blends = fx.blends.borrow_mut();
        if blends.len() < w.units.len() {
            blends.resize(w.units.len(), Blend::default());
        }
        if !draw_character_ex(lib, &fig, npc, pos, heading, pose, CHAR_SCALE, extra, Some(&mut blends[i])) {
            flat.push(i);
        }
    }
    // Projectiles in flight.
    if let Some(lib) = &lib {
        for (e, age) in &fx.items {
            let (f, to) = (e.from, e.to);
            let from_unit = w.units.iter().find(|u| (u.x - f.0).abs() < 1e-3 && (u.y - f.1).abs() < 1e-3);
            let (h0, model, dur) = match e.kind {
                // Spell bolts are drawn as glowing orbs in the effects pass.
                EventKind::Arrow if from_unit.is_some_and(|u| u.splash > 0.0) => continue,
                EventKind::Arrow => (0.7, "arrow", 0.18),
                EventKind::TowerShot => match from_unit.map(|u| u.style) {
                    Some(1) => (3.0, "ball", 0.25),
                    _ => (3.6, "arrow", 0.2),
                },
                _ => continue,
            };
            let k = (age / dur).min(1.0);
            if k >= 1.0 {
                continue;
            }
            let a = w3(f.0, f.1, h0);
            let b = w3(to.0, to.1, 0.6);
            let arc = 4.0 * k * (1.0 - k) * (a - b).length() * 0.12;
            let p = a.lerp(b, k) + vec3(0.0, arc, 0.0);
            if let Some(m) = lib.models.get(model) {
                let d = (b - a).normalize_or_zero();
                let rot = Quat::from_rotation_arc(Vec3::Y, d);
                m.draw_static(Mat4::from_scale_rotation_translation(Vec3::splat(if model == "ball" { 1.6 } else { 0.9 }), rot, p), WHITE);
            }
        }
    }
    if std::env::var_os("BAG_PROFILE").is_some() {
        eprintln!("  cpu 3d pass {:.2} ms", (get_time() - now) * 1000.0);
    }
    end_3d();

    // ---- 2D fallback for anything without a model
    for &i in &flat {
        let u = &w.units[i];
        let p = v.p(sp[i].0, sp[i].1);
        match u.kind {
            Kind::Core => art::draw_town_hall(p, u.radius * s, t),
            Kind::Tower if u.style == 1 => art::draw_cannon(p, u.radius * s, vec2(1.0, 0.3), swing_of(u)),
            Kind::Tower => art::draw_archer_tower(p, u.radius * s, swing_of(u), t),
            _ => {
                let fig = if u.kind == Kind::Guard {
                    Figure::npc_guard()
                } else {
                    looks[i].as_ref().map(|l| l.fig).unwrap_or_else(Figure::raider)
                };
                let a = fx.anim.get(i).copied().unwrap_or_default();
                let pose = Pose { face: if a.face == 0.0 { 1.0 } else { a.face }, phase: a.phase, moving: a.still < 0.15, swing: swing_of(u), t: t + i as f32 * 0.7 };
                art::draw_figure(p, s, &fig, &pose);
            }
        }
    }

    // ---- Snares on the ground (enemy ones barely visible)
    for tr in &w.traps {
        let ours = tr.team == friendly;
        let a = if ours { 0.9 } else { 0.25 };
        let c = v.p(tr.x, tr.y);
        let r = 0.45 * s;
        draw_circle_lines(c.x, c.y, r, 2.0, Color::new(0.75, 0.6, 0.35, a));
        for k in 0..6 {
            let ang = k as f32 * 1.047;
            draw_line(c.x + ang.cos() * r * 0.4, c.y + ang.sin() * r * 0.4, c.x + ang.cos() * r, c.y + ang.sin() * r, 1.5, Color::new(0.85, 0.85, 0.9, a));
        }
    }

    // ---- Overlays: names and health bars
    for (i, u) in w.units.iter().enumerate() {
        if !u.alive {
            continue;
        }
        let head = match u.kind {
            Kind::Core => 4.4,
            Kind::Tower if u.style == 1 => 3.6,
            Kind::Tower => 4.5,
            _ => 1.25,
        };
        let (ux, uy, _) = sp[i];
        let top = v.p3(ux, uy, head);
        if let Some(Some(l)) = looks.get(i) {
            if let Some(n) = &l.name {
                let foot = v.p(ux, uy);
                let d = measure_text(n, None, 14, 1.0);
                draw_text(n, foot.x - d.width / 2.0, foot.y + 16.0, 14.0, TEXT);
            }
        }
        if u.stun > 0 {
            // Stars circling the head.
            for k in 0..3 {
                let ang = t * 5.0 + k as f32 * 2.094;
                star(top.x + ang.cos() * 12.0, top.y - 14.0 + ang.sin() * 4.0, 4.5, true);
            }
        }
        if u.cloak > 0 {
            let c = v.p3(ux, uy, 0.6);
            let shimmer = 0.5 + 0.5 * (t * 6.0 + i as f32).sin();
            draw_circle(c.x, c.y, 0.5 * s, Color::new(0.25, 0.15, 0.35, 0.25 + 0.15 * shimmer));
        }
        if u.ward > 0 {
            let c = v.p3(ux, uy, 0.7);
            draw_circle(c.x, c.y, 0.62 * s, Color::new(1.0, 0.85, 0.35, 0.12));
            draw_circle_lines(c.x, c.y, 0.62 * s, 2.0, Color::new(1.0, 0.9, 0.5, 0.75));
        }
        if u.block_ticks > 0 {
            let c = v.p3(ux, uy, 0.7);
            draw_circle(c.x, c.y, 0.55 * s, Color::new(0.45, 0.7, 1.0, 0.18));
            draw_circle_lines(c.x, c.y, 0.55 * s, 2.5, Color::new(0.6, 0.85, 1.0, 0.85));
        }
        if u.hp < u.max_hp || u.kind == Kind::Troop {
            let bw = if u.kind.is_building() { u.radius * 1.6 * s } else { s * 1.1 };
            let col = if u.team == friendly { GOOD } else { BAD };
            draw_rectangle(top.x - bw / 2.0 - 1.0, top.y - 7.0, bw + 2.0, 6.0, Color::new(0.0, 0.0, 0.0, 0.6));
            bar(Rect::new(top.x - bw / 2.0, top.y - 6.0, bw, 4.0), u.hp / u.max_hp, col);
        }
    }

    // ---- Swings: a slash across the target as the blow lands.
    for (i, sw) in swings.iter().enumerate() {
        let (Some((ph, aim)), u) = (sw, &w.units[i]) else { continue };
        if u.range > 2.0 || u.heal > 0.0 {
            continue;
        }
        const HIT: (f32, f32) = (0.36, 0.6);
        if *ph < HIT.0 || *ph > HIT.1 {
            continue;
        }
        let k = (ph - HIT.0) / (HIT.1 - HIT.0);
        let (ux, uy, _) = sp[i];
        let (dx, dy) = (aim.0 - ux, aim.1 - uy);
        let d = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (nx, ny) = (dx / d, dy / d);
        // Centre a little in front of the target, the arc across the blow.
        let (cx, cy) = (aim.0 - nx * 0.25, aim.1 - ny * 0.25);
        let fade = (1.0 - k) * 0.9;
        let col = Color::new(1.0, 0.96, 0.82, fade);
        let segs = 9;
        let sweep = (k * 1.6).min(1.0);
        let mut prev: Option<Vec2> = None;
        for j in 0..=segs {
            let f = j as f32 / segs as f32;
            if f > sweep {
                break;
            }
            let ang = (f - 0.5) * 2.2;
            let (ca, sa) = (ang.cos(), ang.sin());
            let (ox, oy) = (-nx * ca * 0.25 + -ny * sa * 0.7, -ny * ca * 0.25 + nx * sa * 0.7);
            let q = v.p3(cx + ox, cy + oy, 0.75 + 0.45 * (1.0 - f));
            if let Some(p0) = prev {
                draw_line(p0.x, p0.y, q.x, q.y, 1.5 + 3.5 * (1.0 - (f - 0.5).abs() * 2.0), col);
            }
            prev = Some(q);
        }
        if (0.0..0.35).contains(&k) {
            let c = v.p3(aim.0, aim.1, 0.8);
            draw_circle(c.x, c.y, 3.0 + 14.0 * k, Color::new(1.0, 0.9, 0.6, 0.5 * (1.0 - k / 0.35)));
        }
    }

    // ---- Effects
    for (e, age) in &fx.items {
        let life = effect_life(e.kind);
        let a = 1.0 - age / life;
        let k = (age / life).min(1.0);
        let f = v.p3(e.from.0, e.from.1, 0.7);
        let to = v.p3(e.to.0, e.to.1, 0.7);
        match e.kind {
            EventKind::Melee | EventKind::GuardHit => {
                let col = if e.kind == EventKind::Melee { Color::new(1.0, 0.95, 0.7, a) } else { Color::new(1.0, 0.5, 0.4, a) };
                if *age > 0.2 {
                    continue;
                }
                let col = Color::new(col.r, col.g, col.b, col.a * 0.6);
                draw_circle_lines(to.x, to.y, 3.0 + 10.0 * age, 1.5, col);
                for k in 0..4 {
                    let ang = k as f32 * 1.57 + 0.4;
                    let r0 = 4.0 + 18.0 * age;
                    draw_line(to.x + ang.cos() * r0 * 0.5, to.y + ang.sin() * r0 * 0.5, to.x + ang.cos() * r0, to.y + ang.sin() * r0, 2.0, col);
                }
            }
            EventKind::Heal => {
                draw_line(f.x, f.y, to.x, to.y, 3.0, Color::new(1.0, 0.95, 0.5, a * 0.7));
                for k in 0..5 {
                    let ang = k as f32 * 1.256 + age * 6.0;
                    let r0 = 8.0 + 10.0 * age;
                    draw_circle(to.x + ang.cos() * r0, to.y - 10.0 * age + ang.sin() * r0 * 0.5, 2.5, Color::new(0.6, 1.0, 0.6, a));
                }
            }
            EventKind::TowerShot if lib.is_none() => {
                let m = f.lerp(to, (age / 0.12).min(1.0));
                draw_circle(m.x, m.y, 4.0, Color::new(1.0, 0.55, 0.2, a));
            }
            EventKind::Arrow
                if w.units.iter().any(|u| u.splash > 0.0 && (u.x - e.from.0).abs() < 1e-3 && (u.y - e.from.1).abs() < 1e-3) =>
            {
                let m = f.lerp(to, (age / 0.18).min(1.0));
                if *age < 0.18 {
                    draw_circle(m.x, m.y, 7.0, Color::new(0.55, 0.45, 1.0, 0.35));
                    draw_circle(m.x, m.y, 4.0, Color::new(0.85, 0.8, 1.0, 0.95));
                }
            }
            EventKind::Blast => {
                let t = (age / 0.45).min(1.0);
                v.ring(e.to.0, e.to.1, 0.3 + 1.3 * t, 3.0, Color::new(0.75, 0.55, 1.0, a));
                if *age < 0.15 {
                    draw_circle(to.x, to.y, 10.0 + 30.0 * age, Color::new(0.8, 0.7, 1.0, 0.5 * a));
                }
            }
            EventKind::Arrow if lib.is_none() => {
                let m = f.lerp(to, (age / 0.15).min(1.0));
                draw_line(f.x, f.y, m.x, m.y, 2.5, Color::new(0.7, 1.0, 0.6, a));
            }
            EventKind::Death => {
                let p = v.p3(e.to.0, e.to.1, 0.5);
                draw_circle_lines(p.x, p.y, 6.0 + 30.0 * age, 3.0, Color::new(1.0, 0.8, 0.4, a));
            }
            EventKind::Dodge => {
                // Dust puffs along the roll.
                for j in 0..5 {
                    let q = v.p3(e.from.0 + (e.to.0 - e.from.0) * j as f32 / 4.0, e.from.1 + (e.to.1 - e.from.1) * j as f32 / 4.0, 0.1);
                    draw_circle(q.x, q.y, 4.0 + 6.0 * k, Color::new(0.85, 0.8, 0.7, 0.5 * a));
                }
            }
            EventKind::Blocked => {
                let r = 8.0 + 10.0 * k;
                draw_circle_lines(to.x, to.y - 6.0, r, 3.0, Color::new(0.55, 0.8, 1.0, a));
            }
            EventKind::Stun => {
                draw_circle_lines(to.x, to.y - 20.0, 6.0 + 14.0 * k, 2.0, Color::new(1.0, 0.95, 0.4, a));
            }
            EventKind::Skill(sk) => skill_effect(v, sk, e, k, a, f, to),
            EventKind::Charge => {
                for j in 0..6 {
                    let q = v.p3(e.from.0 + (e.to.0 - e.from.0) * j as f32 / 5.0, e.from.1 + (e.to.1 - e.from.1) * j as f32 / 5.0, 0.15);
                    draw_circle(q.x, q.y, 5.0 + 5.0 * k, Color::new(0.9, 0.75, 0.5, 0.45 * a));
                }
                draw_circle_lines(to.x, to.y, 8.0 + 20.0 * k, 3.0, Color::new(1.0, 0.85, 0.5, a));
            }
            EventKind::Blink => {
                for p in [f, to] {
                    draw_circle(p.x, p.y, 6.0 + 18.0 * k, Color::new(0.5, 0.4, 1.0, 0.45 * a));
                    draw_circle_lines(p.x, p.y, 10.0 + 18.0 * k, 2.0, Color::new(0.8, 0.75, 1.0, a));
                }
            }
            EventKind::Cloak => {
                for j in 0..5 {
                    let ang = j as f32 * 1.256 + k * 3.0;
                    draw_circle(to.x + ang.cos() * 14.0 * k, to.y - 8.0 + ang.sin() * 8.0 * k, 7.0, Color::new(0.3, 0.25, 0.4, 0.5 * a));
                }
            }
            EventKind::TrapSet => {
                draw_circle_lines(to.x, to.y, 6.0 + 8.0 * k, 2.0, Color::new(0.85, 0.7, 0.4, a));
            }
            EventKind::TrapHit => {
                for j in 0..8 {
                    let ang = j as f32 * 0.785;
                    let r0 = 4.0 + 22.0 * k;
                    draw_line(to.x + ang.cos() * r0 * 0.4, to.y + ang.sin() * r0 * 0.4, to.x + ang.cos() * r0, to.y + ang.sin() * r0, 2.5, Color::new(0.95, 0.8, 0.4, a));
                }
            }
            EventKind::Ward => {
                v.ring(e.from.0, e.from.1, 0.5 + 2.5 * k, 3.0, Color::new(1.0, 0.88, 0.45, a));
            }
            _ => {}
        }
    }
}

/// The flash of a weapon skill. `k` runs 0 to 1 over its life, `a` is its fade.
fn skill_effect(v: View, sk: Skill, e: &godcore::sim::Event, k: f32, a: f32, f: Vec2, to: Vec2) {
    let (ex, ey) = e.from;
    match sk {
        Skill::Whirlwind => {
            v.ring(ex, ey, 0.6 + 1.6 * k, 4.0, Color::new(1.0, 0.75, 0.35, a));
            for j in 0..6 {
                let ang = j as f32 * 1.047 + k * 9.0;
                let p = v.p3(ex + ang.cos() * 1.8, ey + ang.sin() * 1.8, 0.6);
                draw_circle(p.x, p.y, 3.0, Color::new(1.0, 0.9, 0.6, a));
            }
        }
        Skill::GroundSlam => {
            v.ring(ex, ey, 0.4 + 1.8 * k, 5.0, Color::new(0.8, 0.6, 0.35, a));
            v.disc(ex, ey, 2.0 * k, Color::new(0.55, 0.42, 0.28, 0.25 * a));
        }
        Skill::ShieldBash => {
            draw_circle(to.x, to.y, 8.0 + 22.0 * k, Color::new(0.75, 0.88, 1.0, 0.45 * a));
            draw_circle_lines(to.x, to.y, 10.0 + 26.0 * k, 3.0, Color::new(1.0, 1.0, 1.0, a));
        }
        Skill::Shadowstep => {
            draw_line(f.x, f.y, to.x, to.y, 6.0 * a, Color::new(0.55, 0.3, 0.85, 0.6 * a));
            for p in [f, to] {
                draw_circle(p.x, p.y, 6.0 + 16.0 * k, Color::new(0.35, 0.2, 0.5, 0.5 * a));
            }
        }
        Skill::Volley => {
            draw_circle_lines(f.x, f.y, 6.0 + 18.0 * k, 2.0, Color::new(0.8, 1.0, 0.7, a));
        }
        Skill::Fireball => {
            let travel = (k / 0.3).min(1.0);
            if travel < 1.0 {
                let m = f.lerp(to, travel);
                draw_circle(m.x, m.y, 12.0, Color::new(1.0, 0.45, 0.1, 0.45));
                draw_circle(m.x, m.y, 7.0, Color::new(1.0, 0.85, 0.4, 0.95));
            } else {
                let b = (k - 0.3) / 0.7;
                v.ring(e.to.0, e.to.1, 0.4 + 2.2 * b, 5.0, Color::new(1.0, 0.5, 0.15, a));
                v.disc(e.to.0, e.to.1, 2.5 * b, Color::new(1.0, 0.6, 0.2, 0.3 * a));
            }
        }
        Skill::HealingNova => {
            v.ring(ex, ey, 0.5 + 3.0 * k, 4.0, Color::new(0.55, 1.0, 0.6, a));
            v.disc(ex, ey, 3.5 * k, Color::new(0.6, 1.0, 0.6, 0.15 * a));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_map_back_to_the_ground_point() {
        let v = View::new(Rect::new(20.0, 66.0, 714.0, 714.0));
        for &(x, y) in &[(16.0, 16.0), (4.0, 4.0), (25.0, 9.0), (10.0, 28.0)] {
            let (bx, by) = v.to_world(v.p(x, y));
            assert!((bx - x).abs() < 0.01 && (by - y).abs() < 0.01, "{x},{y} -> {bx},{by}");
        }
    }
}

/// Test hook: every class, hybrid and enemy side by side.
pub fn draw_gallery() {
    use godcore::classes::{BaseClass, BaseClass::*, ClassSpec};
    let mut figs: Vec<(String, Figure)> = Vec::new();
    for c in BaseClass::ALL.into_iter().chain(BaseClass::DEFENDERS) {
        figs.push((c.name().to_string(), Figure::of(&c.spec())));
    }
    let blends: [&[(BaseClass, f32)]; 9] = [
        &[(Fighter, 50.0), (Healer, 50.0)],
        &[(Mage, 60.0), (Healer, 40.0)],
        &[(Assassin, 50.0), (Healer, 30.0), (Fighter, 20.0)],
        &[(Paladin, 50.0), (Mage, 30.0), (Archer, 20.0)],
        &[(Archer, 40.0), (Mage, 35.0), (Assassin, 25.0)],
        &[(Fighter, 34.0), (Archer, 33.0), (Healer, 33.0)],
        &[(Guard, 40.0), (Warlock, 35.0), (Hunter, 25.0)],
        &[(Hunter, 60.0), (Sentry, 40.0)],
        &[(Warlock, 50.0), (Keeper, 30.0), (Guard, 20.0)],
    ];
    for parts in blends {
        let s = ClassSpec::blend(parts);
        figs.push((s.name.clone(), Figure::of(&s)));
    }
    figs.push(("Raider".into(), Figure::raider()));
    for (k, (name, f)) in figs.iter().enumerate() {
        let c = vec2(95.0 + (k % 7) as f32 * 182.0, 115.0 + (k / 7) as f32 * 262.0);
        draw_circle(c.x, c.y, 72.0, Color::new(0.2, 0.22, 0.3, 1.0));
        draw_showcase3d(c, 72.0, f);
        // Long blend names wrap onto two lines.
        let words: Vec<&str> = name.split(' ').collect();
        if text_width(name, 17.0) > 150.0 && words.len() > 2 {
            let cut = words.len() / 2;
            text_centered(&words[..cut].join(" "), c.x, c.y + 98.0, 17.0, TEXT);
            text_centered(&words[cut..].join(" "), c.x, c.y + 118.0, 17.0, TEXT);
        } else {
            text_centered(name, c.x, c.y + 98.0, 17.0, TEXT);
        }
    }
}
