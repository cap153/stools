use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
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
use crate::launcher::{LauncherWindow, sync_model_in_place};
use slint::VecModel;

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

fn parse_desktop(path: &Path, icon_map: &HashMap<String, PathBuf>) -> Vec<AppEntry> {
    let Ok(content) = fs::read_to_string(path) else { return Vec::new(); };
    let mut default_name = None::<String>;
    let mut zh_name = None::<String>;
    let mut exec = None::<String>;
    let mut icon = None::<String>;
    let mut hidden = false;
    let mut no_display = false;
    let mut is_application = false;
    let mut in_desktop = false;

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
            "Name" => default_name = Some(value.to_string()),
            // Any Chinese locale name: Simplified (zh_CN / zh) and Traditional
            // (zh_TW / zh_HK / …) — vendors sometimes ship only a subset.
            k if k.starts_with("Name[zh") && k.ends_with(']') => {
                zh_name = Some(value.to_string())
            }
            "Exec" => exec = Some(value.to_string()),
            "Icon" => icon = Some(value.to_string()),
            "Hidden" => hidden = parse_bool(Some(value), false),
            "NoDisplay" => no_display = parse_bool(Some(value), false),
            _ => {}
        }
    }

    if !is_application {
        return Vec::new();
    }
    let Some(exec_value) = exec else { return Vec::new(); };
    let Some(def_name) = default_name else { return Vec::new(); };

    // Pick the primary name from the current locale, and keep the other language
    // (when it differs) as a searchable alias. That way an English query hits the
    // English name and a Chinese query hits the Chinese name, each highlighted in
    // its own script. Aliases are filtered out of the empty-query list (see
    // `matcher::rank`) so the first screen is not cluttered with twins.
    let is_zh = crate::core::i18n::is_chinese_locale();
    // Only generate an alias when the Chinese and English names genuinely differ —
    // regardless of the current locale. Without the `zh != def_name` guard here a
    // non-Chinese system with `Name=Firefox` / `Name[zh_CN]=Firefox` would emit two
    // identical entries.
    let (primary_name, secondary_name) = match &zh_name {
        Some(zh) if zh != &def_name => {
            if is_zh {
                (zh.clone(), Some(def_name))
            } else {
                (def_name, Some(zh.clone()))
            }
        }
        _ => (zh_name.unwrap_or(def_name), None),
    };

    let resolved_icon = icon.and_then(|i| resolve_icon(&i, icon_map));
    let main_id = path.to_string_lossy().into_owned();
    let mut result = Vec::with_capacity(2);

    // 1. Primary entry (is_alias = false).
    let (pf, pa, pi) = matcher::pinyin_fields(&primary_name);
    result.push(AppEntry {
        // id is the .desktop file's real path so a later same-name collision can
        // show where this entry came from (used as the subtitle origin).
        id: main_id.as_str().into(),
        name: primary_name.into(),
        exec: exec_value.as_str().into(),
        icon_path: resolved_icon.as_deref().map(Into::into),
        hidden: hidden || no_display,
        pinyin_full: pf,
        pinyin_abbr: pa,
        kind: EntryKind::Desktop,
        subtitle: None,
        pinyin_indices: pi,
        is_alias: false,
    });

    // 2. Secondary-language alias entry (is_alias = true) when the two names differ.
    if let Some(sec_name) = secondary_name {
        let (pf, pa, pi) = matcher::pinyin_fields(&sec_name);
        result.push(AppEntry {
            id: format!("{}:alias", main_id).into(),
            name: sec_name.into(),
            exec: exec_value.into(),
            icon_path: resolved_icon.map(Into::into),
            hidden: hidden || no_display,
            pinyin_full: pf,
            pinyin_abbr: pa,
            kind: EntryKind::Desktop,
            subtitle: None,
            pinyin_indices: pi,
            is_alias: true,
        });
    }

    result
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
                id: id.into_boxed_str(),
                name: name.into(),
                exec: path.to_string_lossy().into_owned().into_boxed_str(),
                icon_path: None,
                hidden: false,
                pinyin_full,
                pinyin_abbr,
                kind: EntryKind::Binary,
                subtitle: None,
                pinyin_indices,
                is_alias: false,
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
            entries.extend(parse_desktop(&path, &icon_map));
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
                EntryKind::Binary => Path::new(&*e.exec),
                // Desktop apps: the .desktop file path (kept in `id`).
                EntryKind::Desktop => Path::new(&*e.id),
            };
            // The row title already carries the file name, so only the folder is
            // shown — that is what tells two same-named entries apart.
            e.subtitle = Some(path_utils::prettify_dir(origin).into());
        } else {
            e.subtitle = None;
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// Cache with true background refresh
// ---------------------------------------------------------------------------

/// Give the scan's scratch memory back to the kernel.
///
/// Reading and parsing a few thousand `.desktop` files allocates (and frees) a
/// lot of short-lived strings, and `build_icon_map` builds a large map that is
/// dropped again before `scan_apps` returns. glibc keeps freed pages in its own
/// arena rather than returning them, so without this the process sits on RSS it
/// will never touch again — for a launcher that lives for a few hundred
/// milliseconds, that would be most of its footprint. `malloc_trim(0)` releases
/// whatever the allocator can spare; the launcher then settles at its true
/// working set.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_memory() {
    // glibc extension. Only touches the allocator's own free lists, so the
    // argument (0 = "as much as you can") is the whole contract.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Nothing to trim outside glibc: musl returns memory eagerly enough on its own
/// and does not expose `malloc_trim`.
#[cfg(all(target_os = "linux", not(target_env = "gnu")))]
fn trim_memory() {}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// `LC_ALL` is process-global, and cargo runs tests in parallel threads
    /// inside a single process, so the tests that poke it would race and each
    /// observe the other's locale. The lock serialises just those two.
    static LOCALE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take `LOCALE_LOCK`, tolerating poisoning so one failing assertion cannot
    /// wedge every later test that needs the locale.
    fn lock_locale() -> std::sync::MutexGuard<'static, ()> {
        LOCALE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_desktop(dir: &std::path::Path, name: &str, zh: Option<&str>) -> std::path::PathBuf {
        let p = dir.join("sample.desktop");
        let mut c = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec=echo hi\nIcon=firefox\n"
        );
        if let Some(zh) = zh {
            c.push_str(&format!("Name[zh_CN]={zh}\n"));
        }
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(c.as_bytes()).unwrap();
        p
    }

    #[test]
    fn zh_desktop_yields_english_alias_under_chinese_locale() {
        let _locale = lock_locale();
        let dir = std::env::temp_dir().join("stools_test_alias_zh");
        let _ = std::fs::create_dir_all(&dir);
        let p = write_desktop(&dir, "Power Off", Some("关机"));

        let old = std::env::var("LC_ALL").ok();
        unsafe {
            std::env::set_var("LC_ALL", "zh_CN.UTF-8");
        }
        let entries = parse_desktop(&p, &HashMap::new());
        if let Some(o) = &old {
            unsafe {
                std::env::set_var("LC_ALL", o);
            }
        } else {
            unsafe {
                std::env::remove_var("LC_ALL");
            }
        }

        assert_eq!(entries.len(), 2);
        assert_eq!(&*entries[0].name, "关机");
        assert!(!entries[0].is_alias);
        assert_eq!(&*entries[1].name, "Power Off");
        assert!(entries[1].is_alias);
    }

    #[test]
    fn alias_omitted_when_names_are_identical() {
        let dir = std::env::temp_dir().join("stools_test_alias_same");
        let _ = std::fs::create_dir_all(&dir);
        let p = write_desktop(&dir, "Firefox", Some("Firefox"));
        let entries = parse_desktop(&p, &HashMap::new());
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_alias);
    }

    #[test]
    fn identical_names_dont_duplicate_on_non_chinese_locale() {
        // Regression: a non-Chinese session must not emit two identical entries
        // just because Name and Name[zh_CN] share the same string.
        let _locale = lock_locale();
        let dir = std::env::temp_dir().join("stools_test_alias_nzh");
        let _ = std::fs::create_dir_all(&dir);
        let p = write_desktop(&dir, "Firefox", Some("Firefox"));

        let old = std::env::var("LC_ALL").ok();
        unsafe {
            std::env::set_var("LC_ALL", "en_US.UTF-8");
        }
        let entries = parse_desktop(&p, &HashMap::new());
        if let Some(o) = &old {
            unsafe {
                std::env::set_var("LC_ALL", o);
            }
        } else {
            unsafe {
                std::env::remove_var("LC_ALL");
            }
        }

        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_alias);
        assert_eq!(&*entries[0].name, "Firefox");
    }
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
    // The scan just churned through thousands of short-lived strings, and the
    // index is all that is left of them. Hand their pages back before the
    // window goes up, so the process settles at its real working set.
    trim_memory();
    let t_trim = t0.elapsed();

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
    // One persistent model is set once; every later result is merged in place so
    // the on-screen rows are reused instead of rebuilt (see `sync_model_in_place`).
    let items_model = Rc::new(VecModel::default());
    ui.set_items(items_model.clone().into());
    sync_model_in_place(&items_model, search.initial_items());

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
            "[stools] load={:?} trim={:?} new={:?} model={:?} show={:?} apps={}",
            t_load, t_trim, t_new, t_model, t_show, app_count
        );
    }
    slint::run_event_loop_until_quit().unwrap();
}
