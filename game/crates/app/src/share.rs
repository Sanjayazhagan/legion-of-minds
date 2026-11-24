//! Sharing a saved video: copy the file itself (so it pastes straight into
//! a chat or a post), show it in its folder, or open X, Reddit or Discord
//! with the words already written. A video can't be attached through a web
//! link, so the quick-post buttons copy the file first: the player pastes it
//! into the page that opens.

use crate::Game;
use macroquad::prelude::*;
use std::path::Path;

/// The words that go with a shared video.
pub fn post_text(line: &str) -> String {
    format!("{line} I trained this troop's brain myself in Legion of Minds. #LegionOfMinds #gamedev #AI")
}

/// The share buttons, in the order `Game::share_action` takes them.
pub const ACTIONS: [&str; 6] = ["Copy video", "Show in folder", "Post on X", "Post on Reddit", "Post on Discord", "Copy text"];

/// The share buttons, laid out in `r` (two rows of three).
pub fn share_buttons(g: &mut Game, r: Rect, video: &Path, words: &str) {
    let bw = (r.w - 20.0) / 3.0;
    for (k, label) in ACTIONS.iter().enumerate() {
        let at = Rect::new(r.x + (k % 3) as f32 * (bw + 10.0), r.y + (k / 3) as f32 * 56.0, bw, 46.0);
        if g.ui.button(at, label) {
            g.share_action(k, video, words);
        }
    }
}

impl Game {
    /// Share button `k` of `ACTIONS`.
    pub fn share_action(&mut self, k: usize, video: &Path, words: &str) {
        match k {
            0 => match copy_file(video) {
                Ok(()) => self.toast("Video copied: paste it into a chat or a post"),
                Err(e) => self.toast(format!("Could not copy the video ({e}). Use Show in folder.")),
            },
            1 => show_in_folder(video),
            5 => {
                macroquad::miniquad::window::clipboard_set(words);
                self.toast("Text copied");
            }
            _ => {
                let url = match k {
                    2 => format!("https://x.com/intent/post?text={}", url_encode(words)),
                    3 => format!("https://www.reddit.com/submit?title={}", url_encode(words)),
                    _ => "https://discord.com/channels/@me".to_string(),
                };
                // A web link can't carry a video: copy the file to paste in.
                let copied = copy_file(video).is_ok();
                if !copied {
                    macroquad::miniquad::window::clipboard_set(words);
                }
                open_url(&url);
                self.toast(if copied {
                    "Video copied: paste it (Ctrl+V) into the post that just opened"
                } else {
                    "Opened. Drag the video in from its folder (Show in folder)"
                });
            }
        }
    }
}

/// `%`-encode everything but letters, digits and `-_.~`.
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Open a web page in the player's browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let r = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        crate::clip::no_window(&mut c);
        c.spawn()
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        eprintln!("could not open {url}: {e}");
    }
}

/// Put the file itself on the clipboard, the way copying it in the file
/// browser does, so it pastes as an attachment.
pub fn copy_file(p: &Path) -> Result<(), String> {
    let p = p.canonicalize().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let status = {
        // PowerShell 5 (in every Windows 10 and 11) can do it. The \\?\
        // prefix canonicalize adds is dropped; quotes are doubled.
        let path = p.display().to_string();
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path).replace('\'', "''");
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", &format!("Set-Clipboard -LiteralPath '{path}'")]);
        crate::clip::no_window(&mut c);
        c.status()
    };
    #[cfg(target_os = "macos")]
    let status = {
        let path = p.display().to_string().replace('\\', "\\\\").replace('"', "\\\"");
        std::process::Command::new("osascript").args(["-e", &format!("set the clipboard to (POSIX file \"{path}\")")]).status()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = {
        use std::io::Write;
        let uri = format!("file://{}\n", p.display());
        std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                c.stdin.take().unwrap().write_all(uri.as_bytes())?;
                c.wait()
            })
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("the clipboard refused it".into()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn show_in_folder(p: &Path) {
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

#[cfg(test)]
mod tests {
    #[test]
    fn url_encoding() {
        assert_eq!(super::url_encode("Sol learned! #AI"), "Sol%20learned%21%20%23AI");
    }
}
