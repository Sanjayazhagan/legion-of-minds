//! Training in the background, like a builder timer: sessions belong to the
//! game rather than to a screen, so the player can raid, build or train other
//! troops while they run. Only the troops being trained are busy: they are
//! greyed out in every squad picker until their session ends.
//!
//! Closing the game (or switching saves) pauses every session and keeps what
//! it learned; the next start picks them up again where they stopped.

use crate::save::Resume;
use crate::ui::*;
use crate::Game;
use macroquad::prelude::*;

impl Game {
    /// Where the troop with this id is in the roster.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.troops.iter().position(|t| t.id == id)
    }

    /// The team session the troop with this id is part of, if any.
    pub fn team_run_of(&self, id: u64) -> Option<usize> {
        self.team_runs.iter().position(|r| r.ids.contains(&id))
    }

    /// Troop `i` is training (alone or with a team), so it can't fight,
    /// level up, change gear or be released until that ends.
    pub fn is_busy(&self, i: usize) -> bool {
        self.busy_label(i).is_some()
    }

    /// A short note for a busy troop's card: "Training 45%".
    pub fn busy_label(&self, i: usize) -> Option<String> {
        let id = self.troops.get(i)?.id;
        if let Some(r) = self.solo_runs.iter().find(|r| r.id == id) {
            return Some(format!("Training {:.0}%", r.progress() * 100.0));
        }
        self.team_run_of(id)
            .map(|k| format!("Team training {:.0}%", self.team_runs[k].progress() * 100.0))
    }

    /// Any troop training right now.
    pub fn training_count(&self) -> usize {
        self.solo_runs.len() + self.team_runs.iter().map(|r| r.ids.len()).sum::<usize>()
    }

    /// Called once a frame, whatever screen is showing: save running
    /// sessions every couple of minutes and wrap up the ones that ended.
    pub fn tick_training(&mut self) {
        let before = self.solo_runs.len() + self.team_runs.len();
        self.tick_solo();
        self.tick_team();
        if before > 0 {
            self.sync_training_list();
        }
        self.tick_moments();
    }

    /// What is running now, as the profile stores it.
    fn running_list(&self) -> Vec<Resume> {
        let solo = self.solo_runs.iter().map(|r| Resume::Solo { id: r.id, target: r.target(), from: r.from });
        let team = self.team_runs.iter().map(|r| Resume::Team {
            ids: r.ids.clone(),
            left: r.left(),
            hardness: r.hardness,
            done: r.done_before + r.done(),
        });
        solo.chain(team).collect()
    }

    /// Keep the profile's list of running sessions up to date on disk, so
    /// even a crash carries on with them next time.
    fn sync_training_list(&mut self) {
        let v = self.running_list();
        if v != self.profile.training {
            self.profile.training = v;
            self.store.save_profile(&self.profile);
        }
    }

    /// Stop every session, keep what it learned, and note in the profile
    /// what to carry on with next time. Saves the profile.
    pub fn pause_all_training(&mut self) {
        if self.solo_runs.is_empty() && self.team_runs.is_empty() {
            // Nothing running: a list not picked up yet stays as it is.
            return;
        }
        let mut v = self.pause_solo();
        v.extend(self.pause_team());
        self.profile.training = v;
        self.store.save_profile(&self.profile);
    }

    /// Start again the sessions the profile says were running when the
    /// game closed. Sessions whose troops are gone or capped are dropped.
    pub fn resume_training(&mut self) {
        let list = std::mem::take(&mut self.profile.training);
        let n = list.len();
        for r in list {
            match r {
                Resume::Solo { id, target, from } => {
                    if let Some(i) = self.index_of(id).filter(|&i| !self.is_busy(i)) {
                        self.start_solo(i, target, Some(from));
                    }
                }
                Resume::Team { ids, left, hardness, done } => {
                    let Some(members) = ids.iter().map(|&id| self.index_of(id)).collect::<Option<Vec<_>>>() else { continue };
                    if members.iter().any(|&i| self.is_busy(i)) {
                        continue;
                    }
                    let room = members.iter().map(|&i| self.troops[i].steps_left()).min().unwrap_or(0);
                    if room > 0 {
                        self.start_team(&members, left.min(room), hardness, done);
                    }
                }
            }
        }
        if n > 0 {
            self.sync_training_list();
            let busy = self.training_count();
            if busy > 0 {
                self.toast(format!(
                    "Carrying on with last time's training ({busy} troop{})",
                    if busy == 1 { "" } else { "s" }
                ));
            }
        }
    }
}

/// Grey out a troop's card in a picker and say why it can't be picked.
pub fn busy_veil(r: Rect, label: &str) {
    draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.04, 0.045, 0.07, 0.72));
    let size = 16.0;
    let w = text_width(label, size) + 16.0;
    let tag = Rect::new(r.x + r.w - w - 6.0, r.y + 5.0, w, 22.0);
    draw_rectangle(tag.x, tag.y, tag.w, tag.h, Color::new(0.12, 0.2, 0.32, 1.0));
    draw_rectangle_lines(tag.x, tag.y, tag.w, tag.h, 1.0, SKY);
    text(label, tag.x + 8.0, tag.y + 16.0, size, TEXT);
}
