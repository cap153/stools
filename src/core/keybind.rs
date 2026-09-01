//! Keybinding parsing and dispatch.
//!
//! The config file describes bindings as `[keybindings.<modifiers>]` tables, for
//! example `[keybindings]`, `[keybindings.none]`, `[keybindings.shift]`,
//! `[keybindings.alt_ctrl_shift]` or `[keybindings."super+shift"]`. Modifier
//! names are split on `+`/`_`, so any order and any combination works.
//!
//! Key names accept the XKB spelling reported by `wev` (`Escape`, `Return`,
//! `Prior`, `space`, …) as well as the common aliases (`esc`, `enter`,
//! `pageup`, …); everything is folded to a canonical lowercase name that the
//! runtime also derives from Slint key events.

use std::collections::HashMap;

/// What a key does once resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Move the selection up.
    Up,
    /// Move the selection down.
    Down,
    /// Linux: quit. Windows: hide the window.
    Close,
    /// Launch the selected entry.
    Execute,
    /// Summon the window (global hotkey on Windows).
    Stools,
}

impl KeyAction {
    /// Parse an action name from the config file.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "up" | "prev" | "previous" => Some(Self::Up),
            "down" | "next" => Some(Self::Down),
            "close" | "hide" | "quit" | "exit" => Some(Self::Close),
            "execute" | "exec" | "launch" | "run" => Some(Self::Execute),
            "stools" | "show" | "summon" => Some(Self::Stools),
            _ => None,
        }
    }

    /// Stable name handed to the UI layer.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Close => "close",
            Self::Execute => "execute",
            Self::Stools => "stools",
        }
    }
}

/// A normalized modifier combination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModifiersMask {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl ModifiersMask {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    pub fn new(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            meta,
        }
    }

    /// Parse a `[keybindings.<name>]` table name. Unknown words are ignored, so
    /// a typo degrades to a weaker combination instead of breaking the file.
    pub fn parse(name: &str) -> Self {
        let mut mask = Self::NONE;
        let lowered = name.trim().to_lowercase();
        if lowered.is_empty() || lowered == "none" {
            return mask;
        }
        for part in lowered.split(['+', '_', '-', ' ']) {
            match part.trim() {
                "ctrl" | "control" => mask.ctrl = true,
                "alt" | "opt" | "option" => mask.alt = true,
                "shift" => mask.shift = true,
                "super" | "win" | "windows" | "meta" | "cmd" | "command" => mask.meta = true,
                "" | "none" => {}
                other => eprintln!("[stools] unknown modifier '{other}' in [keybindings.{name}]"),
            }
        }
        mask
    }
}

/// Canonicalize a key name coming from the config file.
///
/// Single characters stay literal (`"a"`, `"/"`, `"1"`); everything else is
/// matched against the XKB / alias table.
pub fn normalize_key_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(first.to_lowercase().collect());
    }

    let compact: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(|c| c.to_lowercase())
        .collect();

    let canonical = match compact.as_str() {
        "esc" | "escape" => "escape",
        "enter" | "return" | "kpenter" | "kpreturn" => "return",
        "tab" | "backtab" | "isolefttab" => "tab",
        "space" | "spacebar" => "space",
        "backspace" | "bs" => "backspace",
        "delete" | "del" | "kpdelete" => "delete",
        "insert" | "ins" | "kpinsert" => "insert",
        "up" | "uparrow" | "arrowup" | "kpup" => "up",
        "down" | "downarrow" | "arrowdown" | "kpdown" => "down",
        "left" | "leftarrow" | "arrowleft" | "kpleft" => "left",
        "right" | "rightarrow" | "arrowright" | "kpright" => "right",
        "home" | "kphome" => "home",
        "end" | "kpend" => "end",
        "pageup" | "prior" | "kppageup" | "kpprior" => "pageup",
        "pagedown" | "next" | "kppagedown" | "kpnext" => "pagedown",
        "menu" | "contextmenu" => "menu",
        other => return Some(other.to_string()),
    };
    Some(canonical.to_string())
}

/// Canonicalize the `text` of a Slint key event.
///
/// Returns the canonical key name plus whether the event implies Shift
/// (`Backtab` is delivered instead of `Shift`+`Tab` by some backends). Modifier
/// keys pressed on their own, and multi-character (IME) input, yield `None`.
pub fn key_from_event_text(text: &str) -> Option<(String, bool)> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    let canonical = match c {
        '\u{0008}' => "backspace",
        '\u{0009}' => "tab",
        '\u{000a}' => "return",
        '\u{001b}' => "escape",
        '\u{0019}' => return Some(("tab".to_string(), true)),
        '\u{007f}' => "delete",
        '\u{0020}' => "space",
        '\u{F700}' => "up",
        '\u{F701}' => "down",
        '\u{F702}' => "left",
        '\u{F703}' => "right",
        '\u{F727}' => "insert",
        '\u{F729}' => "home",
        '\u{F72B}' => "end",
        '\u{F72C}' => "pageup",
        '\u{F72D}' => "pagedown",
        '\u{F735}' => "menu",
        // Shift / Control / Alt / AltGr / CapsLock / Meta on their own.
        '\u{0010}'..='\u{0018}' => return None,
        // F1..F24 live in a contiguous private-use range.
        c if ('\u{F704}'..='\u{F71B}').contains(&c) => {
            return Some((format!("f{}", c as u32 - 0xF704 + 1), false));
        }
        c => return Some((c.to_lowercase().collect(), false)),
    };
    Some((canonical.to_string(), false))
}

/// The name of the `keyboard-types` `Code` matching a canonical key, used to
/// register global hotkeys on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn hotkey_code_name(canonical: &str) -> Option<String> {
    let name = match canonical {
        "escape" => "Escape",
        "return" => "Enter",
        "tab" => "Tab",
        "space" => "Space",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "insert" => "Insert",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "menu" => "ContextMenu",
        "-" => "Minus",
        "=" => "Equal",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "\\" => "Backslash",
        ";" => "Semicolon",
        "'" => "Quote",
        "`" => "Backquote",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        other => {
            let mut chars = other.chars();
            let first = chars.next()?;
            if chars.next().is_none() {
                return match first {
                    'a'..='z' => Some(format!("Key{}", first.to_ascii_uppercase())),
                    '0'..='9' => Some(format!("Digit{first}")),
                    _ => None,
                };
            }
            if let Some(digits) = other.strip_prefix('f') {
                if matches!(digits.parse::<u32>(), Ok(1..=24)) {
                    return Some(format!("F{digits}"));
                }
            }
            return None;
        }
    };
    Some(name.to_string())
}

/// The resolved (modifiers, key) → action table: built-in defaults first, then
/// overridden and extended by the config file.
#[derive(Debug, Clone)]
pub struct KeybindingMap {
    bindings: HashMap<(ModifiersMask, String), KeyAction>,
}

impl Default for KeybindingMap {
    fn default() -> Self {
        Self::from_config(&HashMap::new())
    }
}

impl KeybindingMap {
    /// Built-in defaults, identical to the generated config template.
    const DEFAULTS: &'static [(ModifiersMask, &'static str, KeyAction)] = &[
        (ModifiersMask::NONE, "tab", KeyAction::Down),
        (ModifiersMask::NONE, "escape", KeyAction::Close),
        (ModifiersMask::NONE, "return", KeyAction::Execute),
        (ModifiersMask::NONE, "up", KeyAction::Up),
        (ModifiersMask::NONE, "down", KeyAction::Down),
        (
            ModifiersMask {
                ctrl: false,
                alt: false,
                shift: true,
                meta: false,
            },
            "tab",
            KeyAction::Up,
        ),
        (
            ModifiersMask {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            "a",
            KeyAction::Stools,
        ),
    ];

    pub fn from_config(sections: &HashMap<String, HashMap<String, String>>) -> Self {
        let mut bindings: HashMap<(ModifiersMask, String), KeyAction> = Self::DEFAULTS
            .iter()
            .map(|(mask, key, action)| ((*mask, (*key).to_string()), *action))
            .collect();

        for (section, keys) in sections {
            let mask = ModifiersMask::parse(section);
            for (key, action) in keys {
                let Some(key) = normalize_key_name(key) else {
                    continue;
                };
                match KeyAction::parse(action) {
                    Some(action) => {
                        bindings.insert((mask, key), action);
                    }
                    None => eprintln!(
                        "[stools] unknown action '{action}' for key '{key}' in [keybindings.{section}]"
                    ),
                }
            }
        }

        Self { bindings }
    }

    /// Resolve a canonical key plus modifiers.
    pub fn resolve(&self, mask: ModifiersMask, key: &str) -> Option<KeyAction> {
        self.bindings.get(&(mask, key.to_lowercase())).copied()
    }

    /// Resolve a Slint key event.
    pub fn resolve_event(
        &self,
        text: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
    ) -> Option<KeyAction> {
        let (key, implies_shift) = key_from_event_text(text)?;
        let mask = ModifiersMask::new(ctrl, alt, shift || implies_shift, meta);
        self.resolve(mask, &key)
    }

    /// The binding that summons the window, used to register the Windows global
    /// hotkey. Sorted so a config with several `"stools"` bindings still picks a
    /// stable one.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn summon_binding(&self) -> Option<(ModifiersMask, String)> {
        self.bindings
            .iter()
            .filter(|(_, action)| **action == KeyAction::Stools)
            .map(|((mask, key), _)| (*mask, key.clone()))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(section: &str, key: &str, action: &str) -> HashMap<String, HashMap<String, String>> {
        HashMap::from([(
            section.to_string(),
            HashMap::from([(key.to_string(), action.to_string())]),
        )])
    }

    #[test]
    fn parses_modifier_combinations() {
        assert_eq!(ModifiersMask::parse(""), ModifiersMask::NONE);
        assert_eq!(ModifiersMask::parse("none"), ModifiersMask::NONE);
        assert_eq!(
            ModifiersMask::parse("alt_shift"),
            ModifiersMask::new(false, true, true, false)
        );
        assert_eq!(
            ModifiersMask::parse("super+shift"),
            ModifiersMask::new(false, false, true, true)
        );
        assert_eq!(
            ModifiersMask::parse("Alt_Ctrl_Shift"),
            ModifiersMask::new(true, true, true, false)
        );
    }

    #[test]
    fn normalizes_xkb_and_alias_names() {
        assert_eq!(normalize_key_name("Escape").as_deref(), Some("escape"));
        assert_eq!(normalize_key_name("esc").as_deref(), Some("escape"));
        assert_eq!(normalize_key_name("Return").as_deref(), Some("return"));
        assert_eq!(normalize_key_name("Prior").as_deref(), Some("pageup"));
        assert_eq!(normalize_key_name("page_down").as_deref(), Some("pagedown"));
        assert_eq!(normalize_key_name("A").as_deref(), Some("a"));
        assert_eq!(normalize_key_name("/").as_deref(), Some("/"));
        assert_eq!(normalize_key_name("  ").as_deref(), None);
    }

    #[test]
    fn maps_slint_event_text() {
        assert_eq!(
            key_from_event_text("\u{1b}"),
            Some(("escape".into(), false))
        );
        assert_eq!(key_from_event_text("\u{9}"), Some(("tab".into(), false)));
        assert_eq!(key_from_event_text("\u{19}"), Some(("tab".into(), true)));
        assert_eq!(
            key_from_event_text("\u{F701}"),
            Some(("down".into(), false))
        );
        assert_eq!(key_from_event_text("\u{F704}"), Some(("f1".into(), false)));
        assert_eq!(key_from_event_text("A"), Some(("a".into(), false)));
        assert_eq!(key_from_event_text("\u{10}"), None);
        assert_eq!(key_from_event_text("汉字"), None);
    }

    #[test]
    fn default_bindings_are_active() {
        let map = KeybindingMap::default();
        assert_eq!(
            map.resolve_event("\u{9}", false, false, false, false),
            Some(KeyAction::Down)
        );
        assert_eq!(
            map.resolve_event("\u{9}", false, false, true, false),
            Some(KeyAction::Up)
        );
        assert_eq!(
            map.resolve_event("\u{19}", false, false, false, false),
            Some(KeyAction::Up)
        );
        assert_eq!(
            map.resolve_event("\u{1b}", false, false, false, false),
            Some(KeyAction::Close)
        );
        assert_eq!(
            map.resolve_event("\u{a}", false, false, false, false),
            Some(KeyAction::Execute)
        );
        assert_eq!(map.resolve_event("x", false, false, false, false), None);
    }

    #[test]
    fn config_extends_and_overrides_defaults() {
        let map = KeybindingMap::from_config(&config("ctrl", "u", "up"));
        assert_eq!(
            map.resolve_event("u", true, false, false, false),
            Some(KeyAction::Up)
        );
        // Untouched defaults survive.
        assert_eq!(
            map.resolve_event("\u{1b}", false, false, false, false),
            Some(KeyAction::Close)
        );

        let map = KeybindingMap::from_config(&config("", "Return", "close"));
        assert_eq!(
            map.resolve_event("\u{a}", false, false, false, false),
            Some(KeyAction::Close)
        );
    }

    #[test]
    fn summon_binding_defaults_to_alt_a() {
        let map = KeybindingMap::default();
        let (mask, key) = map.summon_binding().expect("default summon binding");
        assert_eq!(mask, ModifiersMask::new(false, true, false, false));
        assert_eq!(key, "a");
        assert_eq!(hotkey_code_name(&key).as_deref(), Some("KeyA"));
    }

    #[test]
    fn hotkey_code_names() {
        assert_eq!(hotkey_code_name("space").as_deref(), Some("Space"));
        assert_eq!(hotkey_code_name("f12").as_deref(), Some("F12"));
        assert_eq!(hotkey_code_name("3").as_deref(), Some("Digit3"));
        assert_eq!(hotkey_code_name("return").as_deref(), Some("Enter"));
        assert_eq!(hotkey_code_name("unknownkey"), None);
    }
}
