//! The journal: what the game remembers about the player's story, kept in
//! `journal.json` next to the profile (the test save has its own).
//!
//! - Each troop's service record: battles, wins, kills, best moment and the
//!   title it earns from how it really fights.
//! - What each troop knew when the player last looked, for the "while you
//!   were away" report.
//! - Goals already claimed, friend challenges, and time played per day.
//!
//! Nothing here changes how a battle plays out; losing the file only loses
//! the story, never a troop.

use crate::Game;
use godcore::sim::{Kind, Unit, World};
use godcore::troop::{unix_now, Troop};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How many recent battles a service record keeps.
const LOG_LEN: usize = 12;

/// Battles before a troop earns a title.
pub const TITLE_AFTER: u32 = 3;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Journal {
    /// Service records by troop id.
    pub records: BTreeMap<u64, Record>,
    /// What each troop knew when the player last saw the report, by id.
    pub seen: BTreeMap<u64, Seen>,
    /// Goals whose reward has been collected.
    pub claimed: Vec<String>,
    pub challenges_sent: u32,
    pub challenges_played: u32,
    pub challenges_won: u32,
    /// Seconds played on each day (days since 1970, UTC).
    pub played: BTreeMap<u64, u64>,
    /// "Before and after" views watched (the first is part of the tour).
    pub lessons_watched: u32,
    /// Where this journal is saved. Not saved itself.
    #[serde(skip)]
    pub dir: PathBuf,
    #[serde(skip)]
    dirty: bool,
}

/// One troop's career.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// When the journal first saw this troop (unix seconds).
    pub since: u64,
    pub battles: u32,
    pub wins: u32,
    /// Battles it didn't walk away from.
    pub fell: u32,
    pub kills: u32,
    pub unit_dmg: f32,
    pub building_dmg: f32,
    pub heal: f32,
    pub dodges: u32,
    pub evaded: u32,
    pub blocked: f32,
    pub parried: u32,
    pub calls: u32,
    pub missed: u32,
    pub skills: u32,
    pub surprised: u32,
    /// Ticks spent out of breath.
    pub winded: u32,
    pub best: Option<Moment>,
    /// The most recent battles, newest last.
    pub log: Vec<Entry>,
}

/// A battle worth remembering.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Moment {
    pub when: u64,
    pub what: String,
    pub line: String,
    pub score: f32,
}

/// One line of the battle log.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub when: u64,
    pub what: String,
    pub won: bool,
    pub kills: u32,
    pub dmg: f32,
    pub heal: f32,
    pub fell: bool,
}

/// What a troop knew at one moment.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Seen {
    pub steps: u64,
    pub win: f32,
    pub survival: f32,
    pub level: u32,
    pub sessions: u32,
    pub badges: Vec<String>,
    pub mastery: u8,
}

impl Seen {
    pub fn of(t: &Troop) -> Seen {
        Seen {
            steps: t.steps,
            win: t.skills.win,
            survival: t.skills.survival,
            level: t.level,
            sessions: t.sessions,
            badges: t.skills.badges(&t.spec).into_iter().filter(|(_, ok)| *ok).map(|(l, _)| l.to_string()).collect(),
            mastery: t.spec.mastery,
        }
    }
}

/// Today, as days since 1970 (UTC).
pub fn today() -> u64 {
    unix_now() / 86_400
}

impl Journal {
    pub fn load(dir: &std::path::Path) -> Journal {
        let mut j = crate::save::load_file(&dir.join("journal.json"), |s| serde_json::from_str::<Journal>(s).ok())
            .map(|(j, _)| j)
            .unwrap_or_default();
        j.dir = dir.to_path_buf();
        j
    }

    pub fn save(&mut self) {
        self.dirty = false;
        if self.dir.as_os_str().is_empty() {
            return;
        }
        crate::save::save_file(&self.dir.join("journal.json"), serde_json::to_string(self).unwrap().as_bytes());
    }

    /// Note a change to save at the next quiet moment.
    pub fn touch(&mut self) {
        self.dirty = true;
    }

    pub fn record(&self, id: u64) -> Option<&Record> {
        self.records.get(&id)
    }

    /// Seconds played over the last `days` days, today included.
    pub fn played_last(&self, days: u64) -> u64 {
        let t = today();
        self.played.range(t.saturating_sub(days - 1)..=t).map(|(_, s)| s).sum()
    }

    /// Count `secs` of play today. Days older than two weeks are dropped.
    pub fn add_play(&mut self, secs: u64) {
        let t = today();
        *self.played.entry(t).or_default() += secs;
        self.played.retain(|d, _| *d + 14 >= t);
    }
}

/// How one troop did in one battle.
pub fn score(u: &Unit) -> f32 {
    u.stats.kills as f32 * 20.0 + u.stats.unit_dmg + u.stats.building_dmg * 0.5 + u.stats.heal + u.stats.blocked * 0.5
}

impl Record {
    /// Add one battle. `what` names it ("Raid on Fort").
    pub fn add(&mut self, u: &Unit, what: &str, won: bool, now: u64) {
        let s = &u.stats;
        self.battles += 1;
        self.wins += won as u32;
        self.fell += (!u.alive) as u32;
        self.kills += s.kills;
        self.unit_dmg += s.unit_dmg;
        self.building_dmg += s.building_dmg;
        self.heal += s.heal;
        self.dodges += s.dodges;
        self.evaded += s.evaded;
        self.blocked += s.blocked;
        self.parried += s.parried;
        self.calls += s.calls;
        self.missed += s.missed;
        self.skills += s.skills;
        self.surprised += s.surprised;
        self.winded += s.winded;
        let sc = score(u) + if won { 15.0 } else { 0.0 };
        if self.best.as_ref().is_none_or(|b| sc > b.score) && sc > 0.0 {
            self.best = Some(Moment { when: now, what: what.to_string(), line: moment_line(u, won), score: sc });
        }
        self.log.push(Entry {
            when: now,
            what: what.to_string(),
            won,
            kills: s.kills,
            dmg: s.unit_dmg + s.building_dmg,
            heal: s.heal,
            fell: !u.alive,
        });
        let cut = self.log.len().saturating_sub(LOG_LEN);
        self.log.drain(..cut);
    }

    /// The title this troop has earned from how it fights, once it has
    /// fought enough to tell: (title, why).
    pub fn title(&self) -> Option<(&'static str, String)> {
        if self.battles < TITLE_AFTER {
            return None;
        }
        let n = self.battles as f32;
        let lived = 1.0 - self.fell as f32 / n;
        let dmg = (self.unit_dmg + self.building_dmg).max(1.0);
        // Each style scored per battle, scaled so 1.0 is "a lot".
        let styles: [(&'static str, f32, String); 10] = [
            ("The Patient Healer", self.heal / n / 60.0, format!("healed {:.0} health in {} battles", self.heal, self.battles)),
            ("The Wall Breaker", self.building_dmg / n / 120.0 * (self.building_dmg / dmg), format!("{:.0} damage to buildings", self.building_dmg)),
            ("The Hunter", self.kills as f32 / n / 1.2, format!("{} enemies brought down", self.kills)),
            ("The Dancer", (self.dodges + self.evaded) as f32 / n / 4.0, format!("{} dodges, {} hits missed it", self.dodges, self.evaded)),
            ("The Shield", self.blocked / n / 40.0, format!("blocked {:.0} damage", self.blocked)),
            ("The Duelist", self.parried as f32 / n / 2.0, format!("{} blows parried", self.parried)),
            ("The Voice of the Squad", self.calls as f32 / n / 6.0, format!("{} calls to its team", self.calls)),
            ("The Survivor", if lived >= 0.8 && self.battles >= 4 { lived * 0.9 } else { 0.0 }, format!("came home from {} of {} battles", self.battles - self.fell, self.battles)),
            ("The Daredevil", if lived < 0.4 { (1.0 - lived) * (self.unit_dmg / n / 60.0).min(1.2) } else { 0.0 }, format!("charges in first: fell in {} of {} battles", self.fell, self.battles)),
            ("The Spellslinger", self.skills as f32 / n / 3.0, format!("used its weapon skill {} times", self.skills)),
        ];
        let best = styles.into_iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;
        Some(if best.1 >= 0.35 { (best.0, best.2) } else { ("The Steady One", "does a bit of everything".to_string()) })
    }

    /// One plain line about its career so far.
    pub fn summary(&self) -> String {
        if self.battles == 0 {
            return "No battles yet".to_string();
        }
        format!(
            "{} battle{}, {} won, {} enem{} down",
            self.battles,
            if self.battles == 1 { "" } else { "s" },
            self.wins,
            self.kills,
            if self.kills == 1 { "y" } else { "ies" }
        )
    }
}

/// "3 enemies down and the Town Hall hit" style line for a best moment.
fn moment_line(u: &Unit, won: bool) -> String {
    let s = &u.stats;
    let mut bits = Vec::new();
    if s.kills > 0 {
        bits.push(format!("{} enem{} down", s.kills, if s.kills == 1 { "y" } else { "ies" }));
    }
    if s.building_dmg >= 1.0 {
        bits.push(format!("{:.0} damage to buildings", s.building_dmg));
    }
    if s.heal >= 1.0 {
        bits.push(format!("{:.0} health healed", s.heal));
    }
    if s.dodges + s.evaded > 2 {
        bits.push(format!("{} dodges", s.dodges + s.evaded));
    }
    if bits.is_empty() {
        bits.push(format!("{:.0} damage dealt", s.unit_dmg));
    }
    let end = match (won, u.alive) {
        (true, true) => ", and it walked home a winner",
        (true, false) => ", and it fell, but the squad won",
        (false, true) => ", and it made it home",
        (false, false) => "",
    };
    format!("{}{end}", bits.join(", "))
}

/// "12 days", "3 hours" since `since`.
pub fn age_words(since: u64, now: u64) -> String {
    let s = now.saturating_sub(since);
    let (n, unit) = if s >= 2 * 86_400 {
        (s / 86_400, "day")
    } else if s >= 2 * 3600 {
        (s / 3600, "hour")
    } else {
        ((s / 60).max(1), "minute")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

impl Game {
    /// Keep the journal on the save in use, start records for new troops,
    /// count play time, and write changes now and then. Called every frame.
    pub fn tick_journal(&mut self) {
        if self.journal.dir != self.store.dir {
            if self.journal.dirty {
                self.journal.save();
            }
            self.journal = Journal::load(&self.store.dir);
            // A save opened for the first time starts "seen" where it is now.
            for t in &self.troops {
                self.journal.seen.entry(t.id).or_insert_with(|| Seen::of(t));
            }
            self.journal.touch();
        }
        let now = unix_now();
        for t in &self.troops {
            if !self.journal.records.contains_key(&t.id) {
                self.journal.records.insert(t.id, Record { since: now, ..Default::default() });
                self.journal.seen.entry(t.id).or_insert_with(|| Seen::of(t));
                self.journal.dirty = true;
            }
        }
        self.tick_play_time();
        if self.journal.dirty && (macroquad::time::get_time() - self.journal_saved_at > 20.0 || self.troops.is_empty()) {
            self.journal_saved_at = macroquad::time::get_time();
            self.journal.save();
        }
    }

    /// Add a finished battle to the service records of the troops `mine`
    /// picks (units tagged with their index in `troops`). `won` is from
    /// those troops' side.
    pub fn note_battle(&mut self, w: &World, troops: &[Troop], mine: &dyn Fn(&Unit) -> bool, what: &str, won: bool) {
        let now = unix_now();
        for u in w.units.iter().filter(|u| u.kind == Kind::Troop && u.control == godcore::sim::Control::External && u.tag < troops.len() && mine(u)) {
            let id = troops[u.tag].id;
            let rec = self.journal.records.entry(id).or_insert_with(|| Record { since: now, ..Default::default() });
            let had = rec.title().map(|t| t.0);
            rec.add(u, what, won, now);
            let has = rec.title().map(|t| t.0);
            if has.is_some() && has != had {
                let name = troops[u.tag].name.trim().to_string();
                self.journal_news.push(match had {
                    None => format!("{name} earned a title: {}", has.unwrap()),
                    Some(_) => format!("{name} is now known as {}", has.unwrap()),
                });
            }
        }
        self.journal.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Unit {
        let mut w = World::new(1);
        let i = w.add_troop(&godcore::classes::BaseClass::Healer.spec(), 5.0, 5.0, godcore::sim::Control::External, 0);
        w.units[i].clone()
    }

    #[test]
    fn a_healer_that_heals_earns_its_title_after_three_battles() {
        let mut r = Record::default();
        let mut u = unit();
        u.stats.heal = 90.0;
        r.add(&u, "Raid on Fort", true, 10);
        r.add(&u, "Raid on Fort", false, 20);
        assert!(r.title().is_none(), "too early to tell");
        r.add(&u, "Raid on Outpost", true, 30);
        assert_eq!(r.title().unwrap().0, "The Patient Healer");
        assert_eq!(r.wins, 2);
        assert_eq!(r.log.len(), 3);
        assert!(r.best.is_some());
    }

    #[test]
    fn the_log_keeps_only_recent_battles_and_old_journals_load() {
        let mut r = Record::default();
        let u = unit();
        for k in 0..30 {
            r.add(&u, "Raid", false, k);
        }
        assert_eq!(r.log.len(), LOG_LEN);
        assert_eq!(r.log.last().unwrap().when, 29);
        let j: Journal = serde_json::from_str("{}").unwrap();
        assert!(j.records.is_empty());
        let mut j = Journal::default();
        j.records.insert(7, r);
        let back: Journal = serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert_eq!(back.records[&7].battles, 30);
    }

    #[test]
    fn play_time_adds_up_by_day() {
        let mut j = Journal::default();
        j.add_play(600);
        j.add_play(60);
        assert_eq!(j.played_last(1), 660);
        assert_eq!(j.played_last(7), 660);
    }
}
