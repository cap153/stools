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
use std::sync::{Arc, RwLock};
use std::thread;

use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::core::history::HistoryRecord;
use crate::core::matcher::{self, MatcherScratch};
use crate::core::model::AppEntry;
use crate::launcher::{AppImageCache, AppItem, LauncherWindow, build_model_from};
use slint::ModelRc;

/// Rows the UI actually draws; the window shows ~14, so 16 fills the screen with
/// a little scroll headroom while halving the per-keystroke model rebuild versus 30.
const HIGHLIGHT_ROWS: usize = 16;

type Records = Arc<RwLock<HashMap<String, HistoryRecord>>>;

/// Asynchronous search: `query()` submits, the result is pushed to the UI via
/// `invoke_from_event_loop` (no main-thread polling).
pub struct SearchBackend {
    tx: Option<Sender<(u64, String)>>,
    generation: Arc<AtomicU64>,
    // Kept so the very first list can be built synchronously (see `initial_model`).
    apps: Arc<Vec<AppEntry>>,
    history: Records,
    cache: AppImageCache,
    worker: Option<thread::JoinHandle<()>>,
}

impl SearchBackend {
    /// Spawn the worker. The app list is shared (immutable after the scan) so it
    /// costs no copy; `history` is shared and re-read per query. `ui` is held
    /// weakly so the worker never keeps the window alive on its own.
    pub fn new(
        apps: Arc<Vec<AppEntry>>,
        history: Records,
        ui: slint::Weak<LauncherWindow>,
        cache: AppImageCache,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<(u64, String)>();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_gen = generation.clone();

        let apps_w = apps.clone();
        let history_w = history.clone();
        let worker = thread::Builder::new()
            .name("stools-search".into())
            .spawn(move || {
                let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
                let mut scratch = MatcherScratch::default();

                while let Ok((mut gen_id, mut query)) = rx.recv() {
                    // Debounce: hold a beat, then drain any faster keystrokes into
                    // the latest one. A burst ("q" "u" "t" "e") therefore repaints the
                    // list only once, at the pause, instead of on every keystroke.
                    thread::sleep(std::time::Duration::from_millis(12));
                    while let Ok((g, q)) = rx.try_recv() {
                        gen_id = g;
                        query = q;
                    }

                    // A newer keystroke landed while we were parked: drop this one.
                    if gen_id < worker_gen.load(Ordering::Relaxed) {
                        continue;
                    }

                    let records = history_w.read().ok();
                    let history_ref = records.as_deref();
                    let idxs =
                        matcher::rank(&apps_w, &query, &mut matcher, &mut scratch, history_ref);
                    drop(records);

                    // Another keystroke arrived during the (sub-millisecond) compute:
                    // the result is already stale, never wake the main thread with it.
                    if gen_id < worker_gen.load(Ordering::Relaxed) {
                        continue;
                    }

                    let highlights: Vec<Vec<usize>> = idxs
                        .iter()
                        .filter(|&&i| !apps_w[i].hidden)
                        .take(HIGHLIGHT_ROWS)
                        .map(|&i| {
                            if query.trim().is_empty() {
                                Vec::new()
                            } else {
                                matcher::highlight_indices(
                                    &apps_w[i],
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

                    // Only `Send` data crosses the thread boundary; the model (which
                    // holds non-`Send` `Image`s) is built on the UI thread, where the
                    // icon cache lives.
                    let apps_w2 = apps_w.clone();
                    let ui = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let cache = AppImageCache::clone_on_ui_thread();
                        let model = build_model_from(&apps_w2, &idxs, &highlights, &cache);
                        if let Some(ui) = ui.upgrade() {
                            ui.set_items(model);
                            ui.set_selected_index(0);
                            ui.invoke_scroll_to_top();
                        }
                    });
                }
            })
            .ok();

        Self {
            tx: Some(tx),
            generation,
            apps,
            history,
            cache,
            worker,
        }
    }

    /// Initial list, computed synchronously on the caller's thread.
    /// `invoke_from_event_loop` only works once the event loop is running, so the
    /// very first render is built here (the one-time cost at startup is fine).
    pub fn initial_model(&self) -> ModelRc<AppItem> {
        let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
        let mut scratch = MatcherScratch::default();
        let records = self.history.read().ok();
        let history_ref = records.as_deref();
        let idxs = matcher::rank(&self.apps, "", &mut matcher, &mut scratch, history_ref);
        drop(records);
        let highlights: Vec<Vec<usize>> = idxs
            .iter()
            .filter(|&&i| !self.apps[i].hidden)
            .take(HIGHLIGHT_ROWS)
            .map(|_| Vec::new())
            .collect();
        build_model_from(&self.apps, &idxs, &highlights, &self.cache)
    }

    /// Submit a new query. Only enqueues the work (nanoseconds) and returns
    /// immediately, so the pressed character is painted without delay.
    pub fn query(&self, query: &str) {
        let gen_id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(tx) = &self.tx {
            let _ = tx.send((gen_id, query.to_string()));
        }
    }
}

impl Drop for SearchBackend {
    fn drop(&mut self) {
        // Drop the sender first so the worker's `recv()` returns Err and the loop
        // exits; otherwise `join()` would block forever on a parked worker.
        self.tx.take();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
