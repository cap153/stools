use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::core::indexer;
use crate::core::matcher;
use crate::core::model::AppEntry;
use crate::launcher::{build_model, LauncherWindow};

use slint::{ComponentHandle, Model};

// ---------------------------------------------------------------------------
// Desktop file directories
// ---------------------------------------------------------------------------

fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
        dirs.push(
            PathBuf::from(&home)
                .join(".local/share/flatpak/exports/share/applications"),
        );
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
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

// ---------------------------------------------------------------------------
// Icon map: single bounded scan → HashMap<name, path> for O(1) lookups
// ---------------------------------------------------------------------------

const ICON_SCAN_MAX_DIRS: usize = 5000;

fn build_icon_map() -> HashMap<String, PathBuf> {
    let mut map = HashMap::with_capacity(512);

    // 1) scan hicolor / user icon themes (bounded BFS, depth ≤ 3)
    let mut roots: Vec<PathBuf> = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        roots.push(PathBuf::from(&home).join(".local/share/icons"));
    }
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        if !data_home.is_empty() {
            roots.push(PathBuf::from(data_home).join("icons"));
        }
    }
    roots.push(PathBuf::from("/usr/local/share/icons"));
    roots.push(PathBuf::from("/usr/share/icons"));

    let mut stack: Vec<(PathBuf, usize)> = roots.into_iter().map(|p| (p, 0)).collect();
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > ICON_SCAN_MAX_DIRS || depth > 3 {
            continue;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Some(name) = entry.file_name().to_str().map(String::from) else {
                continue
            };
            let ft = entry.file_type();
            if ft.as_ref().map_or(true, |t| t.is_file()) {
                // found a file — record it if it's a valid image in a "known" location
                if is_icon_file(&name) {
                    let subdir_name = dir
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();
                    if subdir_name == "apps" || dir.parent().is_some_and(|p| {
                        p.file_name().is_some_and(|s| s == "pixmaps")
                    }) {
                        let stem = name
                            .rsplit_once('.')
                            .map_or(name.as_str(), |(s, _)| s);
                        map.entry(stem.to_string())
                            .or_insert_with(|| entry.path());
                    }
                }
                continue;
            }
            if name == "cursors" || name == "@2x" || name.starts_with('.') {
                continue;
            }
            stack.push((entry.path(), depth + 1));
        }
    }

    // 2) flat scan /usr/share/pixmaps (all files are icons there)
    for dir in [
        PathBuf::from("/usr/share/pixmaps"),
        PathBuf::from("/usr/local/share/pixmaps"),
    ] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Some(name) = entry.file_name().to_str().map(String::from) else {
                continue
            };
            if entry.file_type().as_ref().map_or(false, |t| t.is_file())
                && is_icon_file(&name)
            {
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
                map.entry(stem.to_string())
                    .or_insert_with(|| entry.path());
            }
        }
    }

    map
}

fn is_icon_file(name: &str) -> bool {
    matches!(
        name.rsplit_once('.').map(|(_, e)| e),
        Some("png" | "svg" | "xpm" | "jpg" | "jpeg" | "webp" | "ico")
    )
}

/// Resolve an Icon= value to an absolute path using the pre-built map.
/// Absolute paths with a valid extension are accepted as-is.
fn resolve_icon(value: &str, icon_map: &HashMap<String, PathBuf>) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let p = Path::new(value);
    if p.is_absolute() {
        if is_icon_file(value) && p.is_file() {
            return Some(value.to_string());
        }
        for ext in ["png", "svg", "xpm", "jpg", "jpeg", "webp", "ico"] {
            let cand = p.with_extension(ext);
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
        return None;
    }

    // Try the exact name first (Icon=firefox), then fall back to the stem in
    // case the entry wrote an explicit extension (Icon=firefox.svg).
    let stem = value.rsplit_once('.').map_or(value, |(s, _)| s);
    icon_map
        .get(value)
        .or_else(|| icon_map.get(stem))
        .map(|p| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// .desktop file parsing
// ---------------------------------------------------------------------------

fn parse_bool(v: Option<&str>, def: bool) -> bool {
    match v.map(|s| s.trim().to_lowercase()).as_deref() {
        Some("true" | "1" | "yes") => true,
        Some("false" | "0" | "no") => false,
        _ => def,
    }
}

fn parse_desktop(
    path: &Path,
    id: &str,
    icon_map: &HashMap<String, PathBuf>,
) -> Option<AppEntry> {
    let content = fs::read_to_string(path).ok()?;
    let mut name = None::<String>;
    let mut exec = None::<String>;
    let mut icon = None::<String>;
    let mut hidden = false;
    let mut no_display = false;
    let mut is_application = false;
    let mut in_desktop = false;
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
    let name_str = localized_names.pop().or(name)?;
    let exec_value = exec?;
    let (pinyin_full, pinyin_abbr) = matcher::pinyin_fields(&name_str);

    Some(AppEntry {
        id: id.to_string(),
        name: name_str,
        exec: exec_value,
        icon_path: icon.and_then(|i| resolve_icon(&i, icon_map)),
        hidden: hidden || no_display,
        pinyin_full,
        pinyin_abbr,
    })
}

// ---------------------------------------------------------------------------
// Full scan (icon map built once, then all desktop files resolved via it)
// ---------------------------------------------------------------------------

fn scan_apps() -> Vec<AppEntry> {
    let icon_map = build_icon_map();

    let mut entries = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for dir in desktop_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
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
                continue;
            }
            if let Some(entry) = parse_desktop(&path, &id, &icon_map) {
                entries.push(entry);
            }
        }
    }
    entries
}

// ---------------------------------------------------------------------------
// Cache with true background refresh
// ---------------------------------------------------------------------------

pub fn load_apps() -> Vec<AppEntry> {
    if let Some(cached) = indexer::load_cache() {
        // Real background refresh — doesn't block the main thread.
        std::thread::spawn(|| {
            let fresh = scan_apps();
            if !fresh.is_empty() {
                indexer::save_cache(&fresh);
            }
        });
        return cached;
    }
    // Cold start: scan synchronously (only happens once).
    let apps = scan_apps();
    indexer::save_cache(&apps);
    apps
}

// ---------------------------------------------------------------------------
// Launch helper
// ---------------------------------------------------------------------------

fn launch_exec(exec: &str) {
    let cleaned: String = exec
        .split_whitespace()
        .filter(|tok| !tok.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts = cleaned.split_whitespace();
    let Some(program) = parts.next() else { return };
    let args: Vec<&str> = parts.collect();
    let _ = Command::new(program).args(&args).spawn();
}

// ---------------------------------------------------------------------------
// Run: load → build UI → wire callbacks → show
// ---------------------------------------------------------------------------

pub fn run() {
    let t0 = std::time::Instant::now();
    let apps = load_apps();
    let app_count = apps.len();
    let t_load = t0.elapsed();

    let ui = LauncherWindow::new().unwrap();
    let t_new = t0.elapsed();
    let weak = ui.as_weak();

    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let mut scratch = matcher::MatcherScratch::default();
    let image_cache = crate::launcher::AppImageCache::new();
    let initial_idxs: Vec<usize> = (0..apps.len()).collect();
    ui.set_items(build_model(&apps, &initial_idxs, &image_cache));
    let t_model = t0.elapsed();
    if std::env::var("STOOLS_DEBUG").is_ok() {
        eprintln!("[stools] initial-n={}", ui.get_items().row_count());
    }

    let search_weak = weak.clone();
    ui.on_search_changed(move |query| {
        let Some(ui) = search_weak.upgrade() else { return };
        let st = std::time::Instant::now();
        let idxs = matcher::rank(&apps, &query.to_string(), &mut matcher, &mut scratch);
        ui.set_items(build_model(&apps, &idxs, &image_cache));
        ui.set_selected_index(0);
        if std::env::var("STOOLS_DEBUG").is_ok() {
            eprintln!("[stools] search-rebuild={:?} n={}", st.elapsed(), ui.get_items().row_count());
        }
    });

    let exec_weak = weak.clone();
    ui.on_item_executed(move |index| {
        let Some(ui) = exec_weak.upgrade() else { return };
        if let Some(item) = ui.get_items().row_data(index as usize) {
            launch_exec(&item.exec.to_string());
        }
        let _ = slint::quit_event_loop();
    });

    ui.on_escape_pressed(|| {
        let _ = slint::quit_event_loop();
    });

    ui.show().unwrap();
    let t_show = t0.elapsed();
    if std::env::var("STOOLS_DEBUG").is_ok() {
        eprintln!(
            "[stools] load={:?} new={:?} model={:?} show={:?} apps={}",
            t_load,
            t_new,
            t_model,
            t_show,
            app_count
        );
    }
    slint::run_event_loop_until_quit().unwrap();
}
