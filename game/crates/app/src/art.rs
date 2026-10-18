//! Characters and buildings, drawn in code from simple shapes. Every class has
//! its own silhouette (hair or helmet, outfit, weapon) and hybrids carry an
//! accessory from their second parent.

use crate::ui::*;
use godcore::classes::{BaseClass, ClassSpec};
use godcore::scenario::BuildKind;
use macroquad::prelude::*;

/// How one character is drawn.
#[derive(Clone, Copy)]
pub struct Figure {
    pub main: BaseClass,
    pub second: Option<BaseClass>,
    /// A third blend part, if any (shown as more gear).
    pub third: Option<BaseClass>,
    pub outfit: Color,
    /// Enemy raiders: horned helmets and red eyes.
    pub raider: bool,
    /// A weapon other than the main class's own, drawn in its hands.
    pub weapon: Option<godcore::weapons::Weapon>,
}

impl Figure {
    pub fn of(spec: &ClassSpec) -> Figure {
        let mut ps = spec.parents.clone();
        ps.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        Figure {
            main: ps[0].0,
            second: ps.get(1).map(|p| p.0),
            third: ps.get(2).map(|p| p.0),
            outfit: rgb(spec.color),
            raider: false,
            weapon: spec.weapon.filter(|w| *w != godcore::weapons::Weapon::default_for(spec.main_class())),
        }
    }

    pub fn raider() -> Figure {
        Figure {
            main: BaseClass::Fighter,
            second: None,
            third: None,
            weapon: None,
            outfit: Color::new(0.42, 0.40, 0.44, 1.0),
            raider: true,
        }
    }

    /// The built-in drill guards.
    pub fn npc_guard() -> Figure {
        Figure {
            main: BaseClass::Guard,
            second: None,
            third: None,
            weapon: None,
            outfit: Color::new(0.62, 0.16, 0.18, 1.0),
            raider: false,
        }
    }
}

/// Per-frame pose of a character.
#[derive(Clone, Copy, Default)]
pub struct Pose {
    /// +1 faces right, -1 faces left.
    pub face: f32,
    /// Walk cycle phase in radians; only used when `moving`.
    pub phase: f32,
    pub moving: bool,
    /// 1.0 right as it strikes, falling to 0.
    pub swing: f32,
    /// Time in seconds, for idle bob and glows.
    pub t: f32,
}

const SKIN: Color = Color::new(1.0, 0.86, 0.74, 1.0);
const DARK: Color = Color::new(0.10, 0.09, 0.13, 1.0);
const STEEL: Color = Color::new(0.80, 0.83, 0.88, 1.0);
const WOOD: Color = Color::new(0.52, 0.34, 0.18, 1.0);
const GOLD: Color = Color::new(0.98, 0.80, 0.30, 1.0);

fn shade(c: Color, f: f32) -> Color {
    Color::new(c.r * f, c.g * f, c.b * f, c.a)
}

fn hair_color(c: BaseClass) -> Color {
    match c {
        BaseClass::Fighter => Color::new(0.95, 0.42, 0.20, 1.0),
        BaseClass::Archer => Color::new(0.25, 0.55, 0.30, 1.0),
        BaseClass::Healer => Color::new(1.0, 0.93, 0.62, 1.0),
        BaseClass::Guard => Color::new(0.28, 0.22, 0.20, 1.0),
        BaseClass::Sentry => Color::new(0.82, 0.86, 0.92, 1.0),
        BaseClass::Keeper => Color::new(0.55, 0.35, 0.80, 1.0),
        other => hair_color(stand_in(other)),
    }
}

/// The 2D drawings cover the six original classes; newer ones borrow the
/// nearest look (the 3D models have their own).
fn stand_in(c: BaseClass) -> BaseClass {
    match c {
        BaseClass::Mage => BaseClass::Healer,
        BaseClass::Assassin => BaseClass::Archer,
        BaseClass::Paladin => BaseClass::Guard,
        BaseClass::Warlock => BaseClass::Keeper,
        BaseClass::Hunter => BaseClass::Fighter,
        c => c,
    }
}

/// Maps local figure coordinates (x forward, y up, 1.0 = body height) to the
/// screen, with the feet at `feet`.
#[derive(Clone, Copy)]
struct Frame {
    feet: Vec2,
    k: f32,
    face: f32,
    lift: f32,
}

impl Frame {
    fn p(&self, x: f32, y: f32) -> Vec2 {
        vec2(
            self.feet.x + x * self.k * self.face,
            self.feet.y - (y + self.lift) * self.k,
        )
    }
    fn len(&self, l: f32) -> f32 {
        l * self.k
    }
    fn line(&self, a: (f32, f32), b: (f32, f32), w: f32, c: Color) {
        let (p, q) = (self.p(a.0, a.1), self.p(b.0, b.1));
        draw_line(p.x, p.y, q.x, q.y, self.len(w).max(1.0), c);
    }
    fn circle(&self, x: f32, y: f32, r: f32, c: Color) {
        let p = self.p(x, y);
        draw_circle(p.x, p.y, self.len(r), c);
    }
    fn tri(&self, a: (f32, f32), b: (f32, f32), c: (f32, f32), col: Color) {
        draw_triangle(self.p(a.0, a.1), self.p(b.0, b.1), self.p(c.0, c.1), col);
    }
    fn quad(&self, a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32), col: Color) {
        self.tri(a, b, c, col);
        self.tri(a, c, d, col);
    }
    /// Point at distance `l` from `o` along angle `ang` (radians, 0 = forward, up positive).
    fn polar(o: (f32, f32), ang: f32, l: f32) -> (f32, f32) {
        (o.0 + ang.cos() * l, o.1 + ang.sin() * l)
    }
}

/// Draw a full character standing at `feet`, `s` pixels per arena unit.
pub fn draw_figure(feet: Vec2, s: f32, fig: &Figure, pose: &Pose) {
    let fig = &Figure {
        main: stand_in(fig.main),
        second: fig.second.map(stand_in),
        third: fig.third.map(stand_in),
        ..*fig
    };
    let k = s * 1.6;
    let bob = if pose.moving {
        (pose.phase.sin()).abs() * 0.035
    } else {
        (pose.t * 2.2).sin() * 0.008
    };
    let f = Frame {
        feet,
        k,
        face: if pose.face < 0.0 { -1.0 } else { 1.0 },
        lift: bob,
    };
    let flat = Frame { lift: 0.0, ..f };
    draw_ellipse(
        feet.x,
        feet.y,
        k * 0.30,
        k * 0.09,
        0.0,
        Color::new(0.0, 0.0, 0.0, 0.35),
    );

    let robe = matches!(fig.main, BaseClass::Healer | BaseClass::Keeper);
    let outfit = fig.outfit;
    let trim = shade(outfit, 0.6);

    // Back items: cape, quiver, back shield.
    if matches!(fig.main, BaseClass::Archer | BaseClass::Sentry)
        || fig.second == Some(BaseClass::Sentry)
    {
        let wave = (pose.t * 4.0 + pose.phase).sin() * 0.03;
        f.quad(
            (-0.06, 0.66),
            (0.04, 0.66),
            (-0.08, 0.28),
            (-0.22 + wave, 0.30),
            shade(outfit, 0.45),
        );
    }
    if fig.main == BaseClass::Archer || fig.second == Some(BaseClass::Archer) {
        f.quad(
            (-0.16, 0.70),
            (-0.08, 0.72),
            (-0.04, 0.40),
            (-0.12, 0.38),
            WOOD,
        );
        for dx in [0.0, 0.035, 0.07] {
            f.line(
                (-0.135 + dx, 0.70),
                (-0.155 + dx, 0.80),
                0.018,
                Color::new(0.9, 0.9, 0.85, 1.0),
            );
        }
    }
    if fig.second == Some(BaseClass::Guard) {
        f.circle(-0.14, 0.52, 0.12, shade(STEEL, 0.7));
        f.circle(-0.14, 0.52, 0.08, outfit);
    }

    // Legs
    let (stride, lift2) = if pose.moving {
        (pose.phase.sin() * 0.09, pose.phase.cos().max(0.0) * 0.03)
    } else {
        (0.0, 0.0)
    };
    let leg_c = shade(outfit, 0.35);
    if !robe {
        flat.line(
            (-0.04, 0.36 + bob),
            (-0.05 - stride, 0.03 + lift2),
            0.075,
            leg_c,
        );
        flat.line((0.05, 0.36 + bob), (0.06 + stride, 0.03), 0.075, leg_c);
        flat.circle(-0.05 - stride, 0.03 + lift2, 0.045, DARK);
        flat.circle(0.07 + stride, 0.03, 0.045, DARK);
    } else {
        flat.circle(-0.05 - stride * 0.5, 0.03, 0.045, DARK);
        flat.circle(0.07 + stride * 0.5, 0.03, 0.045, DARK);
    }

    // Torso
    if robe {
        f.quad(
            (-0.13, 0.66),
            (0.13, 0.66),
            (0.19, 0.06),
            (-0.19, 0.06),
            outfit,
        );
        f.quad(
            (-0.03, 0.66),
            (0.04, 0.66),
            (0.06, 0.06),
            (-0.04, 0.06),
            shade(outfit, 1.25),
        );
        f.line((-0.15, 0.46), (0.15, 0.46), 0.03, GOLD);
    } else {
        let armour = matches!(fig.main, BaseClass::Fighter | BaseClass::Guard);
        f.quad(
            (-0.13, 0.68),
            (0.13, 0.68),
            (0.12, 0.33),
            (-0.12, 0.33),
            outfit,
        );
        if armour {
            f.quad(
                (-0.14, 0.68),
                (0.14, 0.68),
                (0.12, 0.52),
                (-0.12, 0.52),
                shade(STEEL, 0.85),
            );
            f.circle(-0.13, 0.65, 0.065, STEEL);
            f.circle(0.13, 0.65, 0.065, STEEL);
        }
        f.line((-0.12, 0.38), (0.12, 0.38), 0.04, trim);
        f.circle(0.0, 0.38, 0.025, GOLD);
    }

    // Head
    let hy = 0.83;
    let hair = if fig.raider {
        Color::new(0.25, 0.22, 0.2, 1.0)
    } else {
        hair_color(fig.main)
    };
    match fig.main {
        BaseClass::Healer | BaseClass::Archer => {
            // Long hair behind the head (ponytail for archers).
            if fig.main == BaseClass::Archer {
                let sw = (pose.t * 3.0 + pose.phase).sin() * 0.03;
                f.tri((-0.10, 0.90), (-0.12, 0.80), (-0.30 + sw, 0.66), hair);
            } else {
                f.quad(
                    (-0.15, 0.88),
                    (0.08, 0.88),
                    (0.10, 0.60),
                    (-0.17, 0.60),
                    hair,
                );
            }
        }
        BaseClass::Keeper => {
            f.circle(-0.01, hy + 0.01, 0.19, shade(outfit, 0.75));
            f.tri(
                (-0.17, 0.84),
                (-0.02, 0.98),
                (-0.28, 1.02),
                shade(outfit, 0.75),
            );
        }
        _ => {}
    }
    f.circle(0.0, hy, 0.15, SKIN);
    // Big anime eyes, looking forward.
    let eye = if fig.raider {
        Color::new(0.9, 0.15, 0.1, 1.0)
    } else {
        Color::new(0.15, 0.2, 0.4, 1.0)
    };
    for ex in [0.035, 0.105] {
        let p = f.p(ex, hy - 0.01);
        draw_ellipse(p.x, p.y, f.len(0.024), f.len(0.04), 0.0, eye);
        let h = f.p(ex + 0.008, hy + 0.012);
        draw_circle(h.x, h.y, f.len(0.011), WHITE);
    }
    // Hair, helmets, hoods on top.
    match (fig.main, fig.raider) {
        (_, true) => {
            f.quad(
                (-0.16, 0.86),
                (0.16, 0.86),
                (0.12, 0.99),
                (-0.12, 0.99),
                Color::new(0.45, 0.42, 0.40, 1.0),
            );
            f.tri(
                (-0.13, 0.94),
                (-0.20, 0.96),
                (-0.26, 1.10),
                Color::new(0.92, 0.88, 0.78, 1.0),
            );
            f.tri(
                (0.13, 0.94),
                (0.20, 0.96),
                (0.24, 1.10),
                Color::new(0.92, 0.88, 0.78, 1.0),
            );
        }
        (BaseClass::Fighter, _) => {
            // Spiky hair.
            for (a, b, c) in [
                ((-0.16, 0.84), (0.0, 0.96), (-0.28, 1.02)),
                ((-0.10, 0.93), (0.08, 0.97), (-0.06, 1.13)),
                ((0.0, 0.96), (0.16, 0.90), (0.18, 1.08)),
                ((0.08, 0.94), (0.17, 0.82), (0.25, 0.94)),
            ] {
                f.tri(a, b, c, hair);
            }
            f.quad(
                (-0.15, 0.86),
                (0.15, 0.86),
                (0.13, 0.97),
                (-0.13, 0.97),
                hair,
            );
        }
        (BaseClass::Guard, _) => {
            f.quad(
                (-0.17, 0.84),
                (0.17, 0.84),
                (0.13, 1.0),
                (-0.13, 1.0),
                STEEL,
            );
            f.circle(0.0, 0.98, 0.07, STEEL);
            f.line((0.0, 1.03), (-0.08, 1.12), 0.04, outfit);
            f.line((0.06, 0.86), (0.06, 0.74), 0.03, shade(STEEL, 0.8));
        }
        (BaseClass::Sentry, _) => {
            f.quad(
                (-0.16, 0.86),
                (0.16, 0.86),
                (0.13, 1.0),
                (-0.13, 1.0),
                shade(outfit, 0.9),
            );
            f.quad(
                (-0.02, 0.88),
                (0.17, 0.88),
                (0.17, 0.83),
                (-0.02, 0.83),
                Color::new(0.2, 0.9, 1.0, 0.85),
            );
            f.tri((-0.14, 0.92), (-0.04, 0.98), (-0.20, 1.06), hair);
        }
        (BaseClass::Archer, _) => {
            f.quad(
                (-0.15, 0.86),
                (0.15, 0.88),
                (0.12, 0.98),
                (-0.13, 0.98),
                hair,
            );
            f.tri((0.04, 0.95), (0.16, 0.86), (0.17, 0.80), hair);
        }
        (BaseClass::Healer, _) => {
            f.quad(
                (-0.15, 0.86),
                (0.15, 0.88),
                (0.12, 0.98),
                (-0.13, 0.98),
                hair,
            );
            f.tri((0.0, 0.95), (0.16, 0.88), (0.15, 0.76), hair);
        }
        (BaseClass::Keeper, _) => {
            f.quad(
                (-0.16, 0.88),
                (0.12, 0.92),
                (0.10, 1.0),
                (-0.12, 1.0),
                shade(outfit, 0.75),
            );
            f.tri((0.0, 0.93), (0.15, 0.88), (0.13, 0.80), hair);
        }
        _ => {}
    }
    // Second-parent accessories on the head.
    match fig.second {
        Some(BaseClass::Fighter) => {
            f.line(
                (-0.15, 0.90),
                (0.15, 0.92),
                0.04,
                Color::new(0.9, 0.15, 0.15, 1.0),
            );
            let sw = (pose.t * 5.0).sin() * 0.03;
            f.line(
                (-0.15, 0.90),
                (-0.27, 0.84 + sw),
                0.03,
                Color::new(0.9, 0.15, 0.15, 1.0),
            );
        }
        Some(BaseClass::Healer) => {
            let p = f.p(0.0, 1.10);
            draw_ellipse_lines(
                p.x,
                p.y,
                f.len(0.11),
                f.len(0.03),
                0.0,
                f.len(0.025).max(1.5),
                GOLD,
            );
        }
        Some(BaseClass::Keeper) => {
            let o = (
                0.24 + (pose.t * 2.0).sin() * 0.02,
                0.92 + (pose.t * 2.6).cos() * 0.03,
            );
            f.circle(o.0, o.1, 0.07, Color::new(0.75, 0.5, 1.0, 0.35));
            f.circle(o.0, o.1, 0.04, Color::new(0.9, 0.75, 1.0, 1.0));
        }
        _ => {}
    }

    // Weapon arm.
    let sw = pose.swing.clamp(0.0, 1.0);
    let hand = (0.14, 0.52);
    let arm_c = if robe { outfit } else { SKIN };
    match fig.main {
        BaseClass::Fighter => {
            // Sword raised behind at rest, slashing forward on a hit.
            let ang = 1.9 - sw * 2.6;
            let h = Frame::polar((0.08, 0.60), ang - 0.6, 0.14);
            f.line((0.08, 0.62), h, 0.06, arm_c);
            let tip = Frame::polar(h, ang, 0.5);
            f.line(h, tip, 0.05, STEEL);
            f.line(
                Frame::polar(h, ang + 1.57, 0.07),
                Frame::polar(h, ang - 1.57, 0.07),
                0.035,
                GOLD,
            );
            if sw > 0.4 {
                let a = (sw - 0.4) / 0.6;
                for d in 0..4 {
                    let t = d as f32 * 0.25;
                    f.line(
                        Frame::polar(h, ang + 0.3 + t, 0.48),
                        Frame::polar(h, ang + 0.55 + t, 0.48),
                        0.03,
                        Color::new(1.0, 1.0, 1.0, 0.5 * a),
                    );
                }
            }
        }
        BaseClass::Archer => {
            let draw = sw * 0.12;
            f.line((0.06, 0.62), (0.24, 0.58), 0.05, arm_c);
            let c = (0.26, 0.58);
            let mut prev = Frame::polar(c, 1.25, 0.26);
            for i in 1..=8 {
                let a = 1.25 - i as f32 * (2.5 / 8.0);
                let r = 0.26;
                let p = (c.0 + a.cos() * r * 0.45 + 0.04, c.1 + a.sin() * r);
                f.line(prev, p, 0.035, WOOD);
                prev = p;
            }
            let (top, bot) = (
                (
                    c.0 + 1.25f32.cos() * 0.26 * 0.45 + 0.04,
                    c.1 + 1.25f32.sin() * 0.26,
                ),
                (
                    c.0 + 1.25f32.cos() * 0.26 * 0.45 + 0.04,
                    c.1 - 1.25f32.sin() * 0.26,
                ),
            );
            let nock = (top.0 - 0.06 - draw, c.1);
            f.line(top, nock, 0.012, Color::new(0.95, 0.95, 0.9, 1.0));
            f.line(nock, bot, 0.012, Color::new(0.95, 0.95, 0.9, 1.0));
            if sw < 0.5 {
                f.line(
                    nock,
                    (nock.0 + 0.36, c.1),
                    0.02,
                    Color::new(0.9, 0.85, 0.7, 1.0),
                );
            }
        }
        BaseClass::Healer => {
            f.line((0.06, 0.62), hand, 0.05, arm_c);
            f.line((0.18, 0.04), (0.18, 0.98), 0.04, WOOD);
            let glow = 0.5 + 0.5 * (pose.t * 3.0).sin() + sw;
            f.circle(
                0.18,
                1.0,
                0.09 + 0.03 * glow,
                Color::new(1.0, 0.95, 0.55, 0.25),
            );
            f.circle(0.18, 1.0, 0.055, Color::new(1.0, 0.97, 0.75, 1.0));
            f.line((0.12, 0.98), (0.24, 0.98), 0.025, GOLD);
        }
        BaseClass::Guard => {
            // Spear behind, thrust on a hit; tower shield in front.
            let th = sw * 0.18;
            f.line((-0.10 + th, 0.40), (0.42 + th, 0.80), 0.035, WOOD);
            f.tri(
                (0.40 + th, 0.84),
                (0.44 + th, 0.76),
                (0.52 + th, 0.87),
                STEEL,
            );
            f.quad(
                (0.10, 0.70),
                (0.24, 0.70),
                (0.24, 0.26),
                (0.10, 0.26),
                shade(STEEL, 0.75),
            );
            f.quad(
                (0.12, 0.68),
                (0.22, 0.68),
                (0.22, 0.28),
                (0.12, 0.28),
                outfit,
            );
            f.line((0.17, 0.62), (0.17, 0.34), 0.03, GOLD);
            f.line((0.13, 0.50), (0.21, 0.50), 0.03, GOLD);
        }
        BaseClass::Sentry => {
            let kick = sw * 0.05;
            f.line((0.06, 0.62), (0.20 - kick, 0.56), 0.05, arm_c);
            f.line((0.08 - kick, 0.56), (0.40 - kick, 0.56), 0.05, WOOD);
            f.line(
                (0.34 - kick, 0.68),
                (0.34 - kick, 0.44),
                0.035,
                shade(STEEL, 0.8),
            );
            if sw < 0.5 {
                f.line(
                    (0.20 - kick, 0.58),
                    (0.44 - kick, 0.58),
                    0.02,
                    Color::new(0.9, 0.85, 0.7, 1.0),
                );
            }
        }
        BaseClass::Keeper => {
            f.line((0.06, 0.62), (0.18, 0.60), 0.05, arm_c);
            let bob = (pose.t * 2.5).sin() * 0.03;
            let o = (0.26, 0.70 + bob);
            f.circle(
                o.0,
                o.1,
                0.12 + sw * 0.08,
                Color::new(0.65, 0.45, 1.0, 0.25),
            );
            f.circle(o.0, o.1, 0.07, Color::new(0.80, 0.65, 1.0, 1.0));
            f.circle(o.0 + 0.02, o.1 + 0.02, 0.025, WHITE);
        }
        _ => {}
    }
}

/// A round badge with the character's head and shoulders, for lists.
pub fn draw_portrait(c: Vec2, r: f32, spec: &ClassSpec) {
    let fig = Figure::of(spec);
    draw_circle(c.x, c.y, r, shade(fig.outfit, 0.35));
    if crate::render::draw_portrait3d(c, r, &fig) {
        draw_circle_lines(c.x, c.y, r, 2.0, fig.outfit);
        return;
    }
    draw_figure(
        vec2(c.x - r * 0.1, c.y + r * 0.9),
        r * 1.25,
        &fig,
        &Pose {
            face: 1.0,
            t: crate::record::now() as f32,
            ..Default::default()
        },
    );
    draw_circle_lines(c.x, c.y, r, 2.0, fig.outfit);
}

/// A portrait sized for a large preview: the whole character on a plinth.
pub fn draw_showcase(c: Vec2, r: f32, spec: &ClassSpec) {
    let fig = Figure::of(spec);
    draw_circle(c.x, c.y, r, shade(fig.outfit, 0.3));
    draw_circle_lines(c.x, c.y, r, 2.0, fig.outfit);
    if crate::render::draw_showcase3d(c, r, &fig) {
        return;
    }
    let t = crate::record::now() as f32;
    let swing = ((t * 0.8).fract() * 4.0 - 3.0).max(0.0);
    draw_figure(
        vec2(c.x - r * 0.1, c.y + r * 0.8),
        r * 1.15,
        &fig,
        &Pose {
            face: 1.0,
            t,
            swing,
            ..Default::default()
        },
    );
}

// ---------------------------------------------------------------- Buildings

/// Town Hall: a hall with a red roof, a central tower and a waving flag.
/// `p` is the footprint centre and `r` its half-size in pixels.
pub fn draw_town_hall(p: Vec2, r: f32, t: f32) {
    let base_y = p.y + r * 0.75;
    draw_ellipse(
        p.x,
        base_y,
        r * 1.1,
        r * 0.35,
        0.0,
        Color::new(0.0, 0.0, 0.0, 0.35),
    );
    // Stone foundation
    draw_rectangle(
        p.x - r,
        base_y - r * 0.28,
        r * 2.0,
        r * 0.28,
        Color::new(0.50, 0.50, 0.55, 1.0),
    );
    for i in 0..5 {
        let x = p.x - r + i as f32 * r * 0.4;
        draw_line(
            x,
            base_y - r * 0.28,
            x,
            base_y,
            1.0,
            Color::new(0.35, 0.35, 0.4, 1.0),
        );
    }
    // Walls
    let wall = Color::new(0.93, 0.86, 0.70, 1.0);
    let wt = base_y - r * 0.28 - r * 0.7;
    draw_rectangle(p.x - r * 0.88, wt, r * 1.76, r * 0.7, wall);
    for x in [-0.88, -0.3, 0.3, 0.88] {
        draw_line(p.x + x * r, wt, p.x + x * r, wt + r * 0.7, r * 0.08, WOOD);
    }
    draw_line(
        p.x - r * 0.88,
        wt + r * 0.02,
        p.x + r * 0.88,
        wt + r * 0.02,
        r * 0.07,
        WOOD,
    );
    // Door and windows
    draw_rectangle(
        p.x - r * 0.16,
        wt + r * 0.32,
        r * 0.32,
        r * 0.38,
        Color::new(0.35, 0.2, 0.1, 1.0),
    );
    draw_circle(
        p.x,
        wt + r * 0.32,
        r * 0.16,
        Color::new(0.35, 0.2, 0.1, 1.0),
    );
    for x in [-0.6, 0.6] {
        draw_rectangle(
            p.x + x * r - r * 0.1,
            wt + r * 0.2,
            r * 0.2,
            r * 0.18,
            Color::new(1.0, 0.85, 0.4, 1.0),
        );
    }
    // Roof
    let roof = Color::new(0.72, 0.18, 0.20, 1.0);
    draw_triangle(
        vec2(p.x - r * 1.05, wt),
        vec2(p.x + r * 1.05, wt),
        vec2(p.x, wt - r * 0.75),
        roof,
    );
    draw_triangle_lines(
        vec2(p.x - r * 1.05, wt),
        vec2(p.x + r * 1.05, wt),
        vec2(p.x, wt - r * 0.75),
        2.0,
        shade(roof, 0.6),
    );
    // Tower and flag
    let tt = wt - r * 1.1;
    draw_rectangle(p.x - r * 0.18, tt, r * 0.36, r * 0.5, wall);
    draw_triangle(
        vec2(p.x - r * 0.26, tt),
        vec2(p.x + r * 0.26, tt),
        vec2(p.x, tt - r * 0.4),
        roof,
    );
    let pole = tt - r * 0.4;
    draw_line(p.x, pole, p.x, pole - r * 0.45, 2.0, DARK);
    let wv = (t * 4.0).sin() * r * 0.06;
    draw_triangle(
        vec2(p.x, pole - r * 0.45),
        vec2(p.x, pole - r * 0.22),
        vec2(p.x + r * 0.42, pole - r * 0.34 + wv),
        GOLD,
    );
}

/// Archer tower: a stone tower with a wooden platform and a lookout.
pub fn draw_archer_tower(p: Vec2, r: f32, shot: f32, t: f32) {
    let base_y = p.y + r * 0.7;
    draw_ellipse(
        p.x,
        base_y,
        r * 0.95,
        r * 0.3,
        0.0,
        Color::new(0.0, 0.0, 0.0, 0.35),
    );
    let stone = Color::new(0.60, 0.60, 0.64, 1.0);
    let top_y = base_y - r * 1.5;
    draw_triangle(
        vec2(p.x - r * 0.65, base_y),
        vec2(p.x + r * 0.65, base_y),
        vec2(p.x + r * 0.45, top_y),
        stone,
    );
    draw_triangle(
        vec2(p.x - r * 0.65, base_y),
        vec2(p.x + r * 0.45, top_y),
        vec2(p.x - r * 0.45, top_y),
        stone,
    );
    for k in 1..4 {
        let y = base_y - k as f32 * r * 0.38;
        let hw = r * (0.65 - 0.2 * k as f32 / 4.0);
        draw_line(p.x - hw, y, p.x + hw, y, 1.0, shade(stone, 0.7));
    }
    // Platform with crenellations
    draw_rectangle(p.x - r * 0.75, top_y - r * 0.2, r * 1.5, r * 0.24, WOOD);
    for i in 0..4 {
        draw_rectangle(
            p.x - r * 0.75 + i as f32 * r * 0.42,
            top_y - r * 0.42,
            r * 0.24,
            r * 0.24,
            shade(WOOD, 1.2),
        );
    }
    // Lookout archer, who leans out when shooting.
    let lean = shot * r * 0.12;
    draw_circle(p.x + lean, top_y - r * 0.42, r * 0.17, SKIN);
    draw_rectangle(
        p.x - r * 0.18 + lean,
        top_y - r * 0.62,
        r * 0.36,
        r * 0.12,
        Color::new(0.25, 0.55, 0.30, 1.0),
    );
    // Pennant
    let wv = (t * 5.0 + p.x).sin() * r * 0.05;
    draw_line(
        p.x + r * 0.6,
        top_y - r * 0.2,
        p.x + r * 0.6,
        top_y - r * 0.85,
        1.5,
        DARK,
    );
    draw_triangle(
        vec2(p.x + r * 0.6, top_y - r * 0.85),
        vec2(p.x + r * 0.6, top_y - r * 0.62),
        vec2(p.x + r * 0.95, top_y - r * 0.74 + wv),
        Color::new(0.95, 0.4, 0.3, 1.0),
    );
}

/// Cannon: a heavy barrel on a wooden turntable, aimed along `aim` (screen space).
pub fn draw_cannon(p: Vec2, r: f32, aim: Vec2, shot: f32) {
    let base_y = p.y + r * 0.3;
    draw_ellipse(
        p.x,
        base_y + r * 0.25,
        r * 0.95,
        r * 0.4,
        0.0,
        Color::new(0.0, 0.0, 0.0, 0.35),
    );
    draw_ellipse(p.x, base_y, r * 0.85, r * 0.5, 0.0, shade(WOOD, 0.8));
    draw_ellipse(p.x, base_y - r * 0.08, r * 0.8, r * 0.45, 0.0, WOOD);
    let d = if aim.length() > 0.01 {
        aim.normalize()
    } else {
        vec2(1.0, 0.0)
    };
    let pivot = vec2(p.x, base_y - r * 0.3);
    let recoil = shot * r * 0.18;
    let muzzle = pivot + d * (r * 0.95 - recoil);
    let back = pivot - d * (r * 0.35 + recoil);
    draw_line(
        back.x,
        back.y,
        muzzle.x,
        muzzle.y,
        r * 0.42,
        Color::new(0.18, 0.18, 0.22, 1.0),
    );
    draw_circle(
        muzzle.x,
        muzzle.y,
        r * 0.24,
        Color::new(0.25, 0.25, 0.3, 1.0),
    );
    draw_circle(muzzle.x, muzzle.y, r * 0.12, DARK);
    draw_circle(back.x, back.y, r * 0.2, Color::new(0.25, 0.25, 0.3, 1.0));
    draw_circle(pivot.x, pivot.y, r * 0.14, GOLD);
    if shot > 0.6 {
        let a = (shot - 0.6) / 0.4;
        let f = muzzle + d * r * 0.3;
        draw_circle(f.x, f.y, r * 0.3 * a, Color::new(1.0, 0.75, 0.3, a));
    }
}

/// A shop icon for a building kind; `c` is the centre, `r` its half-size.
pub fn draw_building_icon(c: Vec2, r: f32, kind: BuildKind) {
    match kind {
        BuildKind::Tower => draw_archer_tower(c, r, 0.0, crate::record::now() as f32),
        BuildKind::Cannon => draw_cannon(c, r, vec2(1.0, -0.3), 0.0),
    }
}

/// Looks for a drill: the trained troop (named), its copies, built-in helpers
/// and enemies.
pub fn drill_look(
    i: usize,
    u: &godcore::sim::Unit,
    controlled: &[usize],
    mine: godcore::sim::Team,
    fig: Figure,
    name: &str,
) -> crate::render::Look {
    use crate::render::Look;
    use godcore::sim::{Control, Team};
    if controlled.first() == Some(&i) {
        Look {
            fig,
            name: Some(name.to_string()),
            hostile: false,
        }
    } else if u.team != mine {
        let fig = if u.team == Team::Attack {
            Figure::raider()
        } else {
            Figure::npc_guard()
        };
        Look {
            fig,
            name: None,
            hostile: true,
        }
    } else if u.control == Control::Scripted {
        Look {
            fig: Figure::of(&BaseClass::Guard.spec()),
            name: None,
            hostile: false,
        }
    } else {
        Look {
            fig,
            name: None,
            hostile: false,
        }
    }
}
