use slint::{Image, ModelRc, SharedString, VecModel};

use crate::core::model::AppEntry;

slint::include_modules!();

/// Convert an indexed entry into its Slint display representation.
pub fn to_ui_item(a: &AppEntry) -> AppItem {
    AppItem {
        id: SharedString::from(a.id.as_str()),
        name: SharedString::from(a.name.as_str()),
        exec: SharedString::from(a.exec.as_str()),
        icon: match &a.icon_path {
            Some(p) => Image::load_from_path(std::path::Path::new(p)).unwrap_or_default(),
            None => Image::default(),
        },
    }
}

/// Build a Slint model from either the full app list or a set of ranked indexes.
/// Hidden entries are always filtered out for display.
pub fn build_model(apps: &[AppEntry], idxs: &[usize]) -> ModelRc<AppItem> {
    let items: Vec<AppItem> = if idxs.is_empty() {
        apps.iter()
            .filter(|a| !a.hidden)
            .map(to_ui_item)
            .collect()
    } else {
        idxs.iter()
            .filter_map(|&i| apps.get(i))
            .filter(|a| !a.hidden)
            .map(to_ui_item)
            .collect()
    };
    ModelRc::new(VecModel::from(items))
}
