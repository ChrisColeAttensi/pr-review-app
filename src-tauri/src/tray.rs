//! The menu-bar dropdown: a scannable list of what needs you, most urgent first.

use crate::github::{self, Pr};
use crate::settings::Settings;
use crate::{store, AppState};
use std::collections::HashMap;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Manager, Wry};

const MAX_ROWS: usize = 10;
const TITLE_CHARS: usize = 44;

pub fn visible(p: &Pr, s: &Settings) -> bool {
    !p.ignored
        && !(p.team_only && s.direct_requests_only)
        && (s.show_drafts || !p.is_draft)
        && (!s.actionable_only || p.needs_action || p.closed_state.is_some())
}

fn has(p: &Pr, c: &str) -> bool {
    p.categories.iter().any(|x| x == c)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n { s.chars().take(n - 1).collect::<String>() + "…" } else { s.to_string() }
}

/// Unix seconds from an ISO-8601 UTC timestamp like `2026-09-30T08:22:15Z`.
fn parse_iso(ts: &str) -> Option<i64> {
    let (date, time) = ts.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim_end_matches('Z').split(':').map(|x| x.split('.').next().unwrap_or("0").parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next().flatten().unwrap_or(0));
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn age(ts: &str) -> String {
    let Some(t) = parse_iso(ts) else { return String::new() };
    let now = (crate::jobs::now_ms() / 1000) as i64;
    let s = (now - t).max(0);
    match s {
        0..=3599 => format!("{}m", (s / 60).max(1)),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

fn repo_ref(p: &Pr) -> String {
    format!("{}#{}", p.repo.split('/').last().unwrap_or(""), p.number)
}

/// Where a requested review stands on this machine, as (icon, words, sort rank).
/// Rank: findings to act on first, then reviewed, then in progress, then untouched.
fn request_status(p: &Pr, e: Option<&store::IndexEntry>, running: bool) -> (&'static str, String, u8) {
    if let Some(st) = closed(p) {
        return st;
    }
    if running {
        return ("⏳", "Claude working".into(), 2);
    }
    if p.just_approved {
        return ("👍", "you approved · merge?".into(), 1);
    }
    if let Some(e) = e {
        if let Some(v) = &e.verdict {
            let stale = e.review_head.as_deref() != Some(p.head_sha.as_str());
            let suffix = if stale { " · new commits" } else { "" };
            if v.to_lowercase().contains("change before merge") {
                // "🛠️ Change before merge · 2" → 2 findings
                let n = v.rsplit('·').next().and_then(|x| x.trim().parse::<u32>().ok());
                let words = n.map(|n| format!("{n} to fix")).unwrap_or_else(|| "issues found".into());
                return ("🛠", format!("{words}{suffix}"), 0);
            }
            if v.to_lowercase().contains("decision") {
                return ("🤔", format!("needs a decision{suffix}"), 0);
            }
            return ("✅", format!("reviewed{suffix}"), 1);
        }
        if e.insights_head.is_some() {
            return ("✦", "explained".into(), 3);
        }
    }
    ("○", "not started".into(), 4)
}

/// Merged or closed since it was last open: shown last, then dropped after the grace period.
fn closed(p: &Pr) -> Option<(&'static str, String, u8)> {
    match p.closed_state.as_deref()? {
        "MERGED" => Some(("🟣", "merged".into(), 9)),
        _ => Some(("⚫", "closed".into(), 9)),
    }
}

/// My PR's next step, as (icon, words, sort rank).
fn my_status(p: &Pr) -> (&'static str, &'static str, u8) {
    match p.closed_state.as_deref() {
        Some("MERGED") => return ("🟣", "merged", 9),
        Some(_) => return ("⚫", "closed", 9),
        None => {}
    }
    let ci = github::ci_failed(p);
    match github::approval(p) {
        "APPROVED" if !ci => ("🟢", "ready to merge", 0),
        "CHANGES_REQUESTED" => ("🟠", "changes requested", 1),
        "APPROVED" => ("🔴", "approved, CI failing", 2),
        _ if ci => ("🔴", "CI failing", 2),
        _ => ("⚪", "awaiting review", 3),
    }
}

fn row(icon: &str, title: &str, detail: &str) -> String {
    format!("{icon}  {}   —   {detail}", truncate(title, TITLE_CHARS))
}

/// Rebuild from current state. Safe to call from any thread.
pub fn refresh(app: &AppHandle) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || rebuild(&app2));
}

fn rebuild(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main") else { return };
    let state = app.state::<AppState>();
    let s = state.settings.lock().unwrap().clone();
    let (prs, fetched_at) = {
        let snap = state.snapshot.lock().unwrap();
        (snap.prs.clone(), snap.fetched_at)
    };
    let index: HashMap<String, store::IndexEntry> = store::index(&state.data_dir);
    let running: Vec<String> = state
        .jobs
        .lock()
        .map(|m| m.values().filter(|j| j.info.status == "running").map(|j| j.info.pr_key.clone()).collect())
        .unwrap_or_default();

    let mut requested: Vec<&Pr> = prs.iter().filter(|p| visible(p, &s) && has(p, "reviewRequested")).collect();
    let mut mine: Vec<&Pr> = prs.iter().filter(|p| visible(p, &s) && has(p, "mine")).collect();
    // Requests: what Claude already flagged first, then longest waiting.
    requested.sort_by(|a, b| {
        let ra = request_status(a, index.get(&a.key), running.contains(&a.key)).2;
        let rb = request_status(b, index.get(&b.key), running.contains(&b.key)).2;
        ra.cmp(&rb).then(a.created_at.cmp(&b.created_at))
    });
    mine.sort_by_key(|p| my_status(p).2);

    // The badge is the review-request count, matching the first section's heading.
    let open = |v: &Vec<&Pr>| v.iter().filter(|p| p.closed_state.is_none()).count();
    let n = open(&requested);
    let _ = tray.set_title(if n > 0 { Some(n.to_string()) } else { None::<String> });
    let mine_open = open(&mine);
    let ready = mine.iter().filter(|p| my_status(p).2 == 0).count();
    let _ = tray.set_tooltip(Some(format!(
        "PR Review — {} review request{}, {} of your PRs ready to merge",
        n,
        if n == 1 { "" } else { "s" },
        ready
    )));

    let build = || -> tauri::Result<Menu<Wry>> {
        let item = |id: String, text: String, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
        let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = vec![];

        // `count` is open PRs only (it matches the badge); `total` includes just-merged rows.
        let mut section = |title: &str, rows: Vec<(String, String)>, empty: &str, count: usize, total: usize| -> tauri::Result<()> {
            items.push(Box::new(item(format!("h:{title}"), format!("{}  ·  {count}", title.to_uppercase()), false)?));
            if rows.is_empty() {
                items.push(Box::new(item(format!("e:{title}"), format!("      {empty}"), false)?));
            }
            for (key, text) in rows {
                items.push(Box::new(item(format!("pr:{key}"), text, true)?));
            }
            if total > MAX_ROWS {
                items.push(Box::new(item("open".into(), format!("      …and {} more", total - MAX_ROWS), true)?));
            }
            items.push(Box::new(PredefinedMenuItem::separator(app)?));
            Ok(())
        };

        let req_rows = requested
            .iter()
            .take(MAX_ROWS)
            .map(|p| {
                let (icon, words, _) = request_status(p, index.get(&p.key), running.contains(&p.key));
                (p.key.clone(), row(icon, &p.title, &format!("{} · {} · {words}", repo_ref(p), age(&p.created_at))))
            })
            .collect();
        section("Review requested", req_rows, "Nothing waiting on you 🎉", n, requested.len())?;

        let my_rows = mine
            .iter()
            .take(MAX_ROWS)
            .map(|p| {
                let (icon, words, _) = my_status(p);
                (p.key.clone(), row(icon, &p.title, &format!("{} · {words}", repo_ref(p))))
            })
            .collect();
        let empty = if s.actionable_only { "Nothing to act on" } else { "No open PRs" };
        section("My PRs", my_rows, empty, mine_open, mine.len())?;

        let updated = if fetched_at == 0 {
            "Checking GitHub…".to_string()
        } else {
            let mins = (crate::jobs::now_ms().saturating_sub(fetched_at)) / 60_000;
            if mins == 0 { "Updated just now".into() } else { format!("Updated {mins} min ago") }
        };
        items.push(Box::new(item("open".into(), "Open PR Review".into(), true)?));
        items.push(Box::new(MenuItem::with_id(app, "refresh", format!("Refresh  ({updated})"), true, Some("Cmd+R"))?));
        items.push(Box::new(MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?));
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        items.push(Box::new(MenuItem::with_id(app, "quit", "Quit PR Review", true, Some("Cmd+Q"))?));
        let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|b| b.as_ref()).collect();
        Menu::with_items(app, &refs)
    };
    if let Ok(menu) = build() {
        let _ = tray.set_menu(Some(menu));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_timestamps() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-09-30T08:22:15Z"), Some(1_790_756_535));
        assert_eq!(parse_iso("nonsense"), None);
    }
}
