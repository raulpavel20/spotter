//! Viewed marks (PLAN §6.3): content-addressed, stored in
//! `<git-common-dir>/spotter/viewed.json`, never committed.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub const PRUNE_AFTER_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Debug, Default, Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    viewed: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
enum Op {
    Set(String, i64),
    Unset(String),
}

#[derive(Debug, Default)]
pub struct Marks {
    path: Option<PathBuf>,
    viewed: HashMap<String, i64>,
    /// Changes not yet written, replayed onto the file's current content.
    pending: Vec<Op>,
    mtime: Option<SystemTime>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn read(path: &Path) -> HashMap<String, i64> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<OnDisk>(&b).ok())
        .map(|d| d.viewed)
        .unwrap_or_default()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Marks {
    /// Marks backed by a file. A missing or corrupt file means no marks.
    pub fn load(path: PathBuf) -> Marks {
        Marks {
            viewed: read(&path),
            mtime: mtime(&path),
            path: Some(path),
            pending: Vec::new(),
        }
    }

    /// Marks that are never written (tests).
    pub fn in_memory() -> Marks {
        Marks::default()
    }

    pub fn is_viewed(&self, key: &str) -> bool {
        self.viewed.contains_key(key)
    }

    pub fn set(&mut self, key: String, viewed: bool, now: i64) {
        if viewed {
            self.viewed.insert(key.clone(), now);
            self.pending.push(Op::Set(key, now));
        } else if self.viewed.remove(&key).is_some() {
            self.pending.push(Op::Unset(key));
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.pending.is_empty()
    }

    fn replay(&self, mut base: HashMap<String, i64>) -> HashMap<String, i64> {
        for op in &self.pending {
            match op {
                Op::Set(k, t) => {
                    base.insert(k.clone(), *t);
                }
                Op::Unset(k) => {
                    base.remove(k);
                }
            }
        }
        base
    }

    /// Read-merge-write, atomically, pruning old entries.
    pub fn save(&mut self, now: i64) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            self.pending.clear();
            return Ok(());
        };
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut merged = self.replay(read(&path));
        merged.retain(|_, t| *t >= now - PRUNE_AFTER_SECS);
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("viewed.json.{}.tmp", std::process::id()));
        {
            let mut f = fs::File::create(&tmp)?;
            let data = OnDisk {
                version: 1,
                viewed: merged.clone(),
            };
            serde_json::to_writer(&mut f, &data)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        self.viewed = merged;
        self.pending.clear();
        self.mtime = mtime(&path);
        Ok(())
    }

    /// Picks up marks written by another instance. Returns true if the
    /// set changed.
    pub fn reload_if_changed(&mut self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let m = mtime(path);
        if m == self.mtime {
            return false;
        }
        self.mtime = m;
        let merged = self.replay(read(path));
        let changed = merged != self.viewed;
        self.viewed = merged;
        changed
    }

    pub fn len(&self) -> usize {
        self.viewed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.viewed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_instances_do_not_clobber_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotter").join("viewed.json");
        let mut a = Marks::load(path.clone());
        let mut b = Marks::load(path.clone());
        a.set("a".into(), true, 100);
        b.set("b".into(), true, 100);
        a.save(100).unwrap();
        b.save(100).unwrap();
        let c = Marks::load(path.clone());
        assert!(c.is_viewed("a") && c.is_viewed("b"));
        // a picks up b's mark.
        assert!(a.reload_if_changed() || a.is_viewed("b"));
        assert!(a.is_viewed("b"));
        // Unsetting survives a merge with an older file.
        a.set("b".into(), false, 101);
        a.save(101).unwrap();
        assert!(!Marks::load(path).is_viewed("b"));
    }

    #[test]
    fn prunes_old_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("viewed.json");
        let mut m = Marks::load(path.clone());
        m.set("old".into(), true, 0);
        m.set("new".into(), true, PRUNE_AFTER_SECS + 10);
        m.save(PRUNE_AFTER_SECS + 10).unwrap();
        let m = Marks::load(path.clone());
        assert!(!m.is_viewed("old"));
        assert!(m.is_viewed("new"));
        let raw: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
    }

    #[test]
    fn corrupt_file_is_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("viewed.json");
        fs::write(&path, "{not json").unwrap();
        let mut m = Marks::load(path.clone());
        assert!(m.is_empty());
        m.set("k".into(), true, 5);
        m.save(5).unwrap();
        assert!(Marks::load(path).is_viewed("k"));
    }
}
