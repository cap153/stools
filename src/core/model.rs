use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::core::matcher::FieldIndices;

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode, PartialEq, Eq)]
pub enum EntryKind {
    Desktop,
    Binary,
}

/// A single indexed application entry, shared by the Linux and Windows backends.
///
/// Pinyin fields are precomputed once at scan time so searching is just a cheap
/// string comparison instead of a per-keystroke conversion.
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct AppEntry {
    /// Stable, unique identifier (desktop file id on Linux / .lnk path hash on Windows).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The command / target used to launch the application.
    pub exec: String,
    /// Resolved path to an icon file, if any.
    pub icon_path: Option<String>,
    /// Whether the app should be hidden from results.
    pub hidden: bool,
    /// Concatenated full pinyin (e.g. "wangyiyunyinyue").
    pub pinyin_full: String,
    /// Concatenated initials pinyin (e.g. "wyyyy").
    pub pinyin_abbr: String,
    /// Whether this is a .desktop entry or a raw binary.
    pub kind: EntryKind,
    /// Optional subtitle shown below the name (e.g. shortened path for binaries).
    pub subtitle: Option<String>,
    /// Character ranges of `pinyin_abbr` / `pinyin_full` owned by each character
    /// of `name` — the reverse map used to highlight the original characters
    /// when a query hits pinyin instead of the text itself.
    #[serde(default)]
    pub pinyin_indices: FieldIndices,
}
