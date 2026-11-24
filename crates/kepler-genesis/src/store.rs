//! On-disk designs under `<KEPLER_HOME>/genesis/<id>/` (never inside the user's repository):
//! `spec.json`, `preview-N.html` and, when a browser capture succeeded, `preview-N.png`.
//! Retention is bounded by count and by total bytes; ids are validated so a caller-supplied id
//! can never leave the store directory.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_DESIGNS: usize = 20;
pub const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;
pub const RECORD_SCHEMA: &str = "kepler.genesis.design/1";

#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub created_secs: u64,
    pub brief: String,
    pub candidates: Vec<String>,
    pub model: String,
    pub previews: u32,
    pub pngs: u32,
}

pub struct Store {
    root: PathBuf,
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn valid_id(id: &str) -> bool {
    id.len() >= 2
        && id.len() <= 24
        && id.starts_with('g')
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || c == 'g' || c == '-')
}

impl Store {
    /// `home` is the Kepler data directory; designs live in its `genesis` subdirectory.
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join("genesis"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir_of(&self, id: &str) -> Result<PathBuf, StoreError> {
        if !valid_id(id) {
            return Err(StoreError("invalid design id".into()));
        }
        Ok(self.root.join(id))
    }

    /// Create a fresh design directory and return its id.
    pub fn alloc(&self) -> Result<(String, PathBuf), StoreError> {
        std::fs::create_dir_all(&self.root)?;
        let base = format!("g{:013}", now_millis());
        for n in 0..50 {
            let id = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let dir = self.root.join(&id);
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok((id, dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(StoreError("cannot allocate a design id".into()))
    }

    pub fn write_record(&self, dir: &Path, record: &Value) -> Result<(), StoreError> {
        let text = serde_json::to_string_pretty(record).map_err(|e| StoreError(e.to_string()))?;
        std::fs::write(dir.join("spec.json"), text)?;
        Ok(())
    }

    fn ids(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| valid_id(n))
            .collect();
        ids.sort();
        ids
    }

    pub fn read(&self, id: &str) -> Result<Value, StoreError> {
        let dir = self.dir_of(id)?;
        let text = std::fs::read_to_string(dir.join("spec.json"))
            .map_err(|_| StoreError(format!("no saved design `{id}`")))?;
        serde_json::from_str(&text).map_err(|_| StoreError(format!("design `{id}` is corrupt")))
    }

    fn summarize(&self, id: &str) -> Option<Summary> {
        let v = self.read(id).ok()?;
        let dir = self.root.join(id);
        let count = |ext: &str| {
            std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_name().to_string_lossy().ends_with(ext))
                        .count() as u32
                })
                .unwrap_or(0)
        };
        Some(Summary {
            id: id.to_string(),
            created_secs: v.get("created_secs").and_then(Value::as_u64).unwrap_or(0),
            brief: v
                .get("brief")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(120)
                .collect(),
            candidates: v
                .get("candidates")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            model: v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            previews: count(".html"),
            pngs: count(".png"),
        })
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Summary> {
        let mut v: Vec<Summary> = self
            .ids()
            .iter()
            .filter_map(|i| self.summarize(i))
            .collect();
        v.reverse();
        v
    }

    /// A saved design whose `cache_key` matches (same redacted brief, project summary, model, count).
    pub fn find_by_key(&self, key: &str) -> Option<String> {
        self.ids().into_iter().rev().find(|id| {
            self.read(id)
                .ok()
                .and_then(|v| v.get("cache_key").and_then(Value::as_str).map(|k| k == key))
                == Some(true)
        })
    }

    pub fn purge_one(&self, id: &str) -> Result<(), StoreError> {
        let dir = self.dir_of(id)?;
        if !dir.is_dir() {
            return Err(StoreError(format!("no saved design `{id}`")));
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// Remove every saved design; returns how many.
    pub fn purge_all(&self) -> usize {
        let ids = self.ids();
        let n = ids.len();
        for id in ids {
            let _ = std::fs::remove_dir_all(self.root.join(id));
        }
        n
    }

    fn dir_bytes(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.metadata().ok())
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Delete the oldest designs until at most `keep` remain and the store fits `max_bytes`.
    /// `protect` (the design just written) is never removed. Returns how many were removed.
    pub fn prune(&self, keep: usize, max_bytes: u64, protect: &str) -> usize {
        let mut ids = self.ids();
        let mut removed = 0;
        let total = |ids: &[String]| {
            ids.iter()
                .map(|i| Self::dir_bytes(&self.root.join(i)))
                .sum::<u64>()
        };
        while ids.len() > 1 && (ids.len() > keep || total(&ids) > max_bytes) {
            let Some(pos) = ids.iter().position(|i| i != protect) else {
                break;
            };
            let id = ids.remove(pos);
            if std::fs::remove_dir_all(self.root.join(&id)).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

pub fn record(id: &str, extra: Value) -> Value {
    let mut v = json!({"schema": RECORD_SCHEMA, "id": id, "created_secs": now_millis() / 1000});
    if let (Some(m), Value::Object(e)) = (v.as_object_mut(), extra) {
        m.extend(e);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::tests::temp_dir;

    fn put(store: &Store, brief: &str, key: &str, bytes: usize) -> String {
        let (id, dir) = store.alloc().unwrap();
        store
            .write_record(&dir, &record(&id, json!({"brief": brief, "cache_key": key, "model": "m", "candidates": [{"name": "A"}]})))
            .unwrap();
        std::fs::write(dir.join("preview-1.html"), vec![b'x'; bytes]).unwrap();
        id
    }

    #[test]
    fn save_list_show_purge_roundtrip() {
        let home = temp_dir("store");
        let s = Store::new(&home);
        assert!(s.list().is_empty());
        let a = put(&s, "first", "k1", 10);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let b = put(&s, "second", "k2", 10);
        let l = s.list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].id, b, "newest first");
        assert_eq!(l[0].candidates, vec!["A".to_string()]);
        assert_eq!(l[0].previews, 1);
        assert_eq!(s.read(&a).unwrap()["brief"], "first");
        assert_eq!(s.find_by_key("k1").as_deref(), Some(a.as_str()));
        assert!(s.find_by_key("nope").is_none());
        s.purge_one(&a).unwrap();
        assert!(s.read(&a).is_err());
        assert_eq!(s.purge_all(), 1);
        assert!(s.list().is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn ids_cannot_escape_the_store() {
        let home = temp_dir("store-esc");
        let s = Store::new(&home);
        for bad in [
            "..",
            "../x",
            "g1/../..",
            "g1\\x",
            "",
            "x",
            "g\0",
            "g12345678901234567890123456",
        ] {
            assert!(s.dir_of(bad).is_err(), "{bad:?}");
            assert!(s.read(bad).is_err());
            assert!(s.purge_one(bad).is_err());
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn retention_by_count_and_bytes_keeps_the_newest() {
        let home = temp_dir("store-ret");
        let s = Store::new(&home);
        let mut last = String::new();
        for i in 0..5 {
            last = put(&s, &format!("d{i}"), &format!("k{i}"), 100);
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        assert_eq!(s.prune(3, u64::MAX, &last), 2);
        assert_eq!(s.list().len(), 3);
        assert_eq!(s.list()[0].id, last);
        // by size: everything but the protected newest goes
        assert!(s.prune(10, 10, &last) >= 2);
        assert_eq!(s.list().len(), 1);
        assert_eq!(s.list()[0].id, last);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn corrupt_designs_are_skipped_in_listings() {
        let home = temp_dir("store-bad");
        let s = Store::new(&home);
        let (id, dir) = s.alloc().unwrap();
        std::fs::write(dir.join("spec.json"), "{broken").unwrap();
        assert!(s.list().is_empty());
        assert!(s.read(&id).is_err());
        let _ = std::fs::remove_dir_all(home);
    }
}
