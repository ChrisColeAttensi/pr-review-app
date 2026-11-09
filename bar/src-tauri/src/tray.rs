//! The menu-bar dropdown: a scannable list of what needs you, most urgent first.

use crate::BarState;
use pr_core::github::{self, Pr};
use pr_core::settings::Settings;
use pr_core::now_ms;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{IconMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use std::collections::HashMap;
use tauri::{AppHandle, Manager, Wry};

const MAX_ROWS: usize = 10;
const TITLE_CHARS: usize = 48;

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

/// Minute-level ages tick constantly; letting them trigger a rebuild would close the menu
/// under the pointer every minute. Under an hour reads "new"; hours and days still tick.
fn coarse(age: String) -> String {
    if age.ends_with('m') { "new".into() } else { age }
}

fn repo_ref(p: &Pr) -> String {
    format!("{}#{}", p.repo.split('/').last().unwrap_or(""), p.number)
}

/// A PR's state for the menu: a coloured dot, a plain-words label, and a sort rank.
/// The dot colours mean the same thing in both sections:
/// 🟢 good to go · 🔴 broken / blocked on someone · 🟠 conflicts · 🟡 waiting on a person ·
/// 🔵 CI running · ⚪ draft · 🟣 merged · ⚫ closed.
pub struct Status {
    pub dot: &'static str,
    pub label: &'static str,
    pub rank: u8,
}

const fn st(dot: &'static str, label: &'static str, rank: u8) -> Status {
    Status { dot, label, rank }
}

/// Merged or closed since it was last open: shown last, then dropped after the grace period.
fn closed(p: &Pr) -> Option<Status> {
    match p.closed_state.as_deref()? {
        "MERGED" => Some(st("🟣", "Merged", 9)),
        _ => Some(st("⚫", "Closed", 9)),
    }
}

fn ci_running(p: &Pr) -> bool {
    matches!(p.ci.as_deref(), Some("PENDING" | "EXPECTED"))
}

/// Someone else's PR that asks for my review. Rank: needs me first, then ones I approved,
/// then just merged/closed.
pub fn request_status(p: &Pr) -> Status {
    if let Some(s) = closed(p) {
        return s;
    }
    if p.just_approved {
        return st("🟢", "You approved", 5);
    }
    if p.is_draft {
        return st("⚪", "Draft", 4);
    }
    if github::ci_failed(p) {
        return st("🔴", "CI failing", 1);
    }
    if p.needs_re_review {
        return st("🟡", "Re-review", 0);
    }
    st("🟡", "Needs your review", 0)
}

/// My PR. 🟢 only when it can be merged right now.
pub fn my_status(p: &Pr) -> Status {
    if let Some(s) = closed(p) {
        return s;
    }
    if p.is_draft {
        return st("⚪", "Draft", 6);
    }
    if github::ready_to_merge(p) {
        return st("🟢", "Ready to merge", 0);
    }
    if github::approval(p) == "CHANGES_REQUESTED" {
        return st("🔴", "Changes requested", 1);
    }
    if github::ci_failed(p) {
        return st("🔴", "CI failing", 1);
    }
    if github::has_conflicts(p) {
        return st("🟠", "Conflicts", 2);
    }
    if ci_running(p) {
        return st("🔵", "CI running", 3);
    }
    if p.merge_state.as_deref() == Some("BEHIND") {
        return st("🟡", "Behind base", 3);
    }
    if github::approval(p) == "APPROVED" {
        // Approved and green but GitHub still blocks it (required check or rule).
        return st("🟡", "Approved · blocked", 3);
    }
    st("🟡", "Awaiting review", 4)
}

/// One flat row: `🟡  Title   —   Needs your review · web#12 · 2d`. `author` adds the
/// login to the detail, for rows that have no avatar to show who it's from.
fn row(s: &Status, p: &Pr, author: bool, age_text: &str) -> String {
    let mut detail: Vec<String> = vec![];
    if author && !p.author.is_empty() {
        detail.push(p.author.clone());
    }
    detail.push(s.label.into());
    detail.push(repo_ref(p));
    if !age_text.is_empty() {
        detail.push(age_text.into());
    }
    format!("{}  {}   —   {}", s.dot, truncate(&p.title, TITLE_CHARS), detail.join(" · "))
}

struct Row {
    id: String,
    text: String,
    icon: Option<Image<'static>>,
}

// ---------- refresh feedback ----------

/// Spinner drawn as the tray title while a refresh is running. Quarter circles, because
/// braille frames are too small to read at menu bar size.
const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
/// Long enough that a fast fetch still reads as "it did something".
const MIN_SPIN: Duration = Duration::from_millis(900);
/// How long the ✓ / ! stays after a refresh finishes.
const RESULT_FLASH: Duration = Duration::from_millis(1400);

/// Ask the poller for a fetch now, and show it working: spinner in the menu bar,
/// "Refreshing…" in the menu, then a brief ✓ (or ! on failure) before the count returns.
pub fn start_refresh(app: &AppHandle) {
    let state = app.state::<BarState>();
    state.refresh_requested.fetch_add(1, Ordering::SeqCst);
    crate::wake(app);
    // Already animating: that loop will keep going until this request is done too.
    if state.spinning.swap(true, Ordering::SeqCst) {
        return;
    }
    refresh(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<BarState>();
        let started = Instant::now();
        let mut i = 0usize;
        loop {
            let done = state.refresh_done.load(Ordering::SeqCst) >= state.refresh_requested.load(Ordering::SeqCst);
            if done && started.elapsed() >= MIN_SPIN {
                break;
            }
            let frame = SPIN[i % SPIN.len()].to_string();
            let a = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(t) = a.tray_by_id("main") {
                    let _ = t.set_title(Some(frame));
                }
            });
            i += 1;
            std::thread::sleep(Duration::from_millis(120));
        }
        let ok = state.snapshot.lock().unwrap().errors.is_empty();
        let mark = if ok { "✓" } else { "!" }.to_string();
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(t) = a.tray_by_id("main") {
                let _ = t.set_title(Some(mark));
            }
        });
        std::thread::sleep(RESULT_FLASH);
        state.spinning.store(false, Ordering::SeqCst);
        refresh(&app);
    });
}

fn refreshing(state: &BarState) -> bool {
    state.spinning.load(Ordering::SeqCst)
}

/// Local wall-clock time of the last fetch, e.g. `14:05`. Fixed per fetch, so unlike
/// "n min ago" it never forces a menu rebuild (which would close an open menu).
fn clock(ms: u64) -> String {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("date").arg("-r").arg((ms / 1000).to_string()).arg("+%H:%M").output();
        if let Ok(o) = out {
            let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }
    let _ = ms;
    String::new()
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
    let (prs, fetched_at, error) = {
        let snap = state.snapshot.lock().unwrap();
        (snap.prs.clone(), snap.fetched_at, snap.errors.first().cloned())
    };
    let apps = *state.apps.lock().unwrap();
    let busy = refreshing(&state) || fetched_at == 0;

    let mut requested: Vec<&Pr> = prs.iter().filter(|p| visible(p, &s) && has(p, "reviewRequested")).collect();
    let mut mine: Vec<&Pr> = prs.iter().filter(|p| visible(p, &s) && has(p, "mine")).collect();
    // Requests: needs you first, longest waiting at the top.
    requested.sort_by(|a, b| request_status(a).rank.cmp(&request_status(b).rank).then(a.created_at.cmp(&b.created_at)));
    mine.sort_by_key(|p| my_status(p).rank);

    // The badge is the open review-request count, matching the first section's heading.
    let open = |v: &Vec<&Pr>| v.iter().filter(|p| p.closed_state.is_none()).count();
    let n = open(&requested);
    // While refreshing, the spinner thread owns the title.
    if !refreshing(&state) {
        let _ = tray.set_title(if n > 0 { Some(n.to_string()) } else { None::<String> });
    }
    let mine_open = open(&mine);
    let ready = mine.iter().filter(|p| my_status(p).rank == 0).count();
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

    let footer = if busy {
        "Refreshing…".to_string()
    } else if let Some(e) = &error {
        format!("⚠️  Couldn't reach GitHub: {}", truncate(e, 48))
    } else {
        match clock(fetched_at) {
            t if t.is_empty() => "Up to date".into(),
            t => format!("Updated at {t}"),
        }
    };

    // Clicking a row opens it: in PR Review when installed, else on GitHub.
    let open_id = |p: &Pr| if apps.desktop { format!("open:{}", p.key) } else { format!("gh:{}", p.key) };

    // Review requests show the author's avatar; the name moves to the hover tooltip.
    // Until an avatar is cached the name stays in the row, so it's never anonymous.
    let avatar_for = |login: &str| -> Option<Image<'static>> {
        let mut cache = state.avatars.lock().unwrap();
        cache.entry(login.to_string()).or_insert_with(|| crate::avatar::load(&state.data_dir, login)).clone()
    };
    let mut tips: HashMap<String, String> = HashMap::new();
    let req_rows: Vec<Row> = requested
        .iter()
        .take(MAX_ROWS)
        .map(|p| {
            let icon = avatar_for(&p.author);
            let text = row(&request_status(p), p, icon.is_none(), &coarse(age(&p.created_at)));
            tips.insert(text.clone(), format!("From {}\n{}", p.author, p.title));
            Row { id: open_id(p), text, icon }
        })
        .collect();
    let my_rows: Vec<Row> = mine
        .iter()
        .take(MAX_ROWS)
        .map(|p| {
            let text = row(&my_status(p), p, false, "");
            tips.insert(text.clone(), p.title.clone());
            Row { id: open_id(p), text, icon: None }
        })
        .collect();
    *state.tooltips.lock().unwrap() = tips;

    let build = || -> tauri::Result<Menu<Wry>> {
        let item = |id: String, text: String, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
        let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = vec![];

        // `count` is open PRs only (it matches the badge); `total` includes just-merged rows.
        let mut section = |title: &str, rows: &[Row], empty: &str, count: usize, total: usize| -> tauri::Result<()> {
            items.push(Box::new(item(format!("h:{title}"), format!("{title}  ({count})"), false)?));
            if rows.is_empty() {
                items.push(Box::new(item(format!("e:{title}"), format!("      {empty}"), false)?));
            }
            for r in rows {
                match &r.icon {
                    Some(icon) => items.push(Box::new(IconMenuItem::with_id(app, r.id.clone(), r.text.clone(), true, Some(icon.clone()), None::<&str>)?)),
                    None => items.push(Box::new(item(r.id.clone(), r.text.clone(), true)?)),
                }
            }
            if total > MAX_ROWS {
                let more = if apps.desktop { "open" } else { "noop" };
                items.push(Box::new(item(more.into(), format!("      …and {} more", total - MAX_ROWS), apps.desktop)?));
            }
            items.push(Box::new(PredefinedMenuItem::separator(app)?));
            Ok(())
        };

        section("Review requested", &req_rows, "Nothing waiting on you 🎉", n, requested.len())?;
        let empty = if s.actionable_only { "Nothing to act on" } else { "No open PRs" };
        section("My PRs", &my_rows, empty, mine_open, mine.len())?;

        items.push(Box::new(item("status".into(), footer.clone(), false)?));
        items.push(Box::new(MenuItem::with_id(app, "refresh", if busy { "Refreshing…" } else { "Refresh now" }, !busy, Some("CmdOrCtrl+R"))?));
        if apps.desktop {
            items.push(Box::new(item("open".into(), "Open PR Review".into(), true)?));
        }
        items.push(Box::new(MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?));
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        items.push(Box::new(MenuItem::with_id(app, "quit", "Quit PR Bar", true, Some("CmdOrCtrl+Q"))?));
        let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|b| b.as_ref()).collect();
        Menu::with_items(app, &refs)
    };
    // Replacing the menu closes it if it's open, so only do it when the contents changed.
    let ids = |v: &[Row]| v.iter().map(|r| format!("{}{}{}", r.id, r.text, r.icon.is_some())).collect::<Vec<_>>();
    let sig = format!("{:?}|{:?}|{footer}|{busy}|{}|{n}|{mine_open}", ids(&req_rows), ids(&my_rows), apps.desktop);
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
