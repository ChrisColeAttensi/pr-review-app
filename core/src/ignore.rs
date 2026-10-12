//! PRs the user chose not to see (e.g. team review requests that aren't theirs).
//! An ignore is dropped once the PR hasn't shown up for a while, so the list tidies itself.

use crate::github::Pr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FORGET_AFTER_MS: u64 = 14 * 86_400_000;
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ignored {
    pub title: String,
    pub ignored_at: u64,
    pub last_seen: u64,
}

fn path(data: &Path) -> PathBuf {
    data.join("ignored.json")
}

fn load(data: &Path) -> HashMap<String, Ignored> {
    std::fs::read_to_string(path(data)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(data: &Path, m: &HashMap<String, Ignored>) {
    let _ = std::fs::write(path(data), serde_json::to_string_pretty(m).unwrap_or_default());
}

/// Flag ignored PRs in a fresh fetch, and forget ignores for PRs long gone.
/// The file is only rewritten when that changes something worth keeping: both apps watch it.
pub fn apply(data: &Path, prs: &mut [Pr]) {
    let _g = LOCK.lock().unwrap();
    let now = crate::now_ms();
    let mut m = load(data);
    let mut dirty = false;
    for p in prs.iter_mut() {
        if let Some(e) = m.get_mut(&p.key) {
            p.ignored = true;
            // A day's precision is plenty for a two-week expiry.
            if now.saturating_sub(e.last_seen) > 86_400_000 {
                e.last_seen = now;
                dirty = true;
            }
        }
    }
    let before = m.len();
    m.retain(|_, e| now.saturating_sub(e.last_seen) < FORGET_AFTER_MS);
    if dirty || m.len() != before {
        save(data, &m);
    }
}

/// Flag ignored PRs without touching the file (after another app changed it).
pub fn flag(data: &Path, prs: &mut [Pr]) {
    let m = load(data);
    for p in prs.iter_mut() {
        p.ignored = m.contains_key(&p.key);
    }
}

pub fn set(data: &Path, key: &str, title: &str, ignored: bool) {
    let _g = LOCK.lock().unwrap();
    let now = crate::now_ms();
    let mut m = load(data);
    if ignored {
        m.insert(key.to_string(), Ignored { title: title.to_string(), ignored_at: now, last_seen: now });
    } else {
        m.remove(key);
    }
    save(data, &m);
}
