//! Shared by PR Review (the desktop app) and PR Bar (the menu bar app).
//!
//! Both apps use one data directory. PR Bar owns polling and notifications while it runs;
//! the files in [`sync`] are how the two keep each other up to date.

pub mod cleanup;
pub mod github;
pub mod grace;
pub mod ignore;
pub mod notify;
pub mod poll;
pub mod repos;
pub mod settings;
pub mod shell;
pub mod store;
pub mod sync;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `~/Library/Application Support/com.chriscole.prreview` (`%APPDATA%\…` on Windows): PR Review's
/// Tauri data dir, kept for both apps so settings, reports and notes carry over.
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("com.chriscole.prreview")
}

/// Percent-encode a URL query value.
pub fn url_encode(v: &str) -> String {
    v.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub fn url_decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(x) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_round_trip() {
        let k = "Attensi/creator-web#836";
        assert_eq!(super::url_decode(&super::url_encode(k)), k);
    }
}
