use serde::{Deserialize, Serialize};
use bincode::{Decode, Encode};

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
    /// Whether the app should be hidden from results (e.g. desktop files marked Hidden/NoDisplay).
    pub hidden: bool,
    /// Concatenated full pinyin (e.g. "wangyiyunyinyue").
    pub pinyin_full: String,
    /// Concatenated initials pinyin (e.g. "wyyyy").
    pub pinyin_abbr: String,
}
