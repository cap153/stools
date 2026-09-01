use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};

use crate::core::config::Config;
use crate::core::history::HistoryManager;
use crate::core::indexer;
use crate::core::keybind::KeybindingMap;
use crate::core::matcher;
use crate::core::model::{AppEntry, EntryKind};
use crate::core::path_utils::{self, normalize_dir};
use crate::core::search::SearchBackend;
use crate::core::theme;
use crate::launcher::LauncherWindow;

use slint::{ComponentHandle, Model};

// ---------------------------------------------------------------------------
// Desktop file directories
// ---------------------------------------------------------------------------

fn desktop_dirs(custom_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // Custom dirs passed on the command line come first -- they may hold
    // .desktop files the user added ad-hoc (e.g. a copy under their home).
    for cd in custom_dirs {
        dirs.push(cd.clone());
    }
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
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(normalize_dir(d.clone())));
    dirs.into_iter().map(normalize_dir).collect()
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
                continue;
            };
            let ft = entry.file_type();
            if ft.as_ref().map_or(true, |t| t.is_file()) {
                // found a file — record it if it's a valid image in a "known" location
                if is_icon_file(&name) {
                    let subdir_name = dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
                    if subdir_name == "apps"
                        || dir
                            .parent()
                            .is_some_and(|p| p.file_name().is_some_and(|s| s == "pixmaps"))
                    {
                        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
                        map.entry(stem.to_string()).or_insert_with(|| entry.path());
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
                continue;
            };
            if entry.file_type().as_ref().map_or(false, |t| t.is_file()) && is_icon_file(&name) {
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
                map.entry(stem.to_string()).or_insert_with(|| entry.path());
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

fn parse_desktop(path: &Path, _id: &str, icon_map: &HashMap<String, PathBuf>) -> Option<AppEntry> {
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
    let (pinyin_full, pinyin_abbr, pinyin_indices) = matcher::pinyin_fields(&name_str);

    Some(AppEntry {
        // id is the .desktop file's real path so a later same-name collision can
        // show where this entry came from (used as the subtitle origin).
        id: path.to_string_lossy().into_owned(),
        name: name_str,
        exec: exec_value,
        icon_path: icon.and_then(|i| resolve_icon(&i, icon_map)),
        hidden: hidden || no_display,
        pinyin_full,
        pinyin_abbr,
        kind: EntryKind::Desktop,
        subtitle: None,
        pinyin_indices,
    })
}

// ---------------------------------------------------------------------------
// Binary / executable scanning
// ---------------------------------------------------------------------------

/// Scan the given directories for executable files and turn them into
/// `AppEntry` records. Returns a Vec of (entry, absolute_path) so callers can
/// Scan the given directories for executable files and turn them into
/// `AppEntry` records. Entries are de-duplicated by *canonical path* so the
/// same physical file reached via a symlinked directory (e.g. /bin -> /usr/bin)
/// is only listed once, but genuinely distinct paths sharing a name are all kept
/// (the caller attaches a path subtitle to disambiguate them).
fn scan_binaries(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut entries = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            // Must be user/group/other executable
            if meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Skip hidden files and common non-launchable helpers
            if name.starts_with('.') {
                continue;
            }

            let (pinyin_full, pinyin_abbr, pinyin_indices) = matcher::pinyin_fields(name);
            let id = format!("bin:{}", path.to_string_lossy());
            // Canonicalize so symlinked directories (e.g. /bin -> /usr/bin) don't
            // yield the same physical file twice.
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let canon_str = canon.to_string_lossy().into_owned();
            if !seen_paths.insert(canon_str) {
                continue;
            }
            // subtitle is left empty here; the caller marks it only when an entry's
            // name collides with another entry from a different path.
            entries.push(AppEntry {
                id,
                name: name.to_string(),
                exec: path.to_string_lossy().into_owned(),
                icon_path: None,
                hidden: false,
                pinyin_full,
                pinyin_abbr,
                kind: EntryKind::Binary,
                subtitle: None,
                pinyin_indices,
            });
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// Full scan (icon map built once, then all desktop files resolved via it)
// ---------------------------------------------------------------------------

fn scan_apps(custom_dirs: &[PathBuf], binary_dirs: &[PathBuf]) -> Vec<AppEntry> {
    let icon_map = build_icon_map();

    let mut entries = Vec::new();
    // Keyed by "<dir>:<filename>" so a .desktop that exists in several dirs
    // (e.g. a copy in the user's home vs. the system one) is not silently
    // collapsed -- both show up and can be told apart.
    let mut seen_ids: HashSet<String> = HashSet::new();
    for dir in desktop_dirs(custom_dirs) {
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
            let unique_id = format!("{}:{}", dir.display(), id);
            if !seen_ids.insert(unique_id) {
                continue;
            }
            if let Some(entry) = parse_desktop(&path, &id, &icon_map) {
                entries.push(entry);
            }
        }
    }

    // Binaries go after desktop entries (lower priority). They are *not*
    // de-duplicated: if a command shares a name with a desktop entry or another
    // binary in a different directory, both are kept and distinguished by their
    // path subtitle below.
    entries.extend(scan_binaries(binary_dirs));

    // Mark the path subtitle on entries whose (case-insensitive) name also
    // belongs to another entry from a different path -- both desktop apps and
    // binaries show where they live so the duplicates can be told apart. Unique
    // names show nothing, keeping the list clean.
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for e in &entries {
        *name_counts.entry(e.name.to_lowercase()).or_insert(0) += 1;
    }
    for e in &mut entries {
        if name_counts
            .get(&e.name.to_lowercase())
            .copied()
            .unwrap_or(0)
            > 1
        {
            let origin = match e.kind {
                // Binaries: the executable path itself.
                EntryKind::Binary => Path::new(&e.exec),
                // Desktop apps: the .desktop file path (kept in `id`).
                EntryKind::Desktop => Path::new(&e.id),
            };
            e.subtitle = Some(path_utils::prettify_path(origin));
        } else {
            e.subtitle = None;
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// Cache with true background refresh
// ---------------------------------------------------------------------------

/// `custom_dirs` are the extra directories from the config file plus the command
/// line (both `.desktop` files and executables are picked up there);
/// `binary_dirs` are the merged executable-search paths. `force_fresh` (set when
/// directories were passed on the command line) scans synchronously so a
/// just-added `.desktop` shows up immediately instead of serving a stale cache.
pub fn load_apps(
    custom_dirs: &[PathBuf],
    binary_dirs: &[PathBuf],
    force_fresh: bool,
) -> Vec<AppEntry> {
    let fingerprint = indexer::dirs_fingerprint(&[custom_dirs, binary_dirs]);

    if force_fresh {
        let apps = scan_apps(custom_dirs, binary_dirs);
        indexer::save_cache(&apps, fingerprint);
        return apps;
    }

    if let Some(cached) = indexer::load_cache(fingerprint) {
        // Real background refresh — doesn't block the main thread.
        let custom = custom_dirs.to_vec();
        let dirs = binary_dirs.to_vec();
        std::thread::spawn(move || {
            let fresh = scan_apps(&custom, &dirs);
            if !fresh.is_empty() {
                indexer::save_cache(&fresh, fingerprint);
            }
        });
        return cached;
    }
    // Cold start (or the scanned directories changed): scan synchronously.
    let apps = scan_apps(custom_dirs, binary_dirs);
    indexer::save_cache(&apps, fingerprint);
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
    let config = Config::load_or_create();
    let cli_dirs: Vec<String> = env::args().skip(1).collect();

    // Config file paths come first, CLI arguments extend them.
    let mut extra_dirs = config.path.clone();
    extra_dirs.extend(cli_dirs.iter().cloned());
    let binary_dirs = path_utils::merge_binary_dirs(&extra_dirs);
    // Custom dirs are the user's explicit directories (config + CLI, expanded);
    // they are scanned for BOTH .desktop files and executables.
    let custom_dirs: Vec<PathBuf> = path_utils::resolve_dirs(&extra_dirs);
    let apps = load_apps(&custom_dirs, &binary_dirs, !cli_dirs.is_empty());
    let app_count = apps.len();
    let t_load = t0.elapsed();

    let ui = LauncherWindow::new().unwrap();
    let t_new = t0.elapsed();
    let weak = ui.as_weak();

    theme::apply_theme(&ui, &config.theme);

    let keybindings = KeybindingMap::from_config(&config.keybindings);
    ui.on_resolve_key(move |text, ctrl, alt, shift, meta| {
        keybindings
            .resolve_event(&text, ctrl, alt, shift, meta)
            .map(|action| action.as_str())
            .unwrap_or_default()
            .into()
    });

    let image_cache = crate::launcher::AppImageCache::new();
    // Keep the (non-`Send`) icon cache on the UI thread so the search worker's
    // result closure can read it via `clone_on_ui_thread`.
    crate::launcher::AppImageCache::set_on_ui_thread(image_cache.clone());
    let history = std::rc::Rc::new(std::cell::RefCell::new(HistoryManager::load()));

    // Ranking happens on a worker thread: the pressed character is painted
    // without waiting for the result list (see `core::search`). Results are
    // pushed back event-driven, so the main thread does no polling work.
    let apps = Arc::new(apps);
    let history_records = Arc::new(RwLock::new(history.borrow().records.clone()));
    let search = Arc::new(SearchBackend::new(
        apps.clone(),
        history_records.clone(),
        ui.as_weak(),
        image_cache.clone(),
    ));
    // First list is built synchronously (invoke_from_event_loop needs a running
    // event loop, which isn't up yet here).
    ui.set_items(search.initial_model());

    let t_model = t0.elapsed();
    if std::env::var("STOOLS_DEBUG").is_ok() {
        eprintln!("[stools] model={:?}", t_model);
    }

    ui.on_search_changed({
        let search = search.clone();
        move |query| search.query(&query.to_string())
    });

    let exec_weak = weak.clone();
    ui.on_item_executed(move |index| {
        let Some(ui) = exec_weak.upgrade() else {
            return;
        };
        if let Some(item) = ui.get_items().row_data(index as usize) {
            history.borrow_mut().record_hit(&item.id.to_string());
            // Publish the new counts to the ranking worker.
            if let Ok(mut records) = history_records.write() {
                *records = history.borrow().records.clone();
            }
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
            t_load, t_new, t_model, t_show, app_count
        );
    }
    slint::run_event_loop_until_quit().unwrap();
}
