//! Immediate-mode UI helpers on a fixed 1280x800 virtual canvas.

use macroquad::prelude::*;

pub const W: f32 = 1280.0;
pub const H: f32 = 800.0;

pub const BG: Color = Color::new(0.075, 0.082, 0.118, 1.0);
pub const PANEL: Color = Color::new(0.125, 0.137, 0.188, 1.0);
pub const PANEL_HI: Color = Color::new(0.17, 0.185, 0.25, 1.0);
pub const LINE: Color = Color::new(0.26, 0.28, 0.37, 1.0);
pub const TEXT: Color = Color::new(0.93, 0.93, 0.96, 1.0);
pub const MUTED: Color = Color::new(0.62, 0.64, 0.72, 1.0);
pub const ACCENT: Color = Color::new(0.98, 0.70, 0.25, 1.0);
pub const GOOD: Color = Color::new(0.40, 0.85, 0.50, 1.0);
pub const BAD: Color = Color::new(0.95, 0.40, 0.40, 1.0);
pub const SKY: Color = Color::new(0.40, 0.65, 0.98, 1.0);

/// Per-frame input in virtual coordinates.
pub struct Ui {
    pub mouse: Vec2,
    pub clicked: bool,
    pub right_clicked: bool,
    camera: Camera2D,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            mouse: Vec2::ZERO,
            clicked: false,
            right_clicked: false,
            camera: Camera2D::default(),
        }
    }

    /// Letterbox the virtual canvas into the window and read input.
    pub fn begin(&mut self) {
        let (sw, sh) = (screen_width(), screen_height());
        let s = (sw / W).min(sh / H);
        let (vw, vh) = (W * s, H * s);
        let (vx, vy) = ((sw - vw) / 2.0, (sh - vh) / 2.0);
        // A negative height makes y grow downwards on screen (from_display_rect
        // alone renders upside down when drawing to the window).
        let mut cam = Camera2D::from_display_rect(Rect::new(0.0, H, W, -H));
        // Viewports are in real pixels, while screen size and the mouse are in
        // scaled ones: they differ when Windows display scaling is above 100%.
        let d = screen_dpi_scale();
        cam.viewport = Some(((vx * d) as i32, (vy * d) as i32, (vw * d) as i32, (vh * d) as i32));
        set_camera(&cam);
        crate::render::set_screen(vx, vy, s, &cam);
        let (mx, my) = mouse_position();
        self.mouse = vec2((mx - vx) / s, (my - vy) / s);
        self.clicked = is_mouse_button_pressed(MouseButton::Left);
        self.right_clicked = is_mouse_button_pressed(MouseButton::Right);
        if let Some((m, l, r)) = crate::record::input() {
            (self.mouse, self.clicked, self.right_clicked) = (m, l, r);
        }
        self.camera = cam;
        draw_rectangle(0.0, 0.0, W, H, BG);
    }

    pub fn hover(&self, r: Rect) -> bool {
        r.contains(self.mouse)
    }

    pub fn button(&self, r: Rect, label: &str) -> bool {
        self.button_ex(r, label, true, false)
    }

    pub fn button_ex(&self, r: Rect, label: &str, enabled: bool, selected: bool) -> bool {
        let hover = enabled && self.hover(r);
        let bg = if !enabled {
            Color::new(0.12, 0.13, 0.17, 1.0)
        } else if selected {
            Color::new(0.45, 0.33, 0.12, 1.0)
        } else if hover {
            PANEL_HI
        } else {
            PANEL
        };
        draw_rectangle(r.x, r.y, r.w, r.h, bg);
        let border = if selected {
            ACCENT
        } else if hover {
            Color::new(0.5, 0.52, 0.65, 1.0)
        } else {
            LINE
        };
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, border);
        let size = if r.h < 32.0 { 18.0 } else { 22.0 };
        let dim = measure_text(label, None, size as u16, 1.0);
        let col = if enabled {
            TEXT
        } else {
            Color::new(0.4, 0.42, 0.5, 1.0)
        };
        draw_text(
            label,
            r.x + (r.w - dim.width) / 2.0,
            r.y + r.h / 2.0 + dim.offset_y / 2.0 - 1.0,
            size,
            col,
        );
        hover && self.clicked
    }

    #[allow(dead_code)]
    pub fn camera(&self) -> &Camera2D {
        &self.camera
    }
}

pub fn panel(r: Rect) {
    draw_rectangle(r.x, r.y, r.w, r.h, PANEL);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, LINE);
}

pub fn text(s: &str, x: f32, y: f32, size: f32, c: Color) {
    draw_text(s, x, y, size, c);
}

pub fn text_centered(s: &str, cx: f32, y: f32, size: f32, c: Color) {
    let d = measure_text(s, None, size as u16, 1.0);
    draw_text(s, cx - d.width / 2.0, y, size, c);
}

pub fn text_width(s: &str, size: f32) -> f32 {
    measure_text(s, None, size as u16, 1.0).width
}

pub fn bar(r: Rect, frac: f32, fg: Color) {
    draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.08, 0.09, 0.12, 1.0));
    draw_rectangle(r.x, r.y, r.w * frac.clamp(0.0, 1.0), r.h, fg);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, LINE);
}

pub fn rgb(c: [f32; 3]) -> Color {
    Color::new(c[0], c[1], c[2], 1.0)
}

pub fn star(cx: f32, cy: f32, r: f32, filled: bool) {
    let col = if filled {
        ACCENT
    } else {
        Color::new(0.25, 0.26, 0.33, 1.0)
    };
    let mut pts = Vec::with_capacity(10);
    for k in 0..10 {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
        let rr = if k % 2 == 0 { r } else { r * 0.45 };
        pts.push(vec2(cx + rr * a.cos(), cy + rr * a.sin()));
    }
    let c = vec2(cx, cy);
    for k in 0..10 {
        draw_triangle(c, pts[k], pts[(k + 1) % 10], col);
    }
}

/// Small line chart of 0..1 series.
pub fn chart(r: Rect, series: &[(&[f32], Color, &str)], x_label: &str) {
    panel(r);
    for k in 1..4 {
        let y = r.y + r.h * k as f32 / 4.0;
        draw_line(r.x, y, r.x + r.w, y, 1.0, Color::new(0.2, 0.21, 0.28, 1.0));
    }
    text("100%", r.x + 4.0, r.y + 14.0, 14.0, MUTED);
    text(
        x_label,
        r.x + r.w - text_width(x_label, 14.0) - 4.0,
        r.y + r.h - 4.0,
        14.0,
        MUTED,
    );
    let mut lx = r.x + 50.0;
    for (vals, col, name) in series {
        draw_rectangle(lx, r.y + 6.0, 10.0, 10.0, *col);
        text(name, lx + 14.0, r.y + 15.0, 15.0, TEXT);
        lx += text_width(name, 15.0) + 34.0;
        if vals.len() < 2 {
            continue;
        }
        let n = vals.len() as f32 - 1.0;
        for i in 1..vals.len() {
            let x0 = r.x + r.w * (i - 1) as f32 / n;
            let x1 = r.x + r.w * i as f32 / n;
            let y0 = r.y + r.h - r.h * vals[i - 1].clamp(0.0, 1.0);
            let y1 = r.y + r.h - r.h * vals[i].clamp(0.0, 1.0);
            draw_line(x0, y0, x1, y1, 2.0, *col);
        }
    }
}
