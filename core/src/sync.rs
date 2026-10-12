//! How PR Review and PR Bar talk: small files in the shared data dir, each app checking
//! them every couple of seconds. No sockets, no ports, nothing to fail when one isn't running.
//!
//! - `snapshot.json`: the latest PR list, written by whichever app polled GitHub.
//! - `bar.alive`: PR Bar's heartbeat. While it is fresh, PR Bar polls and notifies, and
//!   PR Review only reads the snapshot.
//! - `refresh.req`: "fetch now, please" (PR Review → PR Bar, e.g. after approving).
//! - `settings.json` and `ignored.json` are watched too: either app may change them.

use crate::github::Pr;
use crate::now_ms;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub const BAR_ALIVE: &str = "bar.alive";
pub const REFRESH: &str = "refresh.req";
pub const SNAPSHOT: &str = "snapshot.json";
pub const SETTINGS: &str = "settings.json";
pub const IGNORED: &str = "ignored.json";

/// PR Bar writes its heartbeat every 15s; allow a few misses (e.g. a slow wake from sleep).
const ALIVE_MS: u64 = 60_000;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    pub me: String,
    pub prs: Vec<Pr>,
    pub errors: Vec<String>,
    pub fetched_at: u64,
    pub loading: bool,
}

/// Write a timestamp, so the other app's watcher sees a change.
pub fn touch(data: &Path, name: &str) {
    let _ = std::fs::write(data.join(name), now_ms().to_string());
}

pub fn stamp(data: &Path, name: &str) -> u64 {
    std::fs::read_to_string(data.join(name)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Modification time in ms (0 if missing): for files written by other code paths.
pub fn mtime(data: &Path, name: &str) -> u64 {
    std::fs::metadata(data.join(name))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn bar_alive(data: &Path) -> bool {
    now_ms().saturating_sub(stamp(data, BAR_ALIVE)) < ALIVE_MS
}

/// Written to a temp file and renamed, so a reader never sees half a snapshot.
pub fn write_snapshot(data: &Path, s: &Snapshot) {
    let tmp = data.join(format!("{SNAPSHOT}.tmp"));
    if std::fs::write(&tmp, serde_json::to_string(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, data.join(SNAPSHOT));
    }
}

pub fn read_snapshot(data: &Path) -> Option<Snapshot> {
    std::fs::read_to_string(data.join(SNAPSHOT)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// Watches a set of files for changes since the last call.
pub struct Watch {
    seen: Vec<(String, u64)>,
}

impl Watch {
    pub fn new(data: &Path, names: &[&str]) -> Self {
        Watch { seen: names.iter().map(|n| (n.to_string(), mtime(data, n))).collect() }
    }

    /// Names whose modification time moved since the last call.
    pub fn changed(&mut self, data: &Path) -> Vec<String> {
        let mut out = vec![];
        for (name, last) in self.seen.iter_mut() {
            let now = mtime(data, name);
            if now != *last {
                *last = now;
                out.push(name.clone());
            }
        }
        out
    }
}
