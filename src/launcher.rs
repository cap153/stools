use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use slint::{Image, ModelRc, SharedString, VecModel};

use crate::core::model::AppEntry;

slint::include_modules!();

/// Only this many items get their model entries built (and, transitively, their
/// icons loaded). The Slint window only shows ~10 rows, so 30 gives some scroll
/// headroom while keeping both startup and per-keystroke rebuilds cheap.
const MAX_VISIBLE_ITEMS: usize = 30;

/// Decoded icons live here for the whole session. Slint `Image` is a cheap
/// clone over already-decoded data, so rebuilding the model on every keystroke
/// reuses these instead of re-reading + re-decoding files from disk.
pub struct AppImageCache {
    map: RefCell<HashMap<PathBuf, Image>>,
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
        let img = Image::load_from_path(path).unwrap_or_else(|_| Image::default());
        self.map.borrow_mut().insert(path.to_path_buf(), img.clone());
        img
    }
}

pub fn to_ui_item(a: &AppEntry, cache: &AppImageCache) -> AppItem {
    let subtitle = a.subtitle.as_deref().unwrap_or("");
    AppItem {
        id: SharedString::from(a.id.as_str()),
        name: SharedString::from(a.name.as_str()),
        exec: SharedString::from(a.exec.as_str()),
        icon: match &a.icon_path {
            Some(p) => cache.get(Path::new(p)),
            None => Image::default(),
        },
        subtitle: subtitle.into(),
    }
}

/// Build a Slint model from a set of ranked indexes. Hidden entries are
/// filtered out first, then capped to the visible window, so a "no matches"
/// search yields an empty model (rather than falling back to the full list).
pub fn build_model(apps: &[AppEntry], idxs: &[usize], cache: &AppImageCache) -> ModelRc<AppItem> {
    let items: Vec<AppItem> = idxs
        .iter()
        .filter_map(|&i| apps.get(i))
        .filter(|a| !a.hidden)
        .take(MAX_VISIBLE_ITEMS)
        .map(|a| to_ui_item(a, cache))
        .collect();
    ModelRc::new(VecModel::from(items))
}
