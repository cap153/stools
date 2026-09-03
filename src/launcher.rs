use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use slint::{Image, Model, ModelRc, SharedString, VecModel};

use crate::core::matcher;
use crate::core::model::AppEntry;

slint::include_modules!();

/// Only this many items get their model entries built (and, transitively, their
/// icons loaded). The window shows ~14 rows, so 16 fills the screen with a little
/// scroll headroom while keeping both startup and per-keystroke rebuilds cheap.
const MAX_VISIBLE_ITEMS: usize = 16;

/// Decoded icons live here for the whole session. Slint `Image` is a cheap
/// clone over already-decoded data, so rebuilding the model on every keystroke
/// reuses these instead of re-reading + re-decoding files from disk.
///
/// `Image` is **not** `Send`, so the cache must stay on the UI thread. The
/// background search (`core::search`) computes results on a worker thread and
/// pushes them back with `invoke_from_event_loop`; the closure runs on the UI
/// thread and reads this cache from a `thread_local` rather than capturing it
/// (which would require it to be `Send`).
pub struct AppImageCache {
    map: RefCell<HashMap<PathBuf, Image>>,
}

thread_local! {
    static THREAD_CACHE: RefCell<Option<AppImageCache>> = const { RefCell::new(None) };
}

impl AppImageCache {
    /// Stash the cache on the UI thread so the search worker's result closure
    /// (which runs there) can load icons without moving a non-`Send` value.
    pub fn set_on_ui_thread(cache: AppImageCache) {
        THREAD_CACHE.with(|c| *c.borrow_mut() = Some(cache));
    }

    /// Clone the UI-thread cache (shares decoded icons) for use inside a result
    /// closure. Must be called from the UI thread.
    pub fn clone_on_ui_thread() -> AppImageCache {
        THREAD_CACHE
            .with(|c| c.borrow().clone())
            .expect("AppImageCache was not set on the UI thread")
    }
}

impl Clone for AppImageCache {
    /// `Image` is a cheap handle over already-decoded data, so cloning the cache
    /// shares the decoded icons instead of re-reading them.
    fn clone(&self) -> Self {
        Self {
            map: RefCell::new(self.map.borrow().clone()),
        }
    }
}

impl Default for AppImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl AppImageCache {
    pub fn new() -> Self {
        Self {
            map: RefCell::new(HashMap::new()),
        }
    }

    fn get(&self, path: &Path) -> Image {
        if let Some(img) = self.map.borrow().get(path) {
            return img.clone();
        }

        // On Windows the stored path is the shortcut/executable itself, whose icon
        // has to be pulled out of the shell rather than decoded from an image file.
        #[cfg(windows)]
        let img = crate::platform::windows_icon::extract_icon_from_path(path)
            .or_else(|| Image::load_from_path(path).ok())
            .unwrap_or_default();

        #[cfg(not(windows))]
        let img = Image::load_from_path(path).unwrap_or_default();

        self.map
            .borrow_mut()
            .insert(path.to_path_buf(), img.clone());
        img
    }
}

/// Build one row. `matched_indices` (character indices into the name) turn the
/// name into coloured spans so the hit characters can be highlighted — including
/// hits that came from pinyin, which are mapped back to their characters.
pub fn to_ui_item(a: &AppEntry, matched_indices: &[usize], cache: &AppImageCache) -> AppItem {
    let subtitle = a.subtitle.as_deref().unwrap_or("");
    let idle_subtitle = if subtitle.is_empty() {
        String::new()
    } else {
        // Head...tail form that fits the idle half-row; the full path is still used
        // for the marquee when the row is selected.
        crate::core::path_utils::abbreviate_path(subtitle, 26)
    };
    let spans = matcher::build_highlight_spans(&a.name, matched_indices)
        .into_iter()
        .map(|span| TextSpan {
            text: span.text.into(),
            is_match: span.is_match,
        })
        .collect::<Vec<_>>();

    AppItem {
        id: SharedString::from(a.id.as_str()),
        name: SharedString::from(a.name.as_str()),
        spans: ModelRc::new(VecModel::from(spans)),
        exec: SharedString::from(a.exec.as_str()),
        icon: match &a.icon_path {
            Some(p) => cache.get(Path::new(p)),
            None => Image::default(),
        },
        subtitle: subtitle.into(),
        idle_subtitle: idle_subtitle.into(),
    }
}

/// Build the row structs for the ranked indexes whose highlight positions have
/// already been computed (in parallel with typing, see `core::search`). Hidden
/// entries are filtered out first, then capped to the visible window, so a "no
/// matches" search yields an empty list (rather than falling back to the full list).
pub fn build_items_vec(
    apps: &[AppEntry],
    idxs: &[usize],
    highlights: &[Vec<usize>],
    cache: &AppImageCache,
) -> Vec<AppItem> {
    idxs.iter()
        .filter_map(|&i| apps.get(i))
        .filter(|a| !a.hidden)
        .take(MAX_VISIBLE_ITEMS)
        .enumerate()
        .map(|(row, a)| to_ui_item(a, highlights.get(row).map_or(&[] as &[usize], |h| h), cache))
        .collect()
}

/// Update a persistent `VecModel` in place. Rows that are already on screen are
/// reused — only their `row_data` changes — so Slint never destroys and rebuilds
/// the row/icon/text widgets, which is what made `set_items` repaints stutter.
pub fn sync_model_in_place(target: &VecModel<AppItem>, new_items: Vec<AppItem>) {
    let old_len = target.row_count();
    let new_len = new_items.len();
    let common = old_len.min(new_len);
    for i in 0..common {
        target.set_row_data(i, new_items[i].clone());
    }
    if old_len > new_len {
        for _ in new_len..old_len {
            target.remove(target.row_count() - 1);
        }
    } else if new_len > old_len {
        for item in new_items.into_iter().skip(old_len) {
            target.push(item);
        }
    }
}
