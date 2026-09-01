//! Configuration file handling: cross-platform location, first-run template
//! generation, deserialization and path expansion.
//!
//!   Linux   : `$XDG_CONFIG_HOME/stools/config.toml` (→ `~/.config/stools/config.toml`)
//!   Windows : `%APPDATA%\stools\config.toml`
//!
//! Every field is optional: a missing file, a missing table or an unparsable
//! value all fall back to the built-in (Dracula) defaults, so the launcher never
//! refuses to start because of a broken config.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Extra directories to scan for executables (and `.desktop` files).
    pub path: Vec<String>,
    /// Colours and fonts.
    pub theme: ThemeConfig,
    /// `modifier table name` → (`key name` → `action name`).
    ///
    /// The table name is the modifier combination (`""`/`none`, `shift`,
    /// `ctrl_alt`, `"super+shift"`, …); it is parsed by
    /// [`crate::core::keybind::ModifiersMask::parse`]. Bindings written directly
    /// under `[keybindings]` (no modifier) are collected under the `""` name.
    #[serde(deserialize_with = "deserialize_keybindings")]
    pub keybindings: HashMap<String, HashMap<String, String>>,
}

/// `[keybindings]` mixes plain `key = "action"` pairs (no modifier) with nested
/// `[keybindings.<modifiers>]` tables, so it cannot be deserialized straight
/// into a nested map: the flat pairs are folded into the `""` table instead.
fn deserialize_keybindings<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, HashMap<String, String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = HashMap::<String, toml::Value>::deserialize(deserializer)?;
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();

    for (name, value) in raw {
        match value {
            toml::Value::String(action) => {
                out.entry(String::new()).or_default().insert(name, action);
            }
            toml::Value::Table(table) => {
                let section = out.entry(name).or_default();
                for (key, action) in table {
                    if let Some(action) = action.as_str() {
                        section.insert(key, action.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    Ok(out)
}

/// Colour + font settings. Key names mirror Fuzzel's `[colors]`/`[main]`
/// sections so existing Fuzzel themes can be pasted in unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub background: String,
    pub text: String,
    #[serde(rename = "match", alias = "match_color")]
    pub match_color: String,
    #[serde(rename = "selection-match", alias = "selection_match")]
    pub selection_match: String,
    pub selection: String,
    #[serde(rename = "selection-text", alias = "selection_text")]
    pub selection_text: String,
    pub border: String,
    /// Font families in priority order; the first one installed wins.
    pub font: Vec<String>,
}

impl Default for ThemeConfig {
    /// The built-in Dracula theme (identical to the generated template).
    fn default() -> Self {
        Self {
            background: "282a36dd".into(),
            text: "f8f8f2ff".into(),
            match_color: "8be9fdff".into(),
            selection_match: "8be9fdff".into(),
            selection: "44475add".into(),
            selection_text: "f8f8f2ff".into(),
            border: "bd93f9ff".into(),
            font: vec!["JetBrains Mono".into()],
        }
    }
}

/// Written verbatim on first run; documents every knob in English.
pub const DEFAULT_CONFIG_TEMPLATE: &str = r##"# =============================================================================
# stools launcher configuration
#
#   Linux   : $XDG_CONFIG_HOME/stools/config.toml  (usually ~/.config/stools/config.toml)
#   Windows : %APPDATA%\stools\config.toml
#
# This file was generated on first run and only contains defaults, so deleting
# it (or any single entry) simply restores the stock behaviour.
# =============================================================================

# -----------------------------------------------------------------------------
# Extra directories to search, in addition to the built-in ones
# (~/.local/bin, ~/.cargo/bin, ~/.deno/bin, /usr/bin, /usr/local/bin, ...).
# Executables are picked up everywhere; on Linux .desktop files placed in these
# directories are indexed as well.
#
# "~", "$VAR", "${VAR}" and "%VAR%" are expanded. Directories that do not exist
# are silently ignored.
# -----------------------------------------------------------------------------
path = [
    "$HOME/.cargo/bin",
    "~/.deno/bin",
]

# -----------------------------------------------------------------------------
# Keybindings: override the defaults or add new ones.
#
# Actions:
#   "down"    - select the next entry
#   "up"      - select the previous entry
#   "execute" - launch the selected entry
#   "close"   - Linux: quit stools / Windows: hide the window
#   "stools"  - summon the window (registered as a global hotkey on Windows)
#
# Key names are case-insensitive and accept both the XKB spelling reported by
# `wev` (Escape, Return, Tab, Up, Down, Prior, Next, space, ...) and the usual
# aliases (esc, enter, pageup, pagedown, ...). Single characters are literal
# keys ("a", "u", "/").
#
# The table name is the modifier combination; modifiers may be combined freely
# and in any order:
#   [keybindings]                 - no modifier
#   [keybindings.none]            - same as above
#   [keybindings.shift]
#   [keybindings.ctrl]
#   [keybindings.alt_shift]
#   [keybindings.alt_ctrl_shift]
#   [keybindings."super+shift"]   - quote the name when using "+"
# Recognised modifier words: ctrl (control), alt (option), shift, super
# (win, meta, cmd).
# -----------------------------------------------------------------------------
[keybindings]
tab = "down"       # select the next entry
esc = "close"      # Linux: quit stools / Windows: hide the window
Return = "execute" # launch the selected entry
Up = "up"
Down = "down"

[keybindings.shift]
tab = "up"         # select the previous entry

[keybindings.alt]
a = "stools"       # summon the window (global hotkey on Windows)

# [keybindings.ctrl]
# u = "up"         # example: add ctrl+u to select the previous entry
# e = "down"       # example: add ctrl+e to select the next entry

# -----------------------------------------------------------------------------
# Theme. Colours use Fuzzel's RRGGBBAA hex notation (a leading '#' is allowed,
# RGB / RGBA / RRGGBB are accepted too), so Fuzzel themes can be reused as-is.
# The defaults below are Dracula.
# -----------------------------------------------------------------------------
[theme]
background = "282a36dd"
text = "f8f8f2ff"
match = "8be9fdff"
selection-match = "8be9fdff"
selection = "44475add"
selection-text = "f8f8f2ff"
border = "bd93f9ff"

# Font families in priority order: the first family installed on the system is
# used, so a list can cover Latin and CJK setups at once. Glyphs missing from
# that family are resolved through the system font fallback.
font = [
    "ComicShannsMono Nerd Font",
    "LXGW WenKai GB Screen",
    "JetBrains Mono",
]
"##;

impl Config {
    /// `<config dir>/stools/config.toml` for the current platform.
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("stools")
            .join("config.toml")
    }

    /// Load the config file, creating it from [`DEFAULT_CONFIG_TEMPLATE`] when
    /// it (or its parent directory) does not exist yet. Any I/O or parse error
    /// degrades to the built-in defaults.
    pub fn load_or_create() -> Self {
        let path = Self::config_path();

        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&path, DEFAULT_CONFIG_TEMPLATE);
            return toml::from_str(DEFAULT_CONFIG_TEMPLATE).unwrap_or_default();
        }

        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("[stools] {}: {err}", path.display());
                Self::default()
            }
        }
    }
}

/// Expand `~`, `$VAR`, `${VAR}` and `%VAR%` in a configured path.
/// Returns `None` for empty input.
pub fn expand_path(raw: &str) -> Option<PathBuf> {
    let expanded = expand_vars(raw.trim());
    if expanded.is_empty() {
        return None;
    }
    if let Some(rest) = expanded.strip_prefix('~') {
        let home = dirs::home_dir()?;
        let rest = rest.trim_start_matches(['/', '\\']);
        return Some(if rest.is_empty() {
            home
        } else {
            home.join(rest)
        });
    }
    Some(PathBuf::from(expanded))
}

/// Look up an environment variable, treating `HOME` / `USERPROFILE` as the home
/// directory even on platforms where they are not set.
fn env_var(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        if !value.is_empty() {
            return Some(value);
        }
    }
    if matches!(name, "HOME" | "USERPROFILE") {
        return dirs::home_dir().map(|h| h.to_string_lossy().into_owned());
    }
    None
}

/// Substitute `$VAR`, `${VAR}` and `%VAR%`. Unknown names are left untouched so
/// the original text stays visible in error messages.
fn expand_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '$' => {
                let braced = chars.peek() == Some(&'{');
                if braced {
                    chars.next();
                }
                let mut name = String::new();
                while let Some(&next) = chars.peek() {
                    if braced {
                        chars.next();
                        if next == '}' {
                            break;
                        }
                        name.push(next);
                    } else if next.is_alphanumeric() || next == '_' {
                        chars.next();
                        name.push(next);
                    } else {
                        break;
                    }
                }
                match env_var(&name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('$');
                        if braced {
                            out.push('{');
                            out.push_str(&name);
                            out.push('}');
                        } else {
                            out.push_str(&name);
                        }
                    }
                }
            }
            '%' => {
                let mut name = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '%' {
                        closed = true;
                        break;
                    }
                    name.push(next);
                }
                match env_var(&name).filter(|_| closed) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(&name);
                        if closed {
                            out.push('%');
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_matches_builtin_defaults() {
        let cfg: Config = toml::from_str(DEFAULT_CONFIG_TEMPLATE).expect("template must parse");
        let theme = ThemeConfig::default();
        assert_eq!(cfg.theme.background, theme.background);
        assert_eq!(cfg.theme.text, theme.text);
        assert_eq!(cfg.theme.match_color, theme.match_color);
        assert_eq!(cfg.theme.selection_match, theme.selection_match);
        assert_eq!(cfg.theme.selection, theme.selection);
        assert_eq!(cfg.theme.selection_text, theme.selection_text);
        assert_eq!(cfg.theme.border, theme.border);
        assert!(!cfg.path.is_empty());
        // [keybindings], [keybindings.shift] and [keybindings.alt]
        assert_eq!(cfg.keybindings.len(), 3);
        assert_eq!(cfg.keybindings[""]["esc"], "close");
        assert_eq!(cfg.keybindings["shift"]["tab"], "up");
        assert_eq!(cfg.keybindings["alt"]["a"], "stools");
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let cfg: Config = toml::from_str("[theme]\nborder = \"ff79c6ff\"\n").unwrap();
        assert_eq!(cfg.theme.border, "ff79c6ff");
        assert_eq!(cfg.theme.background, ThemeConfig::default().background);
        assert!(cfg.path.is_empty());
    }

    #[test]
    fn expands_home_and_env_vars() {
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(expand_path("~/.cargo/bin"), Some(home.join(".cargo/bin")));
        assert_eq!(
            expand_path("$HOME/.cargo/bin"),
            Some(home.join(".cargo/bin"))
        );
        assert_eq!(
            expand_path("${HOME}/.cargo/bin"),
            Some(home.join(".cargo/bin"))
        );
    }

    #[test]
    fn keeps_unknown_vars_verbatim() {
        assert_eq!(
            expand_path("$STOOLS_NOT_SET_XYZ/bin"),
            Some(PathBuf::from("$STOOLS_NOT_SET_XYZ/bin"))
        );
        assert_eq!(
            expand_path("%STOOLS_NOT_SET_XYZ%/bin"),
            Some(PathBuf::from("%STOOLS_NOT_SET_XYZ%/bin"))
        );
    }
}
