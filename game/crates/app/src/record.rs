//! Scripted screen recording for trailers and docs.
//!
//! `BAG_RECORD=out.mp4 BAG_TOUR=tour.txt` runs the game on a fixed 30 fps clock,
//! drives the mouse from the tour script and pipes every frame to ffmpeg, so the
//! video plays at real speed however slow the machine renders. Tour lines (virtual
//! 1280x800 coordinates, `#` starts a comment):
//!
//! ```text
//! wait 2            # seconds
//! move 640 400 0.6  # glide the cursor there over 0.6 s
//! click             # left click (also: rclick)
//! wheel 1           # one mouse-wheel notch (negative zooms out)
//! type 40           # types the text, one key per frame
//! backspace
//! ```
//!
//! Without `BAG_RECORD` every function here falls back to the real clock and input.

use macroquad::prelude::*;
use std::cell::RefCell;
use std::io::Write;
use std::process::{Child, Command, Stdio};

const FPS: f64 = 30.0;

enum Step {
    Wait(f32),
    Move(Vec2, f32),
    Click,
    RightClick,
    Wheel(f32),
    Type(String),
    Backspace,
}

struct Rec {
    out: String,
    ffmpeg: Option<Child>,
    steps: Vec<Step>,
    idx: usize,
    step_frame: u64,
    frame: u64,
    from: Vec2,
    mouse: Vec2,
    click: bool,
    right_click: bool,
    wheel: f32,
    typed: Option<char>,
    backspace: bool,
    typed_n: usize,
}

thread_local! {
    static REC: RefCell<Option<Rec>> = const { RefCell::new(None) };
}

/// Starts recording when BAG_RECORD is set.
pub fn start() {
    let Ok(out) = std::env::var("BAG_RECORD") else { return };
    let steps = std::env::var("BAG_TOUR")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse(&s))
        .unwrap_or_else(|| vec![Step::Wait(10.0)]);
    REC.with(|r| {
        *r.borrow_mut() = Some(Rec {
            out,
            ffmpeg: None,
            steps,
            idx: 0,
            step_frame: 0,
            frame: 0,
            from: vec2(640.0, 400.0),
            mouse: vec2(640.0, 400.0),
            click: false,
            right_click: false,
            wheel: 0.0,
            typed: None,
            backspace: false,
            typed_n: 0,
        })
    });
}

fn parse(src: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let w: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| w.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
        match w.first().copied() {
            Some("wait") => steps.push(Step::Wait(num(1))),
            Some("move") => steps.push(Step::Move(vec2(num(1), num(2)), num(3).max(0.05))),
            Some("click") => steps.push(Step::Click),
            Some("rclick") => steps.push(Step::RightClick),
            Some("wheel") => steps.push(Step::Wheel(num(1))),
            Some("type") => steps.push(Step::Type(w[1..].join(" "))),
            Some("backspace") => steps.push(Step::Backspace),
            Some(other) => eprintln!("tour: unknown step {other:?}"),
            None => {}
        }
    }
    steps
}

pub fn active() -> bool {
    REC.with(|r| r.borrow().is_some())
}

/// Seconds since start: frame-locked while recording.
pub fn now() -> f64 {
    REC.with(|r| match &*r.borrow() {
        Some(rec) => rec.frame as f64 / FPS,
        None => get_time(),
    })
}

/// Frame time: a fixed step while recording.
pub fn dt() -> f32 {
    if active() {
        (1.0 / FPS) as f32
    } else {
        get_frame_time()
    }
}

/// Scripted (virtual mouse, left click, right click) for this frame.
pub fn input() -> Option<(Vec2, bool, bool)> {
    REC.with(|r| r.borrow().as_ref().map(|rec| (rec.mouse, rec.click, rec.right_click)))
}

/// Characters typed this frame (scripted while recording).
pub fn chars() -> Vec<char> {
    let scripted = REC.with(|r| r.borrow().as_ref().map(|rec| rec.typed));
    match scripted {
        Some(c) => c.into_iter().collect(),
        None => {
            let mut v = Vec::new();
            while let Some(c) = get_char_pressed() {
                v.push(c);
            }
            v
        }
    }
}

/// Backspace pressed this frame.
pub fn backspace() -> bool {
    REC.with(|r| match &*r.borrow() {
        Some(rec) => rec.backspace,
        None => is_key_pressed(KeyCode::Backspace),
    })
}

/// Scripted wheel while recording, else the real one.
pub fn wheel() -> f32 {
    REC.with(|r| match &*r.borrow() {
        Some(rec) => rec.wheel,
        None => mouse_wheel().1,
    })
}

/// Draws the cursor, captures the frame and advances the tour.
/// Returns true once the tour is over and the video is written.
pub fn end_frame() -> bool {
    let Some(mouse) = input().map(|i| i.0) else { return false };
    draw_cursor(mouse);
    let img = get_screen_data();
    REC.with(|r| {
        let mut guard = r.borrow_mut();
        let rec = guard.as_mut().unwrap();
        if rec.ffmpeg.is_none() {
            rec.ffmpeg = Some(spawn_ffmpeg(&rec.out, img.width, img.height));
        }
        let stdin = rec.ffmpeg.as_mut().unwrap().stdin.as_mut().unwrap();
        if stdin.write_all(&img.bytes).is_err() {
            eprintln!("record: ffmpeg stopped");
            return true;
        }
        rec.frame += 1;
        advance(rec);
        if rec.idx >= rec.steps.len() {
            let mut child = rec.ffmpeg.take().unwrap();
            drop(child.stdin.take());
            let _ = child.wait();
            return true;
        }
        false
    })
}

fn advance(rec: &mut Rec) {
    rec.click = false;
    rec.right_click = false;
    rec.wheel = 0.0;
    rec.typed = None;
    rec.backspace = false;
    let Some(step) = rec.steps.get(rec.idx) else { return };
    let since = (rec.frame - rec.step_frame) as f32 / FPS as f32;
    let next = match *step {
        Step::Wait(s) => since >= s,
        Step::Move(to, s) => {
            let t = (since / s).min(1.0);
            let e = t * t * (3.0 - 2.0 * t);
            rec.mouse = rec.from.lerp(to, e);
            t >= 1.0
        }
        Step::Click => {
            rec.click = true;
            true
        }
        Step::RightClick => {
            rec.right_click = true;
            true
        }
        Step::Wheel(n) => {
            rec.wheel = n;
            true
        }
        Step::Type(ref text) => {
            let c = text.chars().nth(rec.typed_n);
            rec.typed = c;
            rec.typed_n += 1;
            if rec.typed_n >= text.chars().count() {
                rec.typed_n = 0;
                true
            } else {
                false
            }
        }
        Step::Backspace => {
            rec.backspace = true;
            true
        }
    };
    if next {
        rec.idx += 1;
        rec.step_frame = rec.frame;
        rec.from = rec.mouse;
    }
}

fn spawn_ffmpeg(out: &str, w: u16, h: u16) -> Child {
    Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-s", &format!("{w}x{h}"), "-r", "30", "-i", "-"])
        // GL reads rows bottom-up.
        .args(["-vf", "vflip", "-c:v", "libx264", "-preset", "medium", "-crf", "20"])
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart", out])
        .stdin(Stdio::piped())
        .spawn()
        .expect("record: ffmpeg not found")
}

/// A plain arrow pointer, since the window cursor is not in the capture.
fn draw_cursor(p: Vec2) {
    let pts = [vec2(0.0, 0.0), vec2(0.0, 22.0), vec2(6.0, 17.0), vec2(10.5, 26.0), vec2(14.0, 24.5), vec2(9.5, 15.5), vec2(17.0, 15.5)];
    let at = |v: Vec2| p + v;
    let black = Color::new(0.05, 0.05, 0.08, 1.0);
    for (a, b, c) in [(0, 1, 2), (0, 2, 6), (2, 5, 6), (2, 3, 5), (3, 4, 5)] {
        draw_triangle(at(pts[a]), at(pts[b]), at(pts[c]), WHITE);
    }
    for i in 0..pts.len() {
        draw_line(at(pts[i]).x, at(pts[i]).y, at(pts[(i + 1) % pts.len()]).x, at(pts[(i + 1) % pts.len()]).y, 1.5, black);
    }
}
