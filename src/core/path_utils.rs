use std::path::{Path, PathBuf};

/// Prettify an absolute path for display:
///   /home/user/.cargo/bin/rg → ~/.cargo/bin/rg
///   C:\Users\me\AppData\Roaming\Microsoft\... → %APPDATA%\Microsoft\...
///   /usr/bin/ls              → /usr/bin/ls (no transformation)
pub fn prettify_path(path: &Path) -> String {
    let s = path.to_string_lossy();

    // 1. Windows shell folders first: %APPDATA% / %LOCALAPPDATA% / %PROGRAMDATA%.
    //    Mapping these before the home directory means a Start-Menu path shows as
    //    `%APPDATA%\Microsoft\Windows\Start Menu` instead of `~\AppData\Roaming\...`,
    //    which Windows users read more naturally.
    #[cfg(windows)]
    {
        for (var, prefix) in [
            ("APPDATA", "%APPDATA%"),
            ("LOCALAPPDATA", "%LOCALAPPDATA%"),
            ("PROGRAMDATA", "%PROGRAMDATA%"),
        ] {
            if let Ok(dir) = std::env::var(var) {
                if !dir.is_empty() && s.starts_with(&dir) {
                    return format!("{}{}", prefix, &s[dir.len()..]);
                }
            }
        }
    }

    // 2. Home directory → ~ ($HOME on Linux, %USERPROFILE% on Windows).
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var) {
            if !home.is_empty() && s.starts_with(&home) {
                return format!("~{}", &s[home.len()..]);
            }
        }
    }

    s.into_owned()
}

/// Shrink a long path to a `head...tail` form so it reads well while idle
/// (e.g. `%APPDATA%\...\Programs`, `C:\...\Start Menu\Programs`, `~/.../bin`).
///
/// The head keeps only the most meaningful prefix (an env var, `~`, the drive
/// letter, or the first Linux directory) and the tail keeps the last component or
/// two, joined by an ellipsis. Paths already shorter than `max_len` are unchanged.
pub fn abbreviate_path(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }

    let is_backslash = s.contains('\\');
    let sep = if is_backslash { '\\' } else { '/' };
    let ell = if is_backslash { r"\...\" } else { "/.../" };

    // 1. Extract the head.
    let head = if s.starts_with('%') {
        if let Some(end_idx) = s[1..].find('%') {
            &s[..=end_idx + 1] // "%APPDATA%"
        } else {
            &s[..s.find(sep).unwrap_or(s.len())]
        }
    } else if s.starts_with('~') {
        "~"
    } else if s.len() >= 3
        && s.chars().nth(1) == Some(':')
        && (s.chars().nth(2) == Some('\\') || s.chars().nth(2) == Some('/'))
    {
        &s[..2] // "C:"
    } else if s.starts_with('/') {
        let rest = &s[1..];
        if let Some(next_slash) = rest.find('/') {
            &s[..=next_slash + 1] // "/usr"
        } else {
            s
        }
    } else {
        &s[..s.find(sep).unwrap_or(s.len())]
    };

    let head_clean = head.trim_end_matches(['/', '\\']);
    let head_len = head_clean.chars().count();
    let ell_len = ell.chars().count();
    let tail_budget = max_len.saturating_sub(head_len + ell_len);

    // 2. Pull the most meaningful trailing components from the right.
    let parts: Vec<&str> = s.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let mut tail_parts: Vec<&str> = Vec::new();
    let mut current_tail_len = 0;

    for part in parts.iter().rev() {
        let part_len = part.chars().count() + 1;
        if current_tail_len + part_len <= tail_budget || tail_parts.is_empty() {
            tail_parts.push(part);
            current_tail_len += part_len;
        } else {
            break;
        }
    }
    tail_parts.reverse();
    let tail = tail_parts.join(if is_backslash { "\\" } else { "/" });

    format!("{}{}{}", head_clean, ell, tail)
}

/// Prettified path of the **directory containing** `path`.
///
/// The file name is dropped: the row already shows it as the title, so repeating
/// it in the subtitle wastes the half of the row that is meant to say where the
/// entry came from.
pub fn prettify_dir(path: &Path) -> String {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => prettify_path(parent),
        // A bare file name has no parent to show; keep what we have.
        _ => prettify_path(path),
    }
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
    fn prettify_dir_drops_the_file_name() {
        assert_eq!(prettify_dir(Path::new("/usr/bin/ls")), "/usr/bin");
        assert_eq!(prettify_dir(Path::new("/opt/app/foo.desktop")), "/opt/app");
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(
                prettify_dir(PathBuf::from(&home).join(".cargo/bin/rg").as_path()),
                "~/.cargo/bin"
            );
        }
        // Nothing to strip: keep the path as-is.
        assert_eq!(prettify_dir(Path::new("just-a-name")), "just-a-name");
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

    #[test]
    fn abbreviates_long_paths() {
        assert_eq!(
            abbreviate_path(r"%APPDATA%\Microsoft\Windows\Start Menu\Programs", 26),
            r"%APPDATA%\...\Programs"
        );
        assert_eq!(
            abbreviate_path(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs", 28),
            r"C:\...\Start Menu\Programs"
        );
        assert_eq!(
            abbreviate_path("~/Documents/my_super_long_deep_project_directory/bin", 26),
            "~/.../bin"
        );
        // Short paths are left untouched.
        assert_eq!(
            abbreviate_path("/usr/share/applications", 26),
            "/usr/share/applications"
        );
        // An over-long Linux path collapses onto its first segment.
        assert_eq!(
            abbreviate_path("/usr/share/very_long_deep_dir/leaf", 26),
            "/usr/.../leaf"
        );
    }
}
