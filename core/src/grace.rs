//! PRs I just approved stay in "Review requested" for a while, so I can still merge them.
//! Approving removes me from `review-requested:@me`, which would otherwise drop the PR at once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn path(data: &Path) -> PathBuf {
    data.join("approved.json")
}

fn load(data: &Path) -> HashMap<String, u64> {
    std::fs::read_to_string(path(data)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn record(data: &Path, key: &str) {
    let _g = LOCK.lock().unwrap();
    let mut m = load(data);
    m.insert(key.to_string(), crate::now_ms());
    let _ = std::fs::write(path(data), serde_json::to_string_pretty(&m).unwrap_or_default());
}

/// Keys still inside the grace window, pruning the rest.
pub fn active(data: &Path, minutes: u64) -> Vec<String> {
    let _g = LOCK.lock().unwrap();
    let now = crate::now_ms();
    let window = minutes * 60_000;
    let mut m = load(data);
    let before = m.len();
    m.retain(|_, at| now.saturating_sub(*at) < window);
    if m.len() != before {
        let _ = std::fs::write(path(data), serde_json::to_string_pretty(&m).unwrap_or_default());
    }
    m.into_keys().collect()
}
