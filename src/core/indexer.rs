#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use super::model::AppEntry;

/// Linux writes its index to disk so the launcher starts under ~2ms.
/// Windows does not cache (it enumerates Start Menu on demand).
#[cfg(target_os = "linux")]
pub const CACHE_NAME: &str = "stools-apps-v5.bin";

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
#[cfg(target_os = "linux")]
pub fn dirs_fingerprint(dir_sets: &[&[PathBuf]]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for set in dir_sets {
        set.len().hash(&mut hasher);
        for dir in *set {
            dir.hash(&mut hasher);
        }
    }
    hasher.finish()
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
