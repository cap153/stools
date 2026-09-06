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
///
/// Every field that is written once at scan time and never mutated is a
/// `Box<str>`, not a `String`: 16 bytes instead of 24, and no spare capacity
/// hanging off the allocation. With a few thousand entries indexed that is a
/// few hundred kilobytes, plus a smaller per-entry footprint for the matcher to
/// walk. (Bincode encodes `Box<str>` and `String` identically, so the on-disk
/// cache is unaffected apart from the version bump.)
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct AppEntry {
    /// Stable, unique identifier (desktop file id on Linux / .lnk path hash on Windows).
    pub id: Box<str>,
    /// Display name.
    pub name: Box<str>,
    /// The command / target used to launch the application.
    pub exec: Box<str>,
    /// Resolved path to an icon file, if any.
    pub icon_path: Option<Box<str>>,
    /// Whether the app should be hidden from results.
    pub hidden: bool,
    /// Concatenated full pinyin (e.g. "wangyiyunyinyue").
    pub pinyin_full: Box<str>,
    /// Concatenated initials pinyin (e.g. "wyyyy").
    pub pinyin_abbr: Box<str>,
    /// Whether this is a .desktop entry or a raw binary.
    pub kind: EntryKind,
    /// Optional subtitle shown below the name (e.g. shortened path for binaries).
    pub subtitle: Option<Box<str>>,
    /// Character ranges of `pinyin_abbr` / `pinyin_full` owned by each character
    /// of `name` — the reverse map used to highlight the original characters
    /// when a query hits pinyin instead of the text itself.
    #[serde(default)]
    pub pinyin_indices: FieldIndices,
    /// Whether this entry is a secondary-language alias of another `.desktop`
    /// entry (generated when a `.desktop` carries both `Name` and `Name[zh_CN]`).
    /// Aliases are hidden when the query is empty — so the first screen shows only
    /// the primary (locale-appropriate) name — but they take part in search, so an
    /// English query can hit the English name while a Chinese query hits the Chinese
    /// one, each highlighted in its own script.
    #[serde(default)]
    pub is_alias: bool,
}
