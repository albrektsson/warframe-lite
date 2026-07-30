//! Small disk-backed caches under `~/.cache/warframe-lite/`.
//!
//! Two shapes:
//! * [`save_blob`] / [`load_blob`] — a single timestamped value (the item
//!   catalogue).
//! * [`KeyedCache`] — a persisted `key → timestamped value` map (per-item
//!   prices), designed to serve **stale data instantly** when the network is
//!   slow, which is what the few-second relic-selection window needs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `~/.cache/warframe-lite/`, created if missing.
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "warframe-lite")
        .context("could not determine a cache directory")?;
    let dir = dirs.cache_dir().to_path_buf();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// A value tagged with when it was fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stamped<V> {
    pub value: V,
    pub fetched_at: u64,
}

impl<V> Stamped<V> {
    /// How long ago this value was fetched.
    pub fn age(&self) -> Duration {
        Duration::from_secs(now_unix().saturating_sub(self.fetched_at))
    }
}

/// Serializable wrapper borrowing its value, used only for writing.
#[derive(Serialize)]
struct StampedRef<'a, V> {
    value: &'a V,
    fetched_at: u64,
}

/// Persist a single value as `<cache_dir>/<name>` with the current timestamp.
pub fn save_blob<V: Serialize>(name: &str, value: &V) -> Result<()> {
    let path = cache_dir()?.join(name);
    let wrapped = StampedRef {
        value,
        fetched_at: now_unix(),
    };
    let bytes = serde_json::to_vec(&wrapped).context("serializing cache blob")?;
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Load a single timestamped value from `<cache_dir>/<name>`, if present/valid.
pub fn load_blob<V: DeserializeOwned>(name: &str) -> Option<Stamped<V>> {
    let path = cache_dir().ok()?.join(name);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<Stamped<V>>(&bytes).ok()
}

/// A persisted `key → timestamped value` map.
pub struct KeyedCache<V> {
    path: PathBuf,
    map: Mutex<HashMap<String, Stamped<V>>>,
}

impl<V: Serialize + DeserializeOwned + Clone> KeyedCache<V> {
    /// Load a keyed cache from `<cache_dir>/<file_name>` (empty if absent).
    pub fn load(file_name: &str) -> Self {
        let path = cache_dir()
            .map(|d| d.join(file_name))
            .unwrap_or_else(|_| PathBuf::from(file_name));
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, Stamped<V>>>(&b).ok())
            .unwrap_or_default();
        tracing::debug!("loaded {} cached entries from {}", map.len(), path.display());
        Self {
            path,
            map: Mutex::new(map),
        }
    }

    /// Get a stamped value by key (cloned).
    pub fn get(&self, key: &str) -> Option<Stamped<V>> {
        self.map.lock().unwrap().get(key).cloned()
    }

    /// Insert/replace a value with the current timestamp (in memory only —
    /// call [`save`](Self::save) to persist).
    pub fn put(&self, key: &str, value: V) {
        self.map.lock().unwrap().insert(
            key.to_string(),
            Stamped {
                value,
                fetched_at: now_unix(),
            },
        );
    }

    /// Persist the whole map to disk (best-effort; logs on failure).
    pub fn save(&self) {
        if let Err(e) = self.try_save() {
            tracing::warn!("failed to persist cache {}: {e:#}", self.path.display());
        }
    }

    fn try_save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let map = self.map.lock().unwrap();
        let bytes = serde_json::to_vec(&*map).context("serializing keyed cache")?;
        std::fs::write(&self.path, bytes).with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_cache_roundtrips_via_disk() {
        let name = format!("wf-cache-test-{}.json", std::process::id());
        let c = KeyedCache::<u32>::load(&name);
        c.put("a", 10);
        c.put("b", 20);
        c.save();

        let c2 = KeyedCache::<u32>::load(&name);
        assert_eq!(c2.get("a").map(|s| s.value), Some(10));
        assert_eq!(c2.get("b").map(|s| s.value), Some(20));
        assert!(c2.get("missing").is_none());

        // cleanup
        if let Ok(dir) = cache_dir() {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    }

    #[test]
    fn age_is_small_for_fresh_entry() {
        let s = Stamped {
            value: 1,
            fetched_at: now_unix(),
        };
        assert!(s.age() < Duration::from_secs(2));
    }
}
