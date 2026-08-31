#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use nucleo_matcher::{Config as MatcherConfig, Matcher};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::core::matcher::{pinyin_fields, rank};
use crate::core::history::HistoryManager;
use crate::core::model::{AppEntry, EntryKind};
use crate::launcher::{build_model, LauncherWindow};

use slint::{ComponentHandle, Model};

/// Directories that contain Start Menu shortcuts.
fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Some(programdata) = std::env::var_os("ProgramData") {
        dirs.push(PathBuf::from(programdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    dirs
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let ft = entry.file_type();
        if ft.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
            walk_dir(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Scan Start Menu `.lnk` files to build the app list.
pub fn scan_apps() -> Vec<AppEntry> {
    let mut files = Vec::new();
    for dir in start_menu_dirs() {
        walk_dir(&dir, &mut files);
    }
    files.sort();

    let mut entries = Vec::new();
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("lnk") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if stem.is_empty() {
            continue;
        }
        let (pinyin_full, pinyin_abbr) = pinyin_fields(&stem);
        entries.push(AppEntry {
            id: path.to_string_lossy().into_owned(),
            name: stem,
            // Kept as the .lnk path; ShellExecute (open crate) launches it.
            exec: path.to_string_lossy().into_owned(),
            icon_path: None,
            hidden: false,
            pinyin_full,
            pinyin_abbr,
            kind: EntryKind::Desktop,
            subtitle: None,
        });
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
                        w.hwnd.get() as *mut core::ffi::c_void,
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
}

/// The Windows daemon: lives in the tray, registers a global hotkey and toggles
/// the launcher window between hidden and shown.
pub fn run() {
    let apps = scan_apps();

    let ui = LauncherWindow::new().unwrap();
    let weak = ui.as_weak();

    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let mut scratch = crate::core::matcher::MatcherScratch::default();
    let image_cache = crate::launcher::AppImageCache::new();
    let history = std::rc::Rc::new(std::cell::RefCell::new(HistoryManager::load()));
    let initial_idxs: Vec<usize> = rank(
        &apps,
        "",
        &mut matcher,
        &mut scratch,
        Some(&history.borrow().records),
    );
    ui.set_items(build_model(&apps, &initial_idxs, &image_cache));

    // Live filtering while typing.
    let search_weak = weak.clone();
    {
        let history = history.clone();
        ui.on_search_changed(move |query| {
            let Some(ui) = search_weak.upgrade() else { return };
            let idxs = rank(
                &apps,
                &query,
                &mut matcher,
                &mut scratch,
                Some(&history.borrow().records),
            );
            ui.set_items(build_model(&apps, &idxs, &image_cache));
            ui.set_selected_index(0);
        });
    }

    // Enter / click: launch the app and hide.
    ui.on_item_executed({
        let weak = weak.clone();
        move |index| {
            if let Some(ui) = weak.upgrade() {
                if let Some(item) = ui.get_items().row_data(index as usize) {
                    history.borrow_mut().record_hit(&item.id.to_string());
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

    ui.hide().unwrap();

    // ---- Global hotkey: Alt+Space to summon ---------------------------------
    let hotkey_manager = GlobalHotKeyManager::new().expect("failed to init hotkey manager");
    let hotkey = HotKey::new(Some(Modifiers::ALT), Code::Space);
    hotkey_manager
        .register(hotkey)
        .expect("failed to register hotkey Alt+Space");

    GlobalHotKeyEvent::set_event_handler(Some({
        let weak = weak.clone();
        move |event: global_hotkey::GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                if let Some(ui) = weak.upgrade() {
                    show_and_focus(&ui);
                }
            }
        }
    }));

    // ---- Tray icon + menu ---------------------------------------------------
    let quitting = Arc::new(AtomicBool::new(false));

    let menu = muda::Menu::new();
    let show_item = muda::MenuItem::with_id("show", "Show", true, None);
    let quit_item = muda::MenuItem::with_id("quit", "Quit", true, None);
    let _ = menu.append(&show_item);
    let _ = menu.append(&quit_item);

    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("stools launcher")
        .build();

    {
        let weak = weak.clone();
        let quitting = quitting.clone();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            match event.id().0.as_str() {
                "show" => {
                    if let Some(ui) = weak.upgrade() {
                        show_and_focus(&ui);
                    }
                }
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
        move |event| {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(ui) = weak.upgrade() {
                    show_and_focus(&ui);
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
    drop(hotkey_manager);
    let _ = quitting;
}
