//! Terms of Service and Privacy Policy: shown at first launch (and again
//! when either changes) until the player agrees, and readable any time from
//! Settings. The text lives in /legal at the top of the repository, so the
//! launch page and store pages use the same words.

use crate::ui::*;
use crate::Game;
use macroquad::prelude::*;

pub const TERMS: &str = include_str!("../../../../legal/TERMS_OF_SERVICE.md");
pub const PRIVACY: &str = include_str!("../../../../legal/PRIVACY_POLICY.md");

/// Raise this when either document changes in a way players must agree to
/// again. It is the "Version" date at the top of both files.
pub const VERSION: &str = "2026-11-24";

/// One laid-out line of a document.
struct Line {
    text: String,
    x: f32,
    size: f32,
    color: Color,
    /// Space above this line.
    gap: f32,
}

pub struct LegalView {
    /// Must agree before playing (first launch or new terms), or only reading.
    pub must_agree: bool,
    tab: usize,
    scroll: f32,
    lines: Vec<Line>,
    laid_out_for: Option<usize>,
}

impl LegalView {
    pub fn new(must_agree: bool) -> Self {
        LegalView { must_agree, tab: 0, scroll: 0.0, lines: Vec::new(), laid_out_for: None }
    }
}

/// Turn the light Markdown in the documents into wrapped lines.
fn layout(doc: &str, width: f32) -> Vec<Line> {
    let mut out = Vec::new();
    let mut para = String::new();
    let mut para_style: Option<(f32, f32, Color, &str)> = None;
    let flush = |out: &mut Vec<Line>, para: &mut String, style: &mut Option<(f32, f32, Color, &str)>| {
        let Some((x, size, color, lead)) = style.take() else { return };
        let words = clean(para);
        para.clear();
        let mut line = String::new();
        let mut first = true;
        let push = |out: &mut Vec<Line>, line: String, first: bool| {
            let gap = if !first {
                0.0
            } else if size >= 24.0 {
                18.0
            } else if size >= 20.0 {
                16.0
            } else if lead.is_empty() {
                10.0
            } else {
                4.0
            };
            let (text, x) = if first { (format!("{lead}{line}"), x) } else { (line, x + text_width(lead, size)) };
            out.push(Line { text, x, size, color, gap });
        };
        let avail = width - x - text_width(lead, size);
        for word in words.split_whitespace() {
            let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if text_width(&trial, size) > avail && !line.is_empty() {
                push(out, std::mem::replace(&mut line, word.to_string()), first);
                first = false;
            } else {
                line = trial;
            }
        }
        if !line.is_empty() {
            push(out, line, first);
        }
    };
    for raw in doc.lines() {
        let l = raw.trim_end();
        let style = if let Some(h) = l.strip_prefix("# ") {
            Some((h, (0.0, 28.0, ACCENT, "")))
        } else if let Some(h) = l.strip_prefix("## ") {
            Some((h, (0.0, 22.0, TEXT, "")))
        } else if let Some(h) = l.strip_prefix("- ") {
            Some((h, (12.0, 17.0, TEXT, "-  ")))
        } else {
            None
        };
        match style {
            Some((body, s)) => {
                flush(&mut out, &mut para, &mut para_style);
                para.push_str(body);
                para_style = Some(s);
                // Headings are always one paragraph on their own.
                if s.1 >= 20.0 {
                    flush(&mut out, &mut para, &mut para_style);
                }
            }
            None if l.trim().is_empty() => flush(&mut out, &mut para, &mut para_style),
            None => {
                if para_style.is_none() {
                    para_style = Some((0.0, 17.0, TEXT, ""));
                }
                para.push(' ');
                para.push_str(l.trim());
            }
        }
    }
    flush(&mut out, &mut para, &mut para_style);
    out
}

/// Strip Markdown emphasis and show links as "text (address)".
fn clean(s: &str) -> String {
    let mut out = s.replace("**", "").replace('`', "");
    while let Some(m) = out.find("](") {
        let (Some(a), Some(e)) = (out[..m].rfind('['), out[m..].find(')').map(|e| m + e)) else { break };
        let link = format!("{} ({})", &out[a + 1..m], &out[m + 2..e]);
        out.replace_range(a..=e, &link);
    }
    out
}

const SUMMARY: [&str; 5] = [
    "Offline play sends nothing to us. Your troops and saves stay on this computer.",
    "Online play needs a Google or Discord sign-in, and is for ages 13 and up (under 18 with a parent's OK).",
    "Online, your published base, your armies and their trained brains are kept on the server and other players fight them.",
    "Sharing play data to improve the game is optional and stays off unless you turn it on.",
    "Play fair: no cheats, bots or abuse. You can delete your online account in Settings at any time.",
];

impl Game {
    /// The full-screen Terms and Privacy page.
    pub fn legal_screen(&mut self) {
        let Some(mut v) = self.legal.take() else { return };
        draw_rectangle(0.0, 0.0, W, 52.0, Color::new(0.1, 0.11, 0.155, 1.0));
        draw_line(0.0, 52.0, W, 52.0, 1.0, LINE);
        text("LEGION OF MINDS", 20.0, 34.0, 28.0, ACCENT);
        text(if v.must_agree { "Before you play" } else { "Terms and privacy" }, 250.0, 34.0, 24.0, TEXT);

        // ---- Plain-words summary
        let s = Rect::new(20.0, 70.0, 380.0, 610.0);
        panel(s);
        text("In short", s.x + 22.0, s.y + 38.0, 24.0, TEXT);
        let mut y = s.y + 76.0;
        for p in SUMMARY {
            draw_circle(s.x + 26.0, y - 5.0, 4.0, ACCENT);
            y = wrap_h(p, s.x + 40.0, y, s.w - 62.0, 17.0, TEXT) + 34.0;
        }
        wrap_h(
            "This summary is not the full agreement: the documents on the right are.",
            s.x + 22.0,
            s.y + s.h - 40.0,
            s.w - 44.0,
            15.0,
            MUTED,
        );

        // ---- The documents
        let d = Rect::new(420.0, 70.0, 840.0, 610.0);
        panel(d);
        for (k, label) in ["Terms of Service", "Privacy Policy"].into_iter().enumerate() {
            let r = Rect::new(d.x + 20.0 + k as f32 * 210.0, d.y + 16.0, 200.0, 38.0);
            if self.ui.button_ex(r, label, true, v.tab == k) && v.tab != k {
                v.tab = k;
                v.scroll = 0.0;
            }
        }
        let body = Rect::new(d.x + 24.0, d.y + 74.0, d.w - 64.0, d.h - 90.0);
        if v.laid_out_for != Some(v.tab) {
            v.lines = layout(if v.tab == 0 { TERMS } else { PRIVACY }, body.w);
            v.laid_out_for = Some(v.tab);
        }
        let total: f32 = v.lines.iter().map(|l| l.gap + l.size * 1.3).sum();
        let max_scroll = (total - body.h + 20.0).max(0.0);
        if self.ui.hover(d) {
            v.scroll -= crate::record::wheel().signum() * 60.0;
        }
        if is_key_pressed(KeyCode::PageDown) || is_key_pressed(KeyCode::Space) {
            v.scroll += body.h - 40.0;
        }
        if is_key_pressed(KeyCode::PageUp) {
            v.scroll -= body.h - 40.0;
        }
        if is_key_pressed(KeyCode::Down) {
            v.scroll += 40.0;
        }
        if is_key_pressed(KeyCode::Up) {
            v.scroll -= 40.0;
        }
        v.scroll = v.scroll.clamp(0.0, max_scroll);
        let mut y = body.y - v.scroll;
        for l in &v.lines {
            y += l.gap + l.size * 1.3;
            // Only whole lines inside the panel are drawn.
            if y - l.size >= body.y && y <= body.y + body.h {
                text(&l.text, body.x + l.x, y, l.size, l.color);
            }
        }
        // Scroll bar.
        if max_scroll > 0.0 {
            let track = Rect::new(d.x + d.w - 24.0, body.y, 6.0, body.h);
            draw_rectangle(track.x, track.y, track.w, track.h, PANEL_HI);
            let h = (body.h / total * track.h).max(30.0);
            let ty = track.y + (track.h - h) * (v.scroll / max_scroll);
            draw_rectangle(track.x, ty, track.w, h, LINE);
            if self.ui.clicked && self.ui.hover(Rect::new(track.x - 8.0, track.y, 22.0, track.h)) {
                v.scroll = ((self.ui.mouse.y - track.y) / track.h * total - body.h / 2.0).clamp(0.0, max_scroll);
            }
        }

        // ---- Agree, quit or close
        let b = Rect::new(20.0, 696.0, 1240.0, 84.0);
        panel(b);
        if v.must_agree {
            wrap_h(
                "By choosing I agree, you accept the Terms of Service and the Privacy Policy. Playing online needs you to be 13 or older (under 18, with a parent or guardian's agreement).",
                b.x + 24.0,
                b.y + 32.0,
                780.0,
                16.0,
                TEXT,
            );
            if self.ui.button(Rect::new(b.x + b.w - 410.0, b.y + 20.0, 170.0, 44.0), "Quit") {
                self.quit = true;
            }
            if self.ui.button_ex(Rect::new(b.x + b.w - 220.0, b.y + 20.0, 196.0, 44.0), "I agree", true, true) {
                self.settings.accepted_terms = Some(VERSION.to_string());
                self.settings.save(&self.store.home);
                return;
            }
        } else {
            text(&format!("Version {VERSION}. Questions: see Contact at the end of each document."), b.x + 24.0, b.y + 48.0, 16.0, MUTED);
            if self.ui.button(Rect::new(b.x + b.w - 220.0, b.y + 20.0, 196.0, 44.0), "Close") || is_key_pressed(KeyCode::Escape) {
                return;
            }
        }
        self.legal = Some(v);
    }

    /// Whether the current Terms still need the player's agreement.
    pub fn terms_needed(&self) -> bool {
        self.settings.accepted_terms.as_deref() != Some(VERSION)
    }
}

/// Wrapped text that returns the y below its last line.
fn wrap_h(s: &str, x: f32, y: f32, w: f32, size: f32, c: Color) -> f32 {
    let mut line = String::new();
    let mut yy = y;
    for word in s.split_whitespace() {
        let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if text_width(&trial, size) > w && !line.is_empty() {
            text(&line, x, yy, size, c);
            yy += size * 1.3;
            line = word.to_string();
        } else {
            line = trial;
        }
    }
    if !line.is_empty() {
        text(&line, x, yy, size, c);
    }
    yy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_documents_carry_the_current_version() {
        for doc in [TERMS, PRIVACY] {
            assert!(doc.contains(&format!("Version {VERSION}.")), "bump VERSION with the documents");
        }
    }

    #[test]
    fn links_read_as_text() {
        assert_eq!(clean("see [Settings](https://x.y/z) now"), "see Settings (https://x.y/z) now");
        assert_eq!(clean("**bold** and `code`"), "bold and code");
        assert_eq!(clean("a [b] c"), "a [b] c");
    }
}
