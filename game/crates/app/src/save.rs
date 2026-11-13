//! Save data: the player's profile and one file per troop.

use godcore::troop::Troop;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// Something the player should hear about their save (a failed write, a
/// file restored from its backup). Shown as a toast by the main loop.
static NOTICE: Mutex<Option<String>> = Mutex::new(None);

fn notice(msg: String) {
    eprintln!("{msg}");
    if let Ok(mut n) = NOTICE.lock() {
        *n = Some(msg);
    }
}

/// The latest save problem, once.
pub fn take_notice() -> Option<String> {
    NOTICE.lock().ok().and_then(|mut n| n.take())
}

fn backup_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

/// Write `data` so that `path` always holds either the old or the new
/// contents, never half of each, even if the game or the PC stops midway.
/// The previous version is kept as `<path>.bak`.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    if path.exists() {
        // Keep the last good copy. A failure here only loses the backup.
        let _ = std::fs::rename(path, backup_of(path));
    }
    std::fs::rename(&tmp, path)
}

/// `write_atomic`, telling the player if it fails.
pub fn save_file(path: &Path, data: &[u8]) {
    if let Err(e) = write_atomic(path, data) {
        notice(format!("Couldn't save {}: {e}", path.display()));
    }
}

/// Read and parse `path`, or its backup if the file is missing or damaged.
/// The second value says whether the backup was used.
pub fn load_file<T>(path: &Path, parse: impl Fn(&str) -> Option<T>) -> Option<(T, bool)> {
    let read = |p: &Path| std::fs::read_to_string(p).ok().and_then(|s| parse(&s));
    if let Some(v) = read(path) {
        return Some((v, false));
    }
    read(&backup_of(path)).map(|v| (v, true))
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
        let path = self.dir.join("profile.json");
        let mut p = match load_file(&path, |s| serde_json::from_str::<Profile>(s).ok()) {
            Some((p, false)) => p,
            Some((p, true)) => {
                notice("Your profile was damaged, so the last good copy was loaded.".into());
                p
            }
            None => {
                if path.exists() {
                    // Keep the damaged file out of the way of the next save,
                    // so it can still be looked at.
                    let _ = std::fs::rename(&path, self.dir.join("profile.json.damaged"));
                    notice("Your profile couldn't be read and has been reset. The damaged file is kept as profile.json.damaged.".into());
                }
                Profile::default()
            }
        };
        p.best_stars.resize(godcore::scenario::BASES.len(), 0);
        p.waves_held.resize(godcore::scenario::WAVES.len(), false);
        p
    }

    pub fn save_profile(&self, p: &Profile) {
        save_file(
            &self.dir.join("profile.json"),
            serde_json::to_string_pretty(p).unwrap().as_bytes(),
        );
    }

    pub fn load_troops(&self) -> Vec<Troop> {
        let dir = self.dir.join("troops");
        // Troop ids that have a file or a backup.
        let mut ids: Vec<u64> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name
                    .strip_suffix(".troop.json")
                    .or_else(|| name.strip_suffix(".troop.json.bak"))
                    .and_then(|n| n.parse().ok());
                if let Some(id) = id {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        ids.sort_unstable();
        let mut v = Vec::new();
        for id in ids {
            match load_file(&self.troop_path(id), |s| Troop::load_json(s).ok()) {
                Some((t, used_backup)) => {
                    if used_backup {
                        notice(format!("{}'s save was damaged, so the last good copy was loaded.", t.name.trim()));
                    }
                    v.push(t);
                }
                None => notice(format!("Troop file {id}.troop.json couldn't be read and was skipped.")),
            }
        }
        v
    }

    /// A troop id no troop file uses yet, at least `next`.
    pub fn free_troop_id(&self, next: u64) -> u64 {
        let mut id = next.max(1);
        while self.troop_path(id).exists() || backup_of(&self.troop_path(id)).exists() {
            id += 1;
        }
        id
    }

    fn troop_path(&self, id: u64) -> PathBuf {
        self.dir.join("troops").join(format!("{id}.troop.json"))
    }

    pub fn save_troop(&self, t: &Troop) {
        save_file(&self.troop_path(t.id), t.save_json().as_bytes());
    }

    pub fn delete_troop(&self, id: u64) {
        let p = self.troop_path(id);
        let _ = std::fs::remove_file(backup_of(&p));
        let _ = std::fs::remove_file(p);
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

    fn scratch(name: &str) -> Store {
        let home = std::env::temp_dir().join(format!("lom-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("troops")).unwrap();
        Store { dir: home.clone(), home }
    }

    #[test]
    fn a_damaged_profile_comes_back_from_its_backup() {
        let s = scratch("profile-backup");
        let mut p = Profile::default();
        p.gold = 111;
        s.save_profile(&p);
        p.gold = 222;
        s.save_profile(&p);
        // The PC died halfway through the next save.
        std::fs::write(s.dir.join("profile.json"), "{\"gold\": 3").unwrap();
        assert_eq!(s.load_profile().gold, 111);
        assert!(take_notice().is_some());
        let _ = std::fs::remove_dir_all(&s.home);
    }

    #[test]
    fn an_unreadable_profile_is_kept_and_troops_are_not_overwritten() {
        let s = scratch("profile-lost");
        let t = Troop::new(1, "Old".into(), godcore::classes::BaseClass::Fighter.spec(), 5);
        s.save_troop(&t);
        std::fs::write(s.dir.join("profile.json"), "garbage").unwrap();
        let p = s.load_profile();
        assert_eq!(p.next_id, 1);
        assert!(s.dir.join("profile.json.damaged").exists());
        // A new recruit must not take troop 1's file.
        assert_eq!(s.free_troop_id(p.next_id), 2);
        let _ = std::fs::remove_dir_all(&s.home);
    }

    #[test]
    fn troops_load_once_and_fall_back_to_their_backup() {
        let s = scratch("troop-backup");
        let mut t = Troop::new(4, "Ann".into(), godcore::classes::BaseClass::Fighter.spec(), 5);
        s.save_troop(&t);
        t.steps = 10;
        s.save_troop(&t);
        assert_eq!(s.load_troops().len(), 1, "the backup isn't a second troop");
        std::fs::write(s.dir.join("troops/4.troop.json"), "{").unwrap();
        let v = s.load_troops();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].steps, 0, "last good copy");
        s.delete_troop(4);
        assert!(s.load_troops().is_empty());
        let _ = std::fs::remove_dir_all(&s.home);
    }
}
