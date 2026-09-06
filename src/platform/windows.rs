#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::core::config::Config;
use crate::core::history::HistoryManager;
use crate::core::keybind::{KeybindingMap, hotkey_code_name};
use crate::core::matcher::pinyin_fields;
use crate::core::model::{AppEntry, EntryKind};
use crate::core::search::{AppIndex, SearchBackend};
use crate::core::theme;
use crate::launcher::{LauncherWindow, sync_model_in_place};
use slint::{ComponentHandle, Model, VecModel};

/// Directories a launcher is expected to cover: both Start Menu roots (the
/// per-user one and the common one installers write to — including the shortcuts
/// that sit directly in the root) and both desktops.
fn app_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu"));
    }
    if let Some(programdata) = std::env::var_os("ProgramData") {
        dirs.push(PathBuf::from(programdata).join("Microsoft\\Windows\\Start Menu"));
    }

    // The user's desktop, the shared one (`C:\Users\Public\Desktop`), and the
    // OneDrive-redirected location — when Desktop is redirected elsewhere, the
    // icons live under OneDrive, not under USERPROFILE.
    if let Some(userprofile) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(userprofile);
        dirs.push(home.join("Desktop"));
        dirs.push(home.join("OneDrive").join("Desktop"));
    }
    if let Some(public) = std::env::var_os("PUBLIC") {
        dirs.push(PathBuf::from(public).join("Desktop"));
    }

    // Canonicalizing collapses two routes to the same folder (a redirected
    // Desktop junction, say) so its files are not indexed twice.
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    dirs.retain(|d| {
        let key = std::fs::canonicalize(d).unwrap_or_else(|_| d.clone());
        seen.insert(key)
    });

    dirs
}

/// Guard against junction loops (a folder linked back into itself).
const MAX_WALK_DEPTH: usize = 8;

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    walk_dir_inner(dir, out, 0);
}

fn walk_dir_inner(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        // Directories are descended into; everything else is collected. Note that
        // `is_file()` would drop symlinked executables, so only directories are
        // tested for — and they are depth-limited to survive junction cycles.
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => {
                if depth < MAX_WALK_DEPTH {
                    walk_dir_inner(&path, out, depth + 1);
                }
            }
            _ => out.push(path),
        }
    }
}

/// Extensions treated as launchable: shortcuts, binaries, scripts and `.url`
/// files (Steam drops one on the desktop per game, and they are not `.lnk`).
const APP_EXTENSIONS: &[&str] = &["lnk", "exe", "bat", "cmd", "com", "ps1", "msc", "url"];

/// Extensions that are kept in the displayed name: a bare `rufus` says nothing,
/// while `rufus.exe` makes it obvious that this is a portable binary rather than
/// a shortcut to an installed program.
const NAMED_EXTENSIONS: &[&str] = &["exe", "bat", "cmd", "com", "ps1", "msc"];

/// Display name for a scanned file. Shortcuts (`.lnk`, `.url`) drop their
/// extension — the shell never shows it either — but standalone binaries and
/// scripts keep it.
fn display_name(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let name = if NAMED_EXTENSIONS.contains(&ext.as_str()) {
        path.file_name()
    } else {
        path.file_stem()
    };
    name.and_then(|s| s.to_str()).map(str::to_string)
}

fn is_launchable_app(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| APP_EXTENSIONS.contains(&e.as_str()))
}

/// Scan the extra directories from the config file (non-recursive) for
/// executables, mirroring the Linux binary scan.
fn scan_binaries(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut entries = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            if !is_launchable_app(&path) {
                continue;
            }
            let Some(name) = display_name(&path) else {
                continue;
            };
            if name.is_empty() || name.starts_with('.') {
                continue;
            }
            if !seen.insert(path.to_string_lossy().to_lowercase()) {
                continue;
            }
            let (pinyin_full, pinyin_abbr, pinyin_indices) = pinyin_fields(&name);
            entries.push(AppEntry {
                id: format!("bin:{}", path.to_string_lossy()).into_boxed_str(),
                name: name.into_boxed_str(),
                exec: path.to_string_lossy().into_owned().into_boxed_str(),
                // Icons of the extra directories' binaries come from the .exe
                // itself, extracted through the shell.
                // Icons of the extra directories' binaries come from the .exe
                // itself, extracted through the shell — same as the desktop scan,
                // so no separate icon path is stored (it would just duplicate
                // `exec`).
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

/// Scan the Start Menu and both desktops for launchable files (`.lnk`, `.exe`,
/// `.bat`, `.cmd`, `.url`, …), plus the config file's extra directories, to build
/// the app list. Runs on a worker thread, so it may be repeated freely.
pub fn scan_apps(extra_dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut files = Vec::new();
    for dir in app_search_dirs() {
        walk_dir(&dir, &mut files);
    }
    files.sort();

    let mut entries = Vec::new();
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in files {
        if !is_launchable_app(&path) {
            continue;
        }
        let Some(name) = display_name(&path) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        // A shortcut frequently exists in both the personal and the common
        // directory: keying on the path keeps a single row per file.
        let path_str = path.to_string_lossy().into_owned();
        if !seen_paths.insert(path_str.to_lowercase()) {
            continue;
        }
        let (pinyin_full, pinyin_abbr, pinyin_indices) = pinyin_fields(&name);
        entries.push(AppEntry {
            id: path_str.as_str().into(),
            name: name.into_boxed_str(),
            // Kept as the shortcut's own path; ShellExecute (open crate) resolves
            // `.lnk`, `.url` and `.exe` alike.
            exec: path_str.as_str().into(),
            // No separate icon path: on Windows the icon is pulled out of `exec`
            // itself (embedded for `.exe`, resolved by the shell for `.lnk`/`.url`)
            // at render time — see `platform::windows_icon`. Dropping this field
            // avoids a third allocation of the (often long) path.
            icon_path: None,
            hidden: false,
            pinyin_full,
            pinyin_abbr,
            kind: EntryKind::Desktop,
            subtitle: None,
            pinyin_indices,
            is_alias: false,
        });
    }

    // Binaries rank below Start Menu shortcuts, exactly like on Linux.
    entries.extend(scan_binaries(extra_dirs));

    // Show the origin path only for names that appear more than once.
    let mut name_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for e in &entries {
        *name_counts.entry(e.name.to_lowercase()).or_insert(0) += 1;
    }
    for e in &mut entries {
        e.subtitle = (name_counts
            .get(&e.name.to_lowercase())
            .copied()
            .unwrap_or(0)
            > 1)
        // The row title already carries the file name, so only the folder is shown.
        .then(|| crate::core::path_utils::prettify_dir(Path::new(&*e.exec)).into());
    }

    entries
}

/// Focus + raise the launcher window and select all text in the search box.
fn show_and_focus(ui: &LauncherWindow) {
    let _ = ui.show();
    // Activate the window so it comes to the foreground and can receive input.
    let handle = ui.window().window_handle();
    match handle.window_handle() {
        Ok(wh) if matches!(wh.as_raw(), RawWindowHandle::Win32(_)) => {
            if let RawWindowHandle::Win32(w) = wh.as_raw() {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                        w.hwnd.get() as *mut core::ffi::c_void
                    );
                }
            }
        }
        _ => {}
    }
    // Select all + focus via the snippet's public function.
    ui.invoke_prepare_show();
}

fn hide_window(ui: &LauncherWindow) {
    let _ = ui.hide();

    // While the launcher sits hidden in the tray the index and UI are idle, so
    // hand the freed physical pages back to the OS. `SetProcessWorkingSetSize`
    // with both sizes set to (SIZE_T)-1 — `usize::MAX` — tells Windows to trim
    // the working set as far as it will allow: the task-manager footprint
    // collapses from tens of MB to a few, and the pages are faulted back in
    // within microseconds the next time the window is summoned. This runs on
    // every hide (Esc, launch, summon toggle).
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetProcessWorkingSetSize,
        };
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Keeps the index in step with the file system.
///
/// The scan runs on its own thread and is kicked off every time the launcher is
/// summoned, so an app dropped on the desktop or a shortcut added to the Start
/// Menu shows up on the next summon instead of after a daemon restart. The panel
/// itself is raised first and never waits: the list is only touched if the scan
/// actually found something different (see `AppIndex::replace_if_changed`).
#[derive(Clone)]
struct Rescanner {
    // `Arc` so the handle can be shared with the (Send-only) hotkey and menu
    // handlers; `Mutex` because a `Sender` is not `Sync` on its own.
    tx: Arc<Mutex<std::sync::mpsc::Sender<()>>>,
    // Directories to scan — swapped out when the config is reloaded.
    dirs: Arc<RwLock<Vec<PathBuf>>>,
}

impl Rescanner {
    fn spawn(index: AppIndex, initial_dirs: Vec<PathBuf>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let dirs = Arc::new(RwLock::new(initial_dirs));
        let dirs_w = dirs.clone();

        thread::spawn(move || {
            // Ends when the last `Rescanner` clone (and thus the sender) is dropped.
            while rx.recv().is_ok() {
                // Collapse a burst of summons into a single scan.
                while rx.try_recv().is_ok() {}
                let current = dirs_w.read().map(|d| d.clone()).unwrap_or_default();
                index.replace_if_changed(scan_apps(&current));
            }
        });
        Self {
            tx: Arc::new(Mutex::new(tx)),
            dirs,
        }
    }

    /// Ask for a rescan. Cheap and non-blocking: at most one scan is queued.
    fn request(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(());
        }
    }

    /// Point the scanner at a new set of directories and rescan immediately, so a
    /// config reload picks up added/removed `path` entries without a restart.
    fn set_dirs(&self, new_dirs: Vec<PathBuf>) {
        if let Ok(mut guard) = self.dirs.write() {
            *guard = new_dirs;
        }
        self.request();
    }
}

/// Raise the launcher, then quietly bring the index up to date.
fn show_launcher(ui: &LauncherWindow, rescanner: &Rescanner) {
    show_and_focus(ui);
    rescanner.request();
}

/// Translate a configured binding into a `global-hotkey` shortcut.
fn build_hotkey(mask: crate::core::keybind::ModifiersMask, key: &str) -> Option<HotKey> {
    let code = Code::from_str(&hotkey_code_name(key)?).ok()?;
    let mut mods = Modifiers::empty();
    if mask.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if mask.alt {
        mods |= Modifiers::ALT;
    }
    if mask.shift {
        mods |= Modifiers::SHIFT;
    }
    if mask.meta {
        mods |= Modifiers::SUPER;
    }
    Some(HotKey::new((!mods.is_empty()).then_some(mods), code))
}

/// The global summon hotkey, owned by the thread that registered it.
///
/// `GlobalHotKeyManager` holds a Win32 window handle, so it is neither `Send` nor
/// `Sync` and cannot be captured by the tray menu handler (which must be both).
/// Menu events are delivered on the UI thread — the same thread the manager is
/// created on — so a `thread_local` reaches it from the handler without any
/// unsynchronised sharing.
struct SummonHotKey {
    manager: GlobalHotKeyManager,
    registered: Option<HotKey>,
}

thread_local! {
    static SUMMON_HOTKEY: RefCell<Option<SummonHotKey>> = const { RefCell::new(None) };
}

impl SummonHotKey {
    /// Create the manager and register the initial binding.
    fn init(binding: Option<(crate::core::keybind::ModifiersMask, String)>) {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(err) => {
                eprintln!("[stools] failed to init hotkey manager: {err}");
                return;
            }
        };
        let mut state = Self {
            manager,
            registered: None,
        };
        match binding.and_then(|(mask, key)| build_hotkey(mask, &key)) {
            Some(hotkey) => state.register(hotkey),
            None => eprintln!("[stools] no global hotkey bound to the \"stools\" action"),
        }
        SUMMON_HOTKEY.set(Some(state));
    }

    /// Apply the binding from a freshly read config. An unchanged binding leaves
    /// the current registration alone, and releasing/succeeding failures are
    /// reported rather than silently swallowed.
    fn apply(hotkey: Option<HotKey>) {
        SUMMON_HOTKEY.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let Some(state) = borrow.as_mut() else {
                // Should not happen: menu events arrive on the UI thread, which is
                // where `init` ran. Say so rather than silently ignoring it.
                eprintln!("[stools] hotkey manager is not on this thread; binding not applied");
                return;
            };
            if state.registered == hotkey {
                return;
            }
            if let Some(old) = state.registered.take() {
                if let Err(err) = state.manager.unregister(old) {
                    eprintln!("[stools] failed to unregister global hotkey {old:?}: {err}");
                }
            }
            if let Some(hotkey) = hotkey {
                state.register(hotkey);
            }
        });
    }

    fn register(&mut self, hotkey: HotKey) {
        match self.manager.register(hotkey) {
            Ok(()) => self.registered = Some(hotkey),
            Err(err) => eprintln!("[stools] failed to register global hotkey {hotkey:?}: {err}"),
        }
    }

    /// Drop the manager, which destroys its hidden window and frees the key.
    fn shutdown() {
        SUMMON_HOTKEY.with(|cell| {
            cell.borrow_mut().take();
        });
    }
}

/// Re-read `config.toml` and apply everything that can change at runtime: the
/// theme (colours and font), the in-window keybindings, the global summon hotkey
/// and the scanned directories.
fn reload_config(
    ui: &slint::Weak<LauncherWindow>,
    rescanner: &Rescanner,
    keybindings: &Arc<RwLock<KeybindingMap>>,
    cli_dirs: &[String],
) {
    let config = Config::load_or_create();
    if let Some(ui) = ui.upgrade() {
        theme::apply_theme(&ui, &config.theme);
    }

    // Replace the in-window bindings, then read the summon binding back out of
    // the same map so both views of the config can never disagree.
    if let Ok(mut map) = keybindings.write() {
        *map = KeybindingMap::from_config(&config.keybindings);
    }
    let summon = keybindings.read().ok().and_then(|map| map.summon_binding());
    SummonHotKey::apply(summon.and_then(|(mask, key)| build_hotkey(mask, &key)));

    let mut extra_dirs = config.path.clone();
    extra_dirs.extend(cli_dirs.iter().cloned());
    rescanner.set_dirs(crate::core::path_utils::resolve_dirs(&extra_dirs));
}

/// Open the folder holding `config.toml` in Explorer (creating it if needed), so
/// the file can be edited without hunting for the AppData path.
fn open_config_folder() {
    let config_path = Config::config_path();
    let Some(folder) = config_path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(folder);
    let _ = open::that_detached(folder);
}

/// The Windows daemon: lives in the tray, registers a global hotkey and toggles
/// the launcher window between hidden and shown.
/// The tray badge: decode the PNG that `tools/build_icon.py` renders out of
/// `assets/icon.svg` and that is baked into the binary here.
///
/// Windows asks the shell for a single bitmap and scales it to whatever the
/// taskbar needs, so the asset is 64x64 — that divides cleanly to 32 (200% DPI)
/// and 16 (100% DPI) rather than resampling at a fractional ratio.
fn load_tray_icon() -> Option<tray_icon::Icon> {
    // Only a corrupt checked-in asset can fail here, and a missing badge is not
    // worth aborting startup over: fall through to tray-icon's default so the
    // launcher (and its hotkey) still comes up.
    let image = image::load_from_memory(include_bytes!("../../assets/icon.png")).ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

pub fn run() {
    let config = Config::load_or_create();
    // Before the first window exists: the renderer is a process-wide choice, so
    // a later change only lands when the tray process is restarted.
    config.apply_backend();
    // Kept so a config reload can rebuild the directory list the same way.
    let cli_dirs: Vec<String> = std::env::args().skip(1).collect();
    let mut extra_dirs = config.path.clone();
    extra_dirs.extend(cli_dirs.iter().cloned());
    // Resolved once and kept: the background rescan reuses the same directories.
    let scan_dirs = crate::core::path_utils::resolve_dirs(&extra_dirs);
    let apps = scan_apps(&scan_dirs);

    let ui = LauncherWindow::new().unwrap();
    let weak = ui.as_weak();

    theme::apply_theme(&ui, &config.theme);

    // Shared (not `Rc`) because the tray menu handler that replaces it on a
    // config reload has to be `Send + Sync`.
    let keybindings = Arc::new(RwLock::new(KeybindingMap::from_config(&config.keybindings)));
    let summon = keybindings.read().ok().and_then(|map| map.summon_binding());
    {
        let keybindings = keybindings.clone();
        ui.on_resolve_key(move |text, ctrl, alt, shift, meta| {
            keybindings
                .read()
                .ok()
                .and_then(|map| map.resolve_event(&text, ctrl, alt, shift, meta))
                .map(|action| action.as_str())
                .unwrap_or_default()
                .into()
        });
    }

    let image_cache = crate::launcher::AppImageCache::new();
    // Keep the (non-`Send`) icon cache on the UI thread so the search worker's
    // result closure can read it via `clone_on_ui_thread`.
    crate::launcher::AppImageCache::set_on_ui_thread(image_cache.clone());
    let history = std::rc::Rc::new(std::cell::RefCell::new(HistoryManager::load()));

    // Ranking happens on a worker thread so the caret never waits for the list.
    // Results are pushed back via `invoke_from_event_loop` (no main-thread polling).
    let apps = Arc::new(apps);
    let history_records = Arc::new(RwLock::new(history.borrow().records.clone()));
    let search = Arc::new(SearchBackend::new(
        apps.clone(),
        history_records.clone(),
        ui.as_weak(),
        image_cache.clone(),
    ));
    // Summoning the panel kicks off a background rescan, so apps added while the
    // daemon runs are picked up without a restart.
    let rescanner = Rescanner::spawn(search.index(), scan_dirs);

    // One persistent model is set once; every later result is merged in place so
    // the on-screen rows are reused instead of rebuilt (see `sync_model_in_place`).
    let items_model = Rc::new(VecModel::default());
    ui.set_items(items_model.clone().into());
    sync_model_in_place(&items_model, search.initial_items());

    // Live filtering while typing: submitted to the worker, results are pushed
    // back to the UI via `invoke_from_event_loop`.
    ui.on_search_changed({
        let search = search.clone();
        move |query| search.query(&query.to_string())
    });

    // Enter / click: launch the app and hide.
    ui.on_item_executed({
        let weak = weak.clone();
        move |index| {
            if let Some(ui) = weak.upgrade() {
                if let Some(item) = ui.get_items().row_data(index as usize) {
                    history.borrow_mut().record_hit(&item.id.to_string());
                    if let Ok(mut records) = history_records.write() {
                        *records = history.borrow().records.clone();
                    }
                    let _ = open::that_detached(item.exec.to_string());
                }
                hide_window(&ui);
            }
        }
    });

    // Esc: hide only (process keeps running in the tray).
    ui.on_escape_pressed({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                hide_window(&ui);
            }
        }
    });

    // Start hidden in the tray and immediately hand the freed pages back to the
    // OS, so the freshly spawned daemon already sits at its minimal footprint.
    hide_window(&ui);

    // ---- Global hotkey: whatever the config binds to "stools" ---------------
    // (default Alt+A; the window can still be summoned from the tray icon).
    SummonHotKey::init(summon);

    GlobalHotKeyEvent::set_event_handler(Some({
        let weak = weak.clone();
        let rescanner = rescanner.clone();
        move |event: global_hotkey::GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                if let Some(ui) = weak.upgrade() {
                    show_launcher(&ui, &rescanner);
                }
            }
        }
    }));

    // ---- Tray icon + menu ---------------------------------------------------
    let quitting = Arc::new(AtomicBool::new(false));

    // Tray labels follow the system locale: Chinese hosts get a Chinese menu,
    // everything else keeps the existing English labels.
    let is_zh = crate::core::i18n::is_chinese_locale();
    let (show_text, reload_text, folder_text, quit_text) = if is_zh {
        ("显示", "重载配置", "打开配置目录", "退出")
    } else {
        ("Show", "Reload config", "Show config folder", "Quit")
    };

    let menu = muda::Menu::new();
    let show_item = muda::MenuItem::with_id("show", show_text, true, None);
    let reload_item = muda::MenuItem::with_id("reload_config", reload_text, true, None);
    let folder_item =
        muda::MenuItem::with_id("show_config_folder", folder_text, true, None);
    let quit_item = muda::MenuItem::with_id("quit", quit_text, true, None);

    let _ = menu.append(&show_item);
    let _ = menu.append(&muda::PredefinedMenuItem::separator());
    let _ = menu.append(&reload_item);
    let _ = menu.append(&folder_item);
    let _ = menu.append(&muda::PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let mut tray_builder = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        // Without this the menu pops up on left click too, and the native popup
        // steals the focus from the launcher window we are trying to show.
        .with_menu_on_left_click(false)
        .with_tooltip("stools launcher");

    // A tray entry with no icon is just an empty slot in the taskbar.
    if let Some(icon) = load_tray_icon() {
        tray_builder = tray_builder.with_icon(icon);
    }

    let tray = tray_builder.build();

    {
        let weak = weak.clone();
        let quitting = quitting.clone();
        let rescanner = rescanner.clone();
        let keybindings = keybindings.clone();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            match event.id().0.as_str() {
                "show" => {
                    if let Some(ui) = weak.upgrade() {
                        show_launcher(&ui, &rescanner);
                    }
                }
                "reload_config" => {
                    reload_config(&weak, &rescanner, &keybindings, &cli_dirs);
                }
                "show_config_folder" => open_config_folder(),
                "quit" => {
                    quitting.store(true, Ordering::SeqCst);
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        }));
    }

    // Left-click the tray icon also summons the launcher.
    tray_icon::TrayIconEvent::set_event_handler(Some({
        let weak = weak.clone();
        let rescanner = rescanner.clone();
        move |event| {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(ui) = weak.upgrade() {
                    show_launcher(&ui, &rescanner);
                }
            }
        }
    }));

    // Run the main event loop. The window stays hidden until summoned.
    // `run_event_loop_until_quit` (rather than `run()`) keeps looping while the
    // window is hidden, so the tray/hotkey daemon stays alive. It returns only
    // when `quit_event_loop()` is called (tray -> Quit).
    slint::run_event_loop_until_quit().unwrap();

    // ---- Clean shutdown -----------------------------------------------------
    // Reset handlers so the loop genuinely ends, and drop tray/hotkey (their
    // Drop impls unregister). Kept references go out of scope here.
    GlobalHotKeyEvent::set_event_handler(None::<fn(_) -> _>);
    muda::MenuEvent::set_event_handler(None::<fn(_) -> _>);
    tray_icon::TrayIconEvent::set_event_handler(None::<fn(_) -> _>);
    drop(tray);
    SummonHotKey::shutdown();
    let _ = quitting;
}
