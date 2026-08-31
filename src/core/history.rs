use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

const MAX_HISTORY: usize = 200;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct HistoryRecord {
    pub last_used: u64,
    pub count: u32,
}

pub struct HistoryManager {
    file_path: PathBuf,
    pub records: HashMap<String, HistoryRecord>,
}

impl HistoryManager {
    pub fn load() -> Self {
        let dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("stools");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("history.json");

        let records = fs::read_to_string(&file_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        Self { file_path, records }
    }

    pub fn record_hit(&mut self, id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let entry = self.records.entry(id.to_string()).or_default();
        entry.last_used = now;
        entry.count = entry.count.saturating_add(1);

        // Evict oldest entries if over limit
        if self.records.len() > MAX_HISTORY {
            let mut entries: Vec<_> = self.records.iter().collect();
            entries.sort_by_key(|(_, h)| h.last_used);
            let to_remove = entries.len() - MAX_HISTORY;
            let keys_to_remove: Vec<String> = entries
                .iter()
                .take(to_remove)
                .map(|(k, _)| k.to_string())
                .collect();
            for k in keys_to_remove {
                self.records.remove(&k);
            }
        }

        self.save();
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.records) {
            let _ = fs::write(&self.file_path, json);
        }
    }
}
