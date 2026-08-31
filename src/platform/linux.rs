use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use nucleo_matcher::{Matcher, Config as MatcherConfig};

use crate::core::indexer;
use crate::core::matcher;
use crate::core::matcher::pinyin_fields;
use crate::core::model::AppEntry;
use crate::launcher::{build_model, LauncherWindow};

use slint::{ComponentHandle, Model};


/// Directorys scanned for `.desktop` files, in priority order (later wins).
fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // Standard user locations first so the user's own entries take priority.
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
        dirs.push(PathBuf::from(&home).join(".local/share/flatpak/exports/share/applications"));
    }
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_default();
    if !data_home.is_empty() {
        dirs.push(PathBuf::from(&data_home).join("applications"));
    }
    dirs.push(PathBuf::from("/usr/local/share/applications"));
    dirs.push(PathBuf::from("/usr/share/applications"));
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for d in data_dirs.split(':') {
            if !d.is_empty() {
                dirs.push(PathBuf::from(d).join("applications"));
            }
        }
    }
    // De-duplicate while preserving order.
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

/// Icon root directories that may contain the icon themes.
fn icon_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        dirs.push(PathBuf::from(&home).join(".local/share/icons"));
    }
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        if !data_home.is_empty() {
            dirs.push(PathBuf::from(data_home).join("icons"));
        }
    }
    dirs.push(PathBuf::from("/usr/local/share/icons"));
    dirs.push(PathBuf::from("/usr/share/icons"));
    dirs.push(PathBuf::from("/usr/share/pixmaps"));
    dirs
}

/// Resolve a desktop entry `Icon=` value to an existing file path if possible.
fn resolve_icon(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let p = Path::new(value);
    if p.is_absolute() {
        for ext in ["png", "svg", "xpm", "jpg", "jpeg", "webp", "ico"] {
            let cand = p.with_extension(ext);
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
        // Accept the bare path only if it already has a recognized image extension.
        if let Some(e) = p.extension().and_then(|e| e.to_str()) {
            if matches!(e, "png" | "svg" | "xpm" | "jpg" | "jpeg" | "webp" | "ico") && p.is_file()
            {
                return Some(p.to_string_lossy().into_owned());
            }
        }
        return None;
    }

    // Otherwise search icon theme directories. We do a bounded recursive scan
    // for a file whose stem matches the requested name.
    for root in icon_dirs() {
        if let Some(found) = search_icon(&root, value) {
            return Some(found);
        }
    }
    None
}

fn search_icon(root: &Path, name: &str) -> Option<String> {
    // Skip known scale/cached dirs to bound the search.
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > 4000 {
            return None;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = entry.file_type();
            if ft.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
                if entry.file_name() != "cursors" && entry.file_name() != "scalable" {
                    stack.push(path);
                }
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem == name {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "png" | "svg" | "xpm" | "jpg" | "jpeg" | "webp" | "ico") {
                        return Some(path.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    None
}

fn parse_bool(v: Option<&str>, def: bool) -> bool {
    match v.map(|s| s.trim().to_lowercase()).as_deref() {
        Some("true" | "1" | "yes") => true,
        Some("false" | "0" | "no") => false,
        _ => def,
    }
}

/// Parse a single `.desktop` file into an optional `AppEntry`.
fn parse_desktop(path: &Path, id: &str) -> Option<AppEntry> {
    let content = fs::read_to_string(path).ok()?;
    let mut name = None::<String>;
    let mut exec = None::<String>;
    let mut icon = None::<String>;
    let mut hidden = false;
    let mut no_display = false;
    let mut is_application = false;
    let mut in_desktop = false;
    // Locale name keys take priority: Name[zh_CN], Name[zh], etc.
    let mut localized_names: Vec<String> = Vec::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let (key, value) = (line[..eq].trim(), line[eq + 1..].trim());
        match key {
            "Type" => is_application = value == "Application",
            "Name" => name = Some(value.to_string()),
            "Exec" => exec = Some(value.to_string()),
            "Icon" => icon = Some(value.to_string()),
            "Hidden" => hidden = parse_bool(Some(value), false),
            "NoDisplay" => no_display = parse_bool(Some(value), false),
            _ => {
                if let Some(rest) = key.strip_prefix("Name[") {
                    if rest.ends_with(']') {
                        localized_names.push(value.to_string());
                    }
                }
            }
        }
    }

    if !is_application {
        return None;
    }
    // Prefer the most specific localized name we happen to find.
    let name_str = localized_names.pop().or(name)?;

    let exec_value = exec?;
    let (pinyin_full, pinyin_abbr) = pinyin_fields(&name_str);

    Some(AppEntry {
        id: id.to_string(),
        name: name_str,
        exec: exec_value,
        icon_path: icon.and_then(|i| resolve_icon(&i)),
        hidden: hidden || no_display,
        pinyin_full,
        pinyin_abbr,
    })
}

/// Scan all `.desktop` files and build the full app list.
pub fn scan_apps() -> Vec<AppEntry> {
    let mut entries = Vec::new();
    let mut seen_ids = HashSet::new();
    for dir in desktop_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort(); // deterministic order
        for path in files {
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if !seen_ids.insert(id.clone()) {
                continue; // already handled by a higher-priority dir
            }
            if let Some(entry) = parse_desktop(&path, &id) {
                entries.push(entry);
            }
        }
    }
    entries
}

/// Load the app list, preferring the on-disk cache and lazily refreshing it.
pub fn load_apps() -> Vec<AppEntry> {
    // Fast path: read the cache and trust it for this run.
    if let Some(cached) = indexer::load_cache() {
        // Fire-and-forget a background rescan that refreshes the cache for next time.
        let fresh = scan_apps();
        if !fresh.is_empty() {
            indexer::save_cache(&fresh);
        }
        return cached;
    }
    let apps = scan_apps();
    indexer::save_cache(&apps);
    apps
}

/// Launch a `.desktop` Exec string, detached from this process.
fn launch_exec(exec: &str) -> bool {
    // Strip desktop field codes and split tokens.
    let cleaned: String = exec
        .split_whitespace()
        .filter(|tok| !tok.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts = cleaned.split_whitespace();
    let Some(program) = parts.next() else { return false };
    let args: Vec<&str> = parts.collect();
    match Command::new(program).args(&args).spawn() {
        // The child is reparented to init once we exit; no wait is required.
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Build the Slint UI and run the single-shot Linux loop.
pub fn run() {
    let apps = load_apps();

    let ui = LauncherWindow::new().unwrap();
    let weak = ui.as_weak();

    // Pre-seed the list with everything.
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let mut scratch = matcher::MatcherScratch::default();
    ui.set_items(build_model(&apps, &[]));

    // Refresh the visible list based on the current query.
    let search_weak = weak.clone();
    ui.on_search_changed(move |query| {
        let ui = search_weak.upgrade();
        let Some(ui) = ui else { return };
        let query = query.to_string();
        let idxs = matcher::rank(&apps, &query, &mut matcher, &mut scratch);
        ui.set_items(build_model(&apps, &idxs));
        ui.set_selected_index(0);
    });

    let exec_weak = weak.clone();
    ui.on_item_executed(move |index| {
        if let Some(ui) = exec_weak.upgrade() {
            let items = ui.get_items();
            if let Some(item) = items.row_data(index as usize) {
                let exec = item.exec.to_string();
                launch_exec(&exec);
            }
            let _ = slint::quit_event_loop();
        }
    });

    ui.on_escape_pressed(move || {
        let _ = slint::quit_event_loop();
    });

    // Let the window manager place us (float + center via hyprland/sway rules).
    ui.show().unwrap();
    slint::run_event_loop_until_quit().unwrap();
}
