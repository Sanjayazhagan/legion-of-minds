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
        }
    }
}

pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn open() -> Store {
        let dir = std::env::var_os("BAG_SAVE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("become-a-god")
            });
        let _ = std::fs::create_dir_all(dir.join("troops"));
        Store { dir }
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
