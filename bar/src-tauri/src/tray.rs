//! The menu-bar dropdown: a scannable list of what needs you, most urgent first.

use crate::BarState;
use pr_core::github::{self, Pr};
use pr_core::settings::Settings;
use pr_core::now_ms;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
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
    let now = (now_ms() / 1000) as i64;
    let s = (now - t).max(0);
    match s {
        0..=3599 => format!("{}m", (s / 60).max(1)),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// Minute-level ages tick constantly; letting them alone trigger a rebuild would close the
/// menu under the pointer every minute. Hours and days still do.
fn coarse(age: &str) -> &str {
    if age.ends_with('m') { "" } else { age }
}

fn repo_ref(p: &Pr) -> String {
    format!("{}#{}", p.repo.split('/').last().unwrap_or(""), p.number)
}

/// A review request's GitHub state as (icon, sort rank). The icon is the whole status: the
/// section heading already says it's waiting on you.
/// Rank: waiting on you first (oldest first), then ones you approved, then just merged/closed.
fn request_status(p: &Pr) -> (&'static str, u8) {
    if let Some(st) = closed(p) {
        return st;
    }
    if p.just_approved {
        // GitHub's mark for an approval.
        return ("✅", 5);
    }
    if github::ci_failed(p) {
        return ("🔴", 0);
    }
    ("○", 0)
}

/// Merged or closed since it was last open: shown last, then dropped after the grace period.
fn closed(p: &Pr) -> Option<(&'static str, u8)> {
    match p.closed_state.as_deref()? {
        "MERGED" => Some(("🟣", 9)),
        _ => Some(("⚫", 9)),
    }
}

/// My PR's GitHub state as (icon, sort rank). ✅ only when it can be merged right now.
fn my_status(p: &Pr) -> (&'static str, u8) {
    if let Some(st) = closed(p) {
        return st;
    }
    if github::ready_to_merge(p) {
        return ("✅", 0);
    }
    if github::approval(p) == "CHANGES_REQUESTED" {
        return ("❌", 1);
    }
    if github::ci_failed(p) {
        return ("🔴", 2);
    }
    if github::has_conflicts(p) {
        return ("⚠️", 2);
    }
    // Awaiting review, CI still running, branch behind, or blocked by a rule.
    ("○", 3)
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
    let state = app.state::<BarState>();
    let s = state.settings.lock().unwrap().clone();
    let (prs, fetched_at) = {
        let snap = state.snapshot.lock().unwrap();
        (snap.prs.clone(), snap.fetched_at)
    };
    let apps = *state.apps.lock().unwrap();

    let mut requested: Vec<&Pr> = prs.iter().filter(|p| visible(p, &s) && has(p, "reviewRequested")).collect();
    let mut mine: Vec<&Pr> = prs.iter().filter(|p| visible(p, &s) && has(p, "mine")).collect();
    // Requests: waiting on you first, longest waiting at the top.
    requested.sort_by(|a, b| request_status(a).1.cmp(&request_status(b).1).then(a.created_at.cmp(&b.created_at)));
    mine.sort_by_key(|p| my_status(p).1);

    // The badge is the open review-request count, matching the first section's heading.
    let open = |v: &Vec<&Pr>| v.iter().filter(|p| p.closed_state.is_none()).count();
    let n = open(&requested);
    let _ = tray.set_title(if n > 0 { Some(n.to_string()) } else { None::<String> });
    let mine_open = open(&mine);
    let ready = mine.iter().filter(|p| my_status(p).1 == 0).count();
    let updated = if fetched_at == 0 {
        "checking GitHub…".to_string()
    } else {
        let mins = (now_ms().saturating_sub(fetched_at)) / 60_000;
        if mins == 0 { "updated just now".into() } else { format!("updated {mins} min ago") }
    };
    let _ = tray.set_tooltip(Some(format!(
        "PR Bar — {} review request{}, {} of your PRs ready to merge · {updated}",
        n,
        if n == 1 { "" } else { "s" },
        ready
    )));

    let build = || -> tauri::Result<Menu<Wry>> {
        let item = |id: String, text: String, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
        let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = vec![];

        // Each PR is a submenu: a menu bar menu can't take right-clicks on its rows.
        let pr_menu = |p: &Pr, text: String| -> tauri::Result<Submenu<Wry>> {
            let k = &p.key;
            let mut actions: Vec<Box<dyn IsMenuItem<Wry>>> = vec![];
            if apps.desktop {
                actions.push(Box::new(item(format!("open:{k}"), "Open in PR Review".into(), true)?));
            }
            actions.push(Box::new(item(format!("gh:{k}"), "Open on GitHub".into(), true)?));
            actions.push(Box::new(item(format!("copy:{k}"), "Copy review command".into(), true)?));
            actions.push(Box::new(PredefinedMenuItem::separator(app)?));
            actions.push(Box::new(item(format!("ignore:{k}"), "Ignore".into(), true)?));
            let refs: Vec<&dyn IsMenuItem<Wry>> = actions.iter().map(|b| b.as_ref()).collect();
            Submenu::with_id_and_items(app, format!("pr:{k}"), text, true, &refs)
        };

        // `count` is open PRs only (it matches the badge); `total` includes just-merged rows.
        let mut section = |title: &str, rows: Vec<Submenu<Wry>>, empty: &str, count: usize, total: usize| -> tauri::Result<()> {
            items.push(Box::new(item(format!("h:{title}"), format!("{}  ·  {count}", title.to_uppercase()), false)?));
            if rows.is_empty() {
                items.push(Box::new(item(format!("e:{title}"), format!("      {empty}"), false)?));
            }
            for row in rows {
                items.push(Box::new(row));
            }
            if total > MAX_ROWS {
                let more = if apps.desktop { "open" } else { "noop" };
                items.push(Box::new(item(more.into(), format!("      …and {} more", total - MAX_ROWS), apps.desktop)?));
            }
            items.push(Box::new(PredefinedMenuItem::separator(app)?));
            Ok(())
        };

        let mut req_rows = vec![];
        for p in requested.iter().take(MAX_ROWS) {
            let (icon, _) = request_status(p);
            let detail = [repo_ref(p), age(&p.created_at)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            req_rows.push(pr_menu(p, row(icon, &p.title, &detail))?);
        }
        section("Review requested", req_rows, "Nothing waiting on you 🎉", n, requested.len())?;

        let mut my_rows = vec![];
        for p in mine.iter().take(MAX_ROWS) {
            let (icon, _) = my_status(p);
            my_rows.push(pr_menu(p, row(icon, &p.title, &repo_ref(p)))?);
        }
        let empty = if s.actionable_only { "Nothing to act on" } else { "No open PRs" };
        section("My PRs", my_rows, empty, mine_open, mine.len())?;

        if apps.desktop {
            items.push(Box::new(item("open".into(), "Open PR Review".into(), true)?));
        }
        items.push(Box::new(MenuItem::with_id(app, "refresh", "Refresh", true, Some("CmdOrCtrl+R"))?));
        items.push(Box::new(MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?));
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        items.push(Box::new(MenuItem::with_id(app, "quit", "Quit PR Bar", true, Some("CmdOrCtrl+Q"))?));
        let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|b| b.as_ref()).collect();
        Menu::with_items(app, &refs)
    };
    // Replacing the menu closes it if it's open, so only do it when the contents changed.
    let sig = format!(
        "{:?}|{:?}|{:?}|{}|{}",
        requested.iter().map(|p| (&p.key, &p.title, request_status(p), repo_ref(p), coarse(&age(&p.created_at)).to_string(), p.closed_state.is_some())).collect::<Vec<_>>(),
        mine.iter().map(|p| (&p.key, &p.title, my_status(p), p.closed_state.is_some())).collect::<Vec<_>>(),
        (apps.desktop, s.actionable_only),
        n,
        mine_open,
    );
    let mut last = state.menu_sig.lock().unwrap();
    if *last == sig {
        return;
    }
    if let Ok(menu) = build() {
        let _ = tray.set_menu(Some(menu));
        *last = sig;
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
