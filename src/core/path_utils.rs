use std::path::{Path, PathBuf};

/// Prettify an absolute path for display:
///   /home/user/.cargo/bin/rg → ~/.cargo/bin/rg
///   /usr/bin/ls              → /usr/bin/ls (no transformation)
pub fn prettify_path(path: &Path) -> String {
    let s = path.to_string_lossy();

    // Try the home directory → ~ ($HOME on Linux, %USERPROFILE% on Windows)
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var) {
            if !home.is_empty() && s.starts_with(&home) {
                return format!("~{}", &s[home.len()..]);
            }
        }
    }

    s.into_owned()
}

/// Default directories to scan for executables when no CLI args are given.
/// These cover the most common locations across Arch / Fedora / Ubuntu / NixOS
/// user setups. The user can always override or extend via the config file or
/// CLI arguments.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn default_binary_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();

    if !home.is_empty() {
        let home = PathBuf::from(&home);
        dirs.push(home.join(".local/bin"));
    }

    for system in ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        dirs.push(PathBuf::from(system));
    }

    // Linuxbrew
    dirs.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));

    dirs
}

/// Clean up a directory path so equal directories compare equal: trailing
/// separators are dropped and `.` / (lexical) `..` components are resolved.
///
/// Without this, `~/.cargo/bin`, `~/.cargo/bin/` and `~/.cargo/./bin` are three
/// different `PathBuf`s, so an entry repeated in the config file would be
/// scanned twice — and on Linux every `.desktop` file inside it would be listed
/// twice as well.
pub(crate) fn normalize_dir(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            // No ParentDir handling: `..` cannot be cancelled without knowing
            // where the path starts, and a symlink in between would change the
            // meaning. Binaries are still de-duplicated by real path later on.
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        path
    } else {
        out
    }
}

/// Expand the given directory strings (`~`, `$VAR`, `${VAR}`, `%VAR%`), keeping
/// only the ones that exist and dropping duplicates. Used for both the config
/// file's `path` list and the command line arguments.
pub fn resolve_dirs(raw: &[String]) -> Vec<PathBuf> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    raw.iter()
        .filter_map(|s| crate::core::config::expand_path(s))
        .map(normalize_dir)
        .filter(|p| p.is_dir())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Merge extra directories (config file `path` + CLI arguments) with the default
/// set, deduplicating.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn merge_binary_dirs(extra: &[String]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = resolve_dirs(extra);

    let mut seen: std::collections::HashSet<PathBuf> = dirs.iter().cloned().collect();
    for d in default_binary_dirs() {
        let d = normalize_dir(d);
        if seen.insert(d.clone()) {
            dirs.push(d);
        }
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettifies_home_to_tilde() {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(&home).join(".cargo/bin/rg");
            assert_eq!(prettify_path(&p), "~/.cargo/bin/rg");
        }
    }

    #[test]
    fn leaves_system_path_alone() {
        assert_eq!(prettify_path(Path::new("/usr/bin/ls")), "/usr/bin/ls");
    }

    #[test]
    fn config_paths_dont_duplicate_builtin_dirs() {
        // ~/.local/bin is a built-in binary directory.
        let raw = ["$HOME/.local/bin".into(), "~/.local/bin".into()];
        let merged = merge_binary_dirs(&raw);
        let home = dirs::home_dir().expect("home dir");
        let dir = home.join(".local/bin");
        assert!(
            merged.iter().filter(|d| **d == dir).count() <= 1,
            ".local/bin appears {} times",
            merged.iter().filter(|d| **d == dir).count()
        );
    }

    #[test]
    fn equivalent_spellings_of_one_dir_collapse_to_one_entry() {
        let dirs = resolve_dirs(&[
            "/usr/bin".into(),
            "/usr/bin/".into(),
            "/usr/./bin".into(),
            "/usr/bin//".into(),
        ]);
        assert_eq!(dirs, vec![PathBuf::from("/usr/bin")]);
    }
}
