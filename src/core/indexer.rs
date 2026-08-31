use std::fs;
use std::path::PathBuf;

use super::model::AppEntry;

/// Linux writes its index to disk so the launcher starts under ~2ms.
/// Windows does not cache (it enumerates Start Menu on demand).
#[cfg(target_os = "linux")]
pub const CACHE_NAME: &str = "stools-apps-v4.bin";

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

/// Load the cached application list, if present and readable.
#[cfg(target_os = "linux")]
pub fn load_cache() -> Option<Vec<AppEntry>> {
    let path = cache_path();
    let bytes = fs::read(path).ok()?;
    bincode::decode_from_slice(&bytes, bincode::config::standard())
        .ok()
        .map(|(entries, _)| entries)
}

/// Write the application list to the cache file.
#[cfg(target_os = "linux")]
pub fn save_cache(entries: &[AppEntry]) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(bytes) = bincode::encode_to_vec(entries, bincode::config::standard()) {
        // Ignore errors: caching is best-effort.
        let _ = fs::write(path, bytes);
    }
}

