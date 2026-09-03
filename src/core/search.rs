//! Background search so typing never waits for filtering.
//!
//! Slint runs `TextInput.edited` synchronously inside the event loop and only
//! paints after the callback returns. Doing the ranking there bundles "echo the
//! pressed character" and "rebuild the result list" into one frame, so the caret
//! lags behind the keyboard.
//!
//! The ranking is moved to a worker thread and results are pushed back with
//! [`slint::invoke_from_event_loop`], which is event-driven: the main thread does
//! zero work until a result is ready, so it is always free to paint the next
//! keystroke. A monotonically increasing `generation` is bumped on every keystroke
//! and the worker drops any query that has been superseded by a newer one, so fast
//! typing (`q` -> `qu` -> `qut` -> `qute`) only ever repaints the list once.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::core::history::HistoryRecord;
use crate::core::matcher::{self, MatcherScratch};
use crate::core::model::AppEntry;
use crate::launcher::{AppImageCache, AppItem, LauncherWindow, build_items_vec, sync_model_in_place};
use slint::{Model, VecModel};

/// Rows the UI actually draws; the window shows ~14, so 16 fills the screen with
/// a little scroll headroom while halving the per-keystroke model rebuild versus 30.
const HIGHLIGHT_ROWS: usize = 16;

type Records = Arc<RwLock<HashMap<String, HistoryRecord>>>;

/// Work for the ranking worker.
enum WorkerMsg {
    /// Rank `query`. `keep_selection` marks a refresh of the list the user is
    /// already looking at (the index changed behind their back), where jumping
    /// the selection back to the top would be a surprise.
    Query {
        generation: u64,
        query: String,
        keep_selection: bool,
    },
    Shutdown,
}

/// The live application index, shareable with other threads.
///
/// The list is handed out as an `Arc<Vec<AppEntry>>` snapshot, so a reader keeps a
/// consistent view even when a background rescan swaps the list underneath it.
/// That matters because ranked results are plain indices: they are only meaningful
/// together with the exact snapshot they were computed from.
#[derive(Clone)]
pub struct AppIndex {
    entries: Arc<RwLock<Arc<Vec<AppEntry>>>>,
    generation: Arc<AtomicU64>,
    /// The query currently on screen, replayed when the index changes.
    last_query: Arc<Mutex<String>>,
    tx: Option<Sender<WorkerMsg>>,
}

impl AppIndex {
    /// Current entries. Costs one `Arc` clone, never a copy of the list.
    pub fn snapshot(&self) -> Arc<Vec<AppEntry>> {
        self.entries
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Swap in a freshly scanned list. Returns `false` (and touches nothing) when
    /// it matches what is already indexed, so a rescan that found nothing new
    /// leaves the on-screen list — and the highlighted row — alone.
    ///
    /// Only the Windows build needs this: it stays resident in the tray for days,
    /// whereas the Linux build serves a single invocation and exits.
    #[cfg(windows)]
    pub fn replace_if_changed(&self, new: Vec<AppEntry>) -> bool {
        if fingerprint(&self.snapshot()) == fingerprint(&new) {
            return false;
        }
        if self.entries.write().map(|mut g| *g = Arc::new(new)).is_err() {
            return false;
        }
        self.refresh();
        true
    }

    /// Re-rank with whatever the user is currently searching for, so a newly
    /// indexed app appears without them retyping the query.
    #[cfg(windows)]
    pub fn refresh(&self) {
        let query = self.last_query.lock().map(|q| q.clone()).unwrap_or_default();
        self.submit(&query, true);
    }

    fn submit(&self, query: &str, keep_selection: bool) {
        if let Ok(mut last) = self.last_query.lock() {
            last.clear();
            last.push_str(query);
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(tx) = &self.tx {
            let _ = tx.send(WorkerMsg::Query {
                generation,
                query: query.to_string(),
                keep_selection,
            });
        }
    }
}

/// Identity of a whole list: enough to tell "a directory changed" from "nothing
/// moved" without cloning, sorting or comparing entry by entry.
#[cfg(windows)]
fn fingerprint(entries: &[AppEntry]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    entries.len().hash(&mut hasher);
    for e in entries {
        e.id.hash(&mut hasher);
        e.name.hash(&mut hasher);
        e.hidden.hash(&mut hasher);
    }
    hasher.finish()
}

/// Asynchronous search: `query()` submits, the result is pushed to the UI via
/// `invoke_from_event_loop` (no main-thread polling).
pub struct SearchBackend {
    index: AppIndex,
    history: Records,
    cache: AppImageCache,
    tx: Option<Sender<WorkerMsg>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SearchBackend {
    /// Spawn the worker. The app list lives in a shared index (see [`AppIndex`]) so
    /// it can be rescanned in the background at no copy to callers; `history` is
    /// shared and re-read per query. `ui` is held weakly so the worker never keeps
    /// the window alive on its own.
    pub fn new(
        apps: Arc<Vec<AppEntry>>,
        history: Records,
        ui: slint::Weak<LauncherWindow>,
        cache: AppImageCache,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_gen = generation.clone();

        let index = AppIndex {
            entries: Arc::new(RwLock::new(apps)),
            generation,
            last_query: Arc::new(Mutex::new(String::new())),
            tx: Some(tx.clone()),
        };
        let index_w = index.clone();
        let history_w = history.clone();
        let worker = thread::Builder::new()
            .name("stools-search".into())
            .spawn(move || {
                let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
                let mut scratch = MatcherScratch::default();

                'outer: while let Ok(msg) = rx.recv() {
                    let (mut gen_id, mut query, mut keep_selection) = match msg {
                        WorkerMsg::Shutdown => break,
                        WorkerMsg::Query {
                            generation,
                            query,
                            keep_selection,
                        } => (generation, query, keep_selection),
                    };

                    // Debounce: hold a beat, then drain any faster keystrokes into
                    // the latest one. A burst ("q" "u" "t" "e") therefore repaints
                    // the list only once, at the pause, instead of on every
                    // keystroke.
                    thread::sleep(std::time::Duration::from_millis(12));
                    loop {
                        match rx.try_recv() {
                            Ok(WorkerMsg::Shutdown) => break 'outer,
                            Ok(WorkerMsg::Query {
                                generation,
                                query: q,
                                keep_selection: k,
                            }) => {
                                gen_id = generation;
                                query = q;
                                keep_selection = k;
                            }
                            Err(_) => break,
                        }
                    }

                    // A newer keystroke landed while we were parked: drop this one.
                    if gen_id < worker_gen.load(Ordering::Relaxed) {
                        continue;
                    }

                    // One snapshot for the whole round: the indices we hand to the
                    // UI stay valid even if a rescan replaces the list right after.
                    let apps = index_w.snapshot();

                    let records = history_w.read().ok();
                    let history_ref = records.as_deref();
                    let idxs =
                        matcher::rank(&apps, &query, &mut matcher, &mut scratch, history_ref);
                    drop(records);

                    // Another keystroke arrived during the (sub-millisecond) compute:
                    // the result is already stale, never wake the main thread with it.
                    if gen_id < worker_gen.load(Ordering::Relaxed) {
                        continue;
                    }

                    let highlights: Vec<Vec<usize>> = idxs
                        .iter()
                        .filter(|&&i| !apps[i].hidden)
                        .take(HIGHLIGHT_ROWS)
                        .map(|&i| {
                            if query.trim().is_empty() {
                                Vec::new()
                            } else {
                                matcher::highlight_indices(
                                    &apps[i],
                                    &query,
                                    &mut matcher,
                                    &mut scratch,
                                )
                            }
                        })
                        .collect();

                    if gen_id < worker_gen.load(Ordering::Relaxed) {
                        continue;
                    }

                    // Only `Send` data crosses the thread boundary; the row structs
                    // (which hold non-`Send` `Image`s) are built on the UI thread, where
                    // the icon cache lives, and then merged into the persistent model.
                    let apps_ui = apps.clone();
                    let ui = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            // Remember the highlighted row so a refresh can put the
                            // cursor back on the same app instead of yanking the list
                            // out from under the user.
                            let selected = ui.get_selected_index();
                            let keep_id = if keep_selection && selected >= 0 {
                                ui.get_items()
                                    .row_data(selected as usize)
                                    .map(|row| row.id.to_string())
                            } else {
                                None
                            };

                            let cache = AppImageCache::clone_on_ui_thread();
                            let new_items =
                                build_items_vec(&apps_ui, &idxs, &highlights, &cache);
                            // Reuse the one model set at startup (which lives only on
                            // the UI thread) instead of swapping in a fresh one, so the
                            // on-screen rows are updated in place rather than rebuilt.
                            if let Some(model) =
                                ui.get_items().as_any().downcast_ref::<VecModel<AppItem>>()
                            {
                                sync_model_in_place(model, new_items);
                            }

                            let mut next = 0;
                            if let Some(id) = keep_id {
                                let items = ui.get_items();
                                for i in 0..items.row_count() {
                                    if items.row_data(i).is_some_and(|row| row.id == id) {
                                        next = i as i32;
                                        break;
                                    }
                                }
                            }
                            ui.set_selected_index(next);
                            if !keep_selection {
                                ui.invoke_scroll_to_top();
                            }
                        }
                    });
                }
            })
            .ok();

        Self {
            index,
            history,
            cache,
            tx: Some(tx),
            worker,
        }
    }

    /// Handle for background rescanning: the owner can swap the indexed list
    /// without touching (or even knowing about) the UI.
    #[cfg(windows)]
    pub fn index(&self) -> AppIndex {
        self.index.clone()
    }

    /// Initial list, computed synchronously on the caller's thread and merged
    /// into the persistent model (see `sync_model_in_place`). `invoke_from_event_loop`
    /// only works once the event loop is running, so the first render is built here.
    pub fn initial_items(&self) -> Vec<AppItem> {
        let apps = self.index.snapshot();
        let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let records = self.history.read().ok();
        let history_ref = records.as_deref();
        let idxs = matcher::rank(&apps, "", &mut matcher, &mut scratch, history_ref);
        drop(records);
        let highlights: Vec<Vec<usize>> = idxs
            .iter()
            .filter(|&&i| !apps[i].hidden)
            .take(HIGHLIGHT_ROWS)
            .map(|_| Vec::new())
            .collect();
        build_items_vec(&apps, &idxs, &highlights, &self.cache)
    }

    /// Submit a new query. Only enqueues the work (nanoseconds) and returns
    /// immediately, so the pressed character is painted without delay.
    pub fn query(&self, query: &str) {
        self.index.submit(query, false);
    }
}

impl Drop for SearchBackend {
    fn drop(&mut self) {
        // Ask the worker to stop explicitly: other `AppIndex` clones may still be
        // alive (a rescan thread, say), so closing the channel is not enough to
        // end the loop and `join()` would otherwise block forever.
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(WorkerMsg::Shutdown);
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
