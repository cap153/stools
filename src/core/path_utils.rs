use std::path::{Path, PathBuf};

/// Map of known prefix → display alias, checked in order.
/// The longest matching prefix wins.
const KNOWN_PREFIXES: &[(&str, &str)] = &[
    ("/home/linuxbrew/.linuxbrew", "$HOMEBREW_PREFIX"),
    ("/opt/rocm", "$ROCM_HOME"),
];

/// Prettify an absolute path for display:
///   /home/user/.cargo/bin/rg → ~/.cargo/bin/rg
///   /opt/rocm/bin/rocm-smi  → $ROCM_HOME/bin/rocm-smi
///   /usr/bin/ls              → /usr/bin/ls (no transformation)
pub fn prettify_path(path: &Path) -> String {
    let s = path.to_string_lossy();

    // 1. Try $HOME → ~
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && s.starts_with(&home) {
            return format!("~{}", &s[home.len()..]);
        }
    }

    // 2. Try known prefixes (longest first via sort)
    let mut sorted = KNOWN_PREFIXES.to_vec();
    sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    for (prefix, alias) in sorted {
        if s.starts_with(prefix) {
            return format!("{}{}", alias, &s[prefix.len()..]);
        }
    }

    s.into_owned()
}

/// Default directories to scan for executables when no CLI args are given.
/// These cover the most common locations across Arch / Fedora / Ubuntu / NixOS
/// user setups. The user can always override or extend via CLI arguments.
pub fn default_binary_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();

    if !home.is_empty() {
        let home = PathBuf::from(&home);
        for rel in [
            ".local/bin",
            ".cargo/bin",
            ".deno/bin",
            ".bun/bin",
            ".zvm/bin",
            ".local/share/zvm/bin",
        ] {
            dirs.push(home.join(rel));
        }
    }

    for system in [
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        dirs.push(PathBuf::from(system));
    }

    // Linuxbrew
    dirs.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));
    // ROCm
    dirs.push(PathBuf::from("/opt/rocm/bin"));

    dirs
}

/// Merge CLI arguments (custom directories) with the default set, deduplicating.
pub fn merge_binary_dirs(cli_args: &[String]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = cli_args
        .iter()
        .map(|s| {
            let expanded = if let Some(rest) = s.strip_prefix('~') {
                if let Some(home) = dirs::home_dir() {
                    home.join(rest.trim_start_matches('/'))
                } else {
                    PathBuf::from(s)
                }
            } else {
                PathBuf::from(s)
            };
            expanded
        })
        .filter(|p| p.is_dir())
        .collect();

    let mut seen: std::collections::HashSet<PathBuf> = dirs.iter().cloned().collect();
    for d in default_binary_dirs() {
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
    fn prettifies_known_prefix() {
        assert_eq!(
            prettify_path(Path::new("/opt/rocm/bin/rocm-smi")),
            "$ROCM_HOME/bin/rocm-smi"
        );
    }

    #[test]
    fn leaves_system_path_alone() {
        assert_eq!(prettify_path(Path::new("/usr/bin/ls")), "/usr/bin/ls");
    }
}
