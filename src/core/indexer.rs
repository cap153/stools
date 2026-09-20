#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::hash::{Hash, Hasher};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use super::model::AppEntry;

/// Linux writes its index to disk so the launcher starts under ~2ms.
/// Windows does not cache (it enumerates Start Menu on demand).
#[cfg(target_os = "linux")]
pub const CACHE_NAME: &str = "stools-apps-v9.bin";

#[cfg(target_os = "linux")]
fn cache_dir() -> PathBuf {
    if let Ok(cache) = std::env::var("XDG_CACHE_HOME") {
        if !cache.is_empty() {
            return PathBuf::from(cache);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache")
}

#[cfg(target_os = "linux")]
pub fn cache_path() -> PathBuf {
    cache_dir().join(CACHE_NAME)
}

/// Hash of the scanned directory sets. Stored next to the entries so editing the
/// config file's `path` list (or passing different CLI arguments) invalidates the
/// cache instead of serving results from the previous directory set.
///
/// The fingerprint is *directory-level* and deliberately cheap: it never
/// `read_dir`s the directory or stats its individual entries. For each directory
/// it folds in the path, inode, `mtime`/`mtime_nsec` and mode. A newly added (or
/// removed) executable — including a symlink to one — changes the directory's
/// `mtime`, so the fingerprint differs on the very next launch and the cache is
/// treated as stale. This keeps a cold `stools` start free of scanning `/usr/bin`
/// etc., while the background refresh in `load_apps` still keeps `.desktop`/icon
/// inputs current.
#[cfg(target_os = "linux")]
pub fn dirs_fingerprint(dir_sets: &[&[PathBuf]]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    for set in dir_sets {
        set.len().hash(&mut hasher);

        for dir in *set {
            dir.hash(&mut hasher);

            match fs::metadata(dir) {
                Ok(meta) => {
                    meta.ino().hash(&mut hasher);
                    meta.mtime().hash(&mut hasher);
                    meta.mtime_nsec().hash(&mut hasher);
                    meta.mode().hash(&mut hasher);
                }
                Err(_) => {
                    0u8.hash(&mut hasher);
                }
            }
        }
    }

    hasher.finish()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::dirs_fingerprint;

    #[test]
    fn dirs_fingerprint_changes_when_entry_is_added() {
        let dir = std::env::temp_dir().join("stools_test_fingerprint_added");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let before = dirs_fingerprint(&[&[dir.clone()]]);

        std::fs::write(dir.join("new-tool"), b"test").unwrap();

        let after = dirs_fingerprint(&[&[dir.clone()]]);

        assert_ne!(before, after);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Load the cached application list, if present, readable and scanned from the
/// same directories.
#[cfg(target_os = "linux")]
pub fn load_cache(fingerprint: u64) -> Option<Vec<AppEntry>> {
    let bytes = fs::read(cache_path()).ok()?;
    let ((cached_fingerprint, entries), _): ((u64, Vec<AppEntry>), _) =
        bincode::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
    (cached_fingerprint == fingerprint).then_some(entries)
}

/// Write the application list to the cache file.
#[cfg(target_os = "linux")]
pub fn save_cache(entries: &[AppEntry], fingerprint: u64) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(bytes) = bincode::encode_to_vec((fingerprint, entries), bincode::config::standard()) {
        // Ignore errors: caching is best-effort.
        let _ = fs::write(path, bytes);
    }
}
