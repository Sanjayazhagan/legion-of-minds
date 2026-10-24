//! Save data: the player's profile and one file per troop.

use godcore::troop::Troop;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const START_GOLD: u32 = 300;
pub const RECRUIT_COST: u32 = 50;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub gold: u32,
    pub best_stars: Vec<u32>,
    pub next_id: u64,
    pub battles: u32,
    pub victories: u32,
    #[serde(default)]
    pub waves_held: Vec<bool>,
    #[serde(default)]
    pub base: godcore::scenario::HomeBase,
    /// Weapons bought in the armoury (class weapons are always free).
    #[serde(default)]
    pub weapons: Vec<godcore::weapons::Weapon>,
    /// Set on the test-mode save: gold never runs out and cheats are open.
    #[serde(default)]
    pub test: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            gold: START_GOLD,
            best_stars: vec![0; godcore::scenario::BASES.len()],
            next_id: 1,
            battles: 0,
            victories: 0,
            waves_held: vec![false; godcore::scenario::WAVES.len()],
            base: Default::default(),
            weapons: Vec::new(),
            test: false,
        }
    }
}

/// The game used to be called Become a God. Its save folder moves to the
/// new name the first time; if it can't be moved, the old one keeps being used.
fn moved_saves(old: &std::path::Path, new: PathBuf) -> PathBuf {
    if new.exists() || !old.exists() {
        return new;
    }
    if std::fs::rename(old, &new).is_ok() {
        return new;
    }
    old.to_path_buf()
}

pub struct Store {
    /// Where profile and troops are saved: the player's folder, or its
    /// test-save folder while test mode is on.
    pub dir: PathBuf,
    /// The player's own folder. Settings and the sign-in live here.
    pub home: PathBuf,
}

/// Gold shown in test mode; it is topped up every frame so it never runs out.
pub const TEST_GOLD: u32 = 999_999;

impl Store {
    pub fn open() -> Store {
        let dir = std::env::var_os("BAG_SAVE_DIR").map(PathBuf::from).unwrap_or_else(|| {
            let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
            moved_saves(&data.join("become-a-god"), data.join("legion-of-minds"))
        });
        let _ = std::fs::create_dir_all(dir.join("troops"));
        Store { home: dir.clone(), dir }
    }

    pub fn is_test(&self) -> bool {
        self.dir != self.home
    }

    /// The test save, a separate copy so cheats never touch the real one.
    /// The first time, it starts as a copy of the real profile and troops.
    pub fn test_store(&self) -> Store {
        let dir = self.home.join("test-save");
        if !dir.join("profile.json").exists() {
            let _ = std::fs::create_dir_all(dir.join("troops"));
            if let Ok(rd) = std::fs::read_dir(self.home.join("troops")) {
                for e in rd.flatten() {
                    let _ = std::fs::copy(e.path(), dir.join("troops").join(e.file_name()));
                }
            }
            let mut p = self.load_profile();
            p.test = true;
            Store { dir: dir.clone(), home: self.home.clone() }.save_profile(&p);
        }
        let _ = std::fs::create_dir_all(dir.join("troops"));
        Store { dir, home: self.home.clone() }
    }

    /// The player's real save.
    pub fn real_store(&self) -> Store {
        Store { dir: self.home.clone(), home: self.home.clone() }
    }

    pub fn load_profile(&self) -> Profile {
        let mut p: Profile = std::fs::read_to_string(self.dir.join("profile.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        p.best_stars.resize(godcore::scenario::BASES.len(), 0);
        p.waves_held.resize(godcore::scenario::WAVES.len(), false);
        p
    }

    pub fn save_profile(&self, p: &Profile) {
        let _ = std::fs::write(
            self.dir.join("profile.json"),
            serde_json::to_string_pretty(p).unwrap(),
        );
    }

    pub fn load_troops(&self) -> Vec<Troop> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.dir.join("troops")) {
            for e in rd.flatten() {
                if let Ok(s) = std::fs::read_to_string(e.path()) {
                    if let Ok(t) = Troop::load_json(&s) {
                        v.push(t);
                    }
                }
            }
        }
        v.sort_by_key(|t| t.id);
        v
    }

    fn troop_path(&self, id: u64) -> PathBuf {
        self.dir.join("troops").join(format!("{id}.troop.json"))
    }

    pub fn save_troop(&self, t: &Troop) {
        let _ = std::fs::write(self.troop_path(t.id), t.save_json());
    }

    pub fn delete_troop(&self, id: u64) {
        let _ = std::fs::remove_file(self.troop_path(id));
    }

    /// Copy a troop's file to the exports folder, ready to share.
    pub fn export(&self, t: &Troop) -> PathBuf {
        let d = self.dir.join("exports");
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(format!("{}-{}.troop.json", t.name.to_lowercase(), t.id));
        let _ = std::fs::write(&p, t.save_json());
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_save_is_a_copy_that_leaves_the_real_one_alone() {
        let home = std::env::temp_dir().join(format!("lom-test-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("troops")).unwrap();
        let real = Store { dir: home.clone(), home: home.clone() };
        let mut p = Profile::default();
        p.gold = 123;
        real.save_profile(&p);
        std::fs::write(home.join("troops/7.troop.json"), "{}").unwrap();

        let test = real.test_store();
        assert!(test.is_test() && !real.is_test());
        let mut tp = test.load_profile();
        assert!(tp.test);
        assert_eq!(tp.gold, 123);
        assert!(test.dir.join("troops/7.troop.json").exists());
        tp.gold = TEST_GOLD;
        test.save_profile(&tp);
        assert_eq!(real.load_profile().gold, 123, "real save untouched");
        assert_eq!(test.real_store().load_profile().gold, 123);
        assert_eq!(real.test_store().load_profile().gold, TEST_GOLD, "test save kept between switches");
        let _ = std::fs::remove_dir_all(&home);
    }
}
