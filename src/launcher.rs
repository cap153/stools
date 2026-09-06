use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

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
/// thread and reaches this cache through a `thread_local` rather than capturing
/// it (which would require it to be `Send`).
///
/// The state sits behind an `Rc` so every clone shares one map: a clone is how
/// the closure gets hold of it, and with a plain `HashMap` field each clone
/// would copy the map and then throw its additions away — the cache would never
/// actually cache anything. Sharing also means the bound below is enforced on
/// one real map instead of on short-lived copies.
pub struct AppImageCache {
    inner: Rc<RefCell<ImageCacheInner>>,
}

/// Icons the launcher keeps decoded, most recently used last.
struct ImageCacheInner {
    map: HashMap<PathBuf, Image>,
    /// Recency queue, oldest first, holding exactly the keys in `map`.
    order: VecDeque<PathBuf>,
}

/// How many decoded icons to keep. The window shows 16 rows, so this is ~8×
/// that: enough that scrolling through a long list never re-decodes, small
/// enough that a long-lived tray process cannot creep. Windows shell icons are
/// 32×32 (4 KB each) and Linux theme icons rarely exceed 128×128, so the whole
/// cache stays in the low megabytes even when completely full.
const MAX_CACHED_IMAGES: usize = 128;

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
    /// Shares the decoded icons rather than copying them: every clone points at
    /// the same (UI-thread-only) map, so icons loaded by one clone are seen by
    /// all of them.
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
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
            inner: Rc::new(RefCell::new(ImageCacheInner {
                map: HashMap::new(),
                order: VecDeque::new(),
            })),
        }
    }

    /// The decoded icon for `path`, loading (and caching) it on a miss.
    fn get(&self, path: &Path) -> Image {
        let mut inner = self.inner.borrow_mut();
        // Cloned first: `map.get` borrows `inner`, and the recency shuffle below
        // needs it mutably.
        if let Some(img) = inner.map.get(path).cloned() {
            // Mark it as most recently used so the icons on screen are the last
            // ones to go. The scan is over at most `MAX_CACHED_IMAGES` keys.
            if let Some(pos) = inner.order.iter().position(|k| k == path) {
                if let Some(key) = inner.order.remove(pos) {
                    inner.order.push_back(key);
                }
            }
            return img;
        }

        // On Windows the stored path is the shortcut/executable itself, whose icon
        // has to be pulled out of the shell rather than decoded from an image file.
        #[cfg(windows)]
        let img = crate::platform::windows_icon::extract_icon_from_path(path)
            .or_else(|| Image::load_from_path(path).ok())
            .unwrap_or_default();

        #[cfg(not(windows))]
        let img = Image::load_from_path(path).unwrap_or_default();

        inner.map.insert(path.to_path_buf(), img.clone());
        inner.order.push_back(path.to_path_buf());
        while inner.order.len() > MAX_CACHED_IMAGES {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            inner.map.remove(&oldest);
        }
        img
    }

    /// Number of cached icons (test helper: the bound is the point of the cache).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.borrow().map.len()
    }

    #[cfg(test)]
    fn contains(&self, path: &Path) -> bool {
        self.inner.borrow().map.contains_key(path)
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
        id: SharedString::from(&*a.id),
        name: SharedString::from(&*a.name),
        spans: ModelRc::new(VecModel::from(spans)),
        exec: SharedString::from(&*a.exec),
        // Prefer an explicit icon file; when none is stored — Windows shortcuts
        // (icon pulled from the `.lnk`/`.exe` itself) and Linux binaries — fall
        // back to `exec`, the target path. On Windows that yields the
        // shell/embedded icon; elsewhere it resolves to an empty image, which is
        // exactly the previous `None` behaviour.
        icon: {
            let icon_path = a
                .icon_path
                .as_deref()
                .map(Path::new)
                .unwrap_or_else(|| Path::new(&*a.exec));
            cache.get(icon_path)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys that no icon can live at. A failed load caches `Image::default()`
    /// just like a successful one, which is enough to exercise the bound.
    fn fake_icon_path(i: usize) -> PathBuf {
        PathBuf::from(format!("/nonexistent/stools-test-icon-{i}"))
    }

    #[test]
    fn icon_cache_never_exceeds_its_bound() {
        let cache = AppImageCache::new();
        for i in 0..(MAX_CACHED_IMAGES * 3) {
            cache.get(&fake_icon_path(i));
            assert!(
                cache.len() <= MAX_CACHED_IMAGES,
                "cache grew to {} at i={i}",
                cache.len()
            );
        }
        assert_eq!(cache.len(), MAX_CACHED_IMAGES);
    }

    #[test]
    fn icon_cache_evicts_the_least_recently_used() {
        let cache = AppImageCache::new();
        let oldest = fake_icon_path(0);
        let second_oldest = fake_icon_path(1);
        for i in 0..MAX_CACHED_IMAGES {
            cache.get(&fake_icon_path(i));
        }
        assert_eq!(cache.len(), MAX_CACHED_IMAGES);

        // Touching the oldest entry makes it the most recent, so the next
        // insertion has to evict the *second* oldest instead.
        cache.get(&oldest);
        cache.get(&fake_icon_path(MAX_CACHED_IMAGES));

        assert!(cache.contains(&oldest), "the entry just used was evicted");
        assert!(!cache.contains(&second_oldest), "the LRU entry survived");
        assert_eq!(cache.len(), MAX_CACHED_IMAGES);
    }

    #[test]
    fn icon_cache_clones_share_one_map() {
        // `clone_on_ui_thread` is how the search worker reaches the cache; if a
        // clone copied the map, every icon would be decoded afresh on every
        // keystroke and the bound would only ever apply to throwaway copies.
        let cache = AppImageCache::new();
        let clone = cache.clone();
        let path = fake_icon_path(0);
        cache.get(&path);

        assert_eq!(clone.len(), 1);
        assert!(clone.contains(&path));
    }
}
