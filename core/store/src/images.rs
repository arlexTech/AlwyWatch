// SPDX-License-Identifier: AGPL-3.0-or-later
//! Size-capped on-disk image cache. Files are written atomically (temp file + rename) so a
//! crash never leaves a half image that a UI would try to decode.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Result, Store, image_file};

pub struct ImageCache {
    store: Arc<Store>,
    dir: PathBuf,
    cap_bytes: u64,
    total: AtomicU64,
}

impl ImageCache {
    pub fn new(store: Arc<Store>, dir: &Path, cap_bytes: u64) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let total = store.image_total()?;
        Ok(Self {
            store,
            dir: dir.to_owned(),
            cap_bytes,
            total: AtomicU64::new(total),
        })
    }

    /// Path of a cached image, or `None`. Refreshes its LRU position.
    pub fn get(&self, key: &str) -> Result<Option<PathBuf>> {
        let path = image_file(&self.dir, key);
        if !self.store.image_touch(key)? {
            return Ok(None);
        }
        if path.exists() {
            Ok(Some(path))
        } else {
            // Index says yes, disk says no (user cleared app cache): forget it.
            self.store.image_remove(key)?;
            Ok(None)
        }
    }

    /// Stores bytes and returns the file path, evicting old entries above the cap.
    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = image_file(&self.dir, key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.store.image_insert(key, bytes.len() as u64)?;
        let total =
            self.total.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64;
        if total > self.cap_bytes {
            // Evict down to 90% of the cap so we do not evict on every insert.
            let target = self.cap_bytes / 10 * 9;
            for (victim, size) in self.store.image_evict(total - target)? {
                if victim != key {
                    let _ = std::fs::remove_file(image_file(&self.dir, &victim));
                    self.total.fetch_sub(size, Ordering::Relaxed);
                } else {
                    // Never evict what we just wrote; re-register it.
                    self.store.image_insert(key, size)?;
                }
            }
        }
        Ok(path)
    }

    pub fn total_bytes(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn clear(&self) -> Result<()> {
        let all = self.store.image_evict(u64::MAX)?;
        for (k, _) in all {
            let _ = std::fs::remove_file(image_file(&self.dir, &k));
        }
        self.total.store(0, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_above_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let cache = ImageCache::new(store.clone(), dir.path(), 1000).unwrap();
        let a = cache.put("a", &[0u8; 400]).unwrap();
        cache.put("b", &[0u8; 400]).unwrap();
        // Make "a" older than "b" regardless of clock resolution.
        store
            .conn()
            .execute("UPDATE images SET last_access = 1 WHERE key = 'a'", [])
            .unwrap();
        cache.put("c", &[0u8; 400]).unwrap(); // 1200 > 1000 -> evict down to 900
        assert!(!a.exists(), "oldest entry evicted from disk");
        assert!(cache.get("a").unwrap().is_none());
        assert!(cache.get("b").unwrap().is_some());
        assert!(cache.get("c").unwrap().is_some());
        assert_eq!(cache.total_bytes(), 800);
    }

    #[test]
    fn missing_file_is_a_miss_and_index_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let cache = ImageCache::new(store.clone(), dir.path(), 10_000).unwrap();
        let p = cache.put("x", b"img").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"img");
        let again = ImageCache::new(store.clone(), dir.path(), 10_000).unwrap();
        assert_eq!(again.total_bytes(), 3);
        std::fs::remove_file(&p).unwrap();
        assert!(again.get("x").unwrap().is_none());
        cache.clear().unwrap();
        assert_eq!(cache.total_bytes(), 0);
    }
}
