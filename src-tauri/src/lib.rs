mod cleanup;
mod github;
mod grace;
mod ignore;
mod jobs;
mod repos;
mod settings;
mod shell;
mod slack;
mod store;
mod tray;

use github::Pr;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use settings::Settings;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt;

const LAUNCHED_AT_LOGIN: &str = "--hidden";

pub struct AppState {
    pub settings: Arc<Mutex<Settings>>,
    pub data_dir: PathBuf,
    pub jobs: jobs::Jobs,
    snapshot: Arc<Mutex<Snapshot>>,
    wake: Mutex<Sender<()>>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    me: String,
    prs: Vec<Pr>,
    errors: Vec<String>,
    fetched_at: u64,
    loading: bool,
}

/// What we last told the user about each PR, so we only notify on change.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seen {
    head: String,
    ci: Option<String>,
    reviews: usize,
    categories: Vec<String>,
    #[serde(default)]
    draft: bool,
}

fn settings_path(data: &PathBuf) -> PathBuf {
    data.join("settings.json")
}

// ---------- polling & notifications ----------

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn diff_and_notify(app: &AppHandle, s: &Settings, me: &str, prs: &[Pr], seen: &mut HashMap<String, Seen>, first_run: bool) {
    let nc = &s.notify_categories;
    let mut new_requests: Vec<&Pr> = vec![];
    for pr in prs {
        let has = |c: &str| pr.categories.iter().any(|x| x == c);
        let reviews_by_others = pr.reviews.iter().filter(|r| r.author != me).count();
        let prev = seen.get(&pr.key).cloned();
        let now = Seen {
            head: pr.head_sha.clone(),
            ci: pr.ci.clone(),
            reviews: reviews_by_others,
            categories: pr.categories.clone(),
            draft: pr.is_draft,
        };
        seen.insert(pr.key.clone(), now);
        if pr.ignored || (pr.team_only && s.direct_requests_only) {
            continue;
        }
        if first_run || !s.notifications || (pr.is_draft && !s.show_drafts) {
            continue;
        }
        // A hidden draft that turns ready counts as new.
        let was_hidden = prev.as_ref().is_some_and(|p| p.draft && !s.show_drafts);
        let was = |c: &str| !was_hidden && prev.as_ref().map(|p| p.categories.iter().any(|x| x == c)).unwrap_or(false);
        let label = format!("{}#{}", pr.repo, pr.number);

        if has("reviewRequested") && !was("reviewRequested") {
            new_requests.push(pr);
            if nc.review_requested {
                notify(app, &format!("Review requested · {label}"), &format!("{} — by {}", pr.title, pr.author));
            }
            continue;
        }
        if has("reReview") && nc.re_review {
            if let Some(p) = &prev {
                if p.head != pr.head_sha && !p.head.is_empty() {
                    notify(app, &format!("New commits since your review · {label}"), &pr.title);
                    continue;
                }
            }
        }
        if has("watched") && prev.is_none() && nc.watched && pr.author != me {
            notify(app, &format!("New PR · {label}"), &format!("{} — by {}", pr.title, pr.author));
            continue;
        }
        if has("mine") && nc.mine {
            if let Some(p) = &prev {
                if p.ci != pr.ci {
                    match pr.ci.as_deref() {
                        Some("SUCCESS") => notify(app, &format!("CI passed · {label}"), &pr.title),
                        Some("FAILURE") | Some("ERROR") => notify(app, &format!("CI failed · {label}"), &pr.title),
                        _ => {}
                    }
                }
                if reviews_by_others > p.reviews {
                    if let Some(r) = pr.reviews.iter().rev().find(|r| r.author != me) {
                        let verb = match r.state.as_str() {
                            "APPROVED" => "approved",
                            "CHANGES_REQUESTED" => "requested changes on",
                            _ => "commented on",
                        };
                        notify(app, &format!("{} {verb} your PR", r.author), &format!("{label} · {}", pr.title));
                    }
                }
            }
        }
    }
    if s.auto_insights {
        for pr in new_requests {
            let _ = jobs::start(app.clone(), pr.repo.clone(), pr.number, "insights".into(), None, false);
        }
    }
}

fn spawn_poller(app: AppHandle, rx: std::sync::mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let seen_path = state.data_dir.join("seen.json");
        let mut first_run = !seen_path.exists();
        let mut seen: HashMap<String, Seen> = std::fs::read_to_string(&seen_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut last_cleanup: Option<Instant> = None;
        // PRs that left the open lists, by key: (last known PR with closed_state set, when).
        let mut gone: HashMap<String, (github::Pr, u64)> = HashMap::new();
        loop {
            let s = state.settings.lock().unwrap().clone();
            {
                let mut snap = state.snapshot.lock().unwrap();
                snap.loading = true;
                let _ = app.emit("prs-updated", snap.clone());
            }
            let grace = if s.approved_grace_minutes > 0 { grace::active(&state.data_dir, s.approved_grace_minutes) } else { vec![] };
            let mut res = github::fetch_all(&s, &grace);
            ignore::apply(&state.data_dir, &mut res.prs);
            if res.errors.is_empty() || !res.prs.is_empty() {
                diff_and_notify(&app, &s, &res.me, &res.prs, &mut seen, first_run);
                first_run = false;
                let _ = std::fs::write(&seen_path, serde_json::to_string(&seen).unwrap_or_default());
            }
            // A PR that vanished was merged or closed (or I was un-requested). Ask which,
            // and keep merged/closed ones on show for the grace period.
            let window = s.approved_grace_minutes * 60_000;
            let now = jobs::now_ms();
            if res.errors.is_empty() && window > 0 {
                let prev: Vec<github::Pr> = state.snapshot.lock().unwrap().prs.iter().filter(|p| p.closed_state.is_none()).cloned().collect();
                for p in prev {
                    if res.prs.iter().any(|x| x.key == p.key) || gone.contains_key(&p.key) {
                        continue;
                    }
                    if let Some(st) = github::pr_state(&p.repo, p.number).filter(|st| st != "OPEN") {
                        let mut p = p;
                        p.closed_state = Some(st);
                        p.needs_action = false;
                        p.just_approved = false;
                        gone.insert(p.key.clone(), (p, now));
                    }
                }
            }
            gone.retain(|k, (_, at)| now.saturating_sub(*at) < window && !res.prs.iter().any(|x| &x.key == k));
            res.prs.extend(gone.values().map(|(p, _)| p.clone()));
            let snap = {
                let mut snap = state.snapshot.lock().unwrap();
                *snap = Snapshot {
                    me: if res.me.is_empty() { snap.me.clone() } else { res.me },
                    prs: res.prs,
                    errors: res.errors,
                    fetched_at: jobs::now_ms(),
                    loading: false,
                };
                snap.clone()
            };
            let _ = app.emit("prs-updated", snap.clone());
            tray::refresh(&app);

            if !snap.loading && snap.errors.is_empty() && last_cleanup.is_none_or(|t| t.elapsed() >= cleanup::INTERVAL) {
                last_cleanup = Some(Instant::now());
                let app2 = app.clone();
                let open: HashSet<String> = snap.prs.iter().filter(|p| p.closed_state.is_none()).map(|p| p.key.clone()).collect();
                std::thread::spawn(move || {
                    let state = app2.state::<AppState>();
                    let days = state.settings.lock().unwrap().worktree_retention_days;
                    let _ = cleanup::run(&state.data_dir, days, &open, &busy_keys(&state), false);
                });
            }

            let wait = Duration::from_secs(s.poll_interval_secs.max(30));
            match rx.recv_timeout(wait) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {
                    while rx.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

fn busy_keys(state: &AppState) -> HashSet<String> {
    state
        .jobs
        .lock()
        .map(|m| m.values().filter(|j| j.info.status == "running").map(|j| j.info.pr_key.clone()).collect())
        .unwrap_or_default()
}

// ---------- window ----------

fn show_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn hide_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    // Drop out of the dock and app switcher; the menu-bar icon stays.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

// ---------- commands ----------

#[tauri::command]
fn get_snapshot(state: tauri::State<AppState>) -> Snapshot {
    state.snapshot.lock().unwrap().clone()
}

#[tauri::command]
fn refresh(state: tauri::State<AppState>) {
    let _ = state.wake.lock().unwrap().send(());
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: tauri::State<AppState>, settings: Settings) -> Result<(), String> {
    settings::save(&settings_path(&state.data_dir), &settings)?;
    *state.settings.lock().unwrap() = settings;
    let _ = state.wake.lock().unwrap().send(());
    Ok(())
}

#[tauri::command]
async fn discover_repos(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let s = state.settings.lock().unwrap().clone();
    let found = tauri::async_runtime::spawn_blocking(move || repos::discover(&s))
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!(found))
}

#[tauri::command]
async fn pr_detail(repo: String, number: u64) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || github::pr_detail(&repo, number))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn pr_diff(repo: String, number: u64) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || github::pr_diff(&repo, number))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn pr_state(state: tauri::State<AppState>, repo: String, number: u64) -> store::PrState {
    store::load(&state.data_dir, &repo, number)
}

#[tauri::command]
fn save_drafts(state: tauri::State<AppState>, repo: String, number: u64, drafts: Value) -> Result<(), String> {
    store::write(&state.data_dir, &repo, number, "drafts.json", &serde_json::to_string_pretty(&drafts).unwrap())
}

#[tauri::command]
fn start_job(app: AppHandle, repo: String, number: u64, kind: String, message: Option<String>, chrome: Option<bool>) -> Result<jobs::JobInfo, String> {
    jobs::start(app, repo, number, kind, message, chrome.unwrap_or(false))
}

#[tauri::command]
fn cancel_job(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    jobs::cancel(&state.jobs, &id)
}

#[tauri::command]
fn list_jobs(state: tauri::State<AppState>) -> Vec<jobs::JobInfo> {
    let mut v: Vec<_> = state.jobs.lock().unwrap().values().map(|j| j.info.clone()).collect();
    v.sort_by_key(|j| j.started_at);
    v
}

#[tauri::command]
fn job_lines(state: tauri::State<AppState>, id: String) -> Vec<String> {
    state.jobs.lock().unwrap().get(&id).map(|j| j.lines.clone()).unwrap_or_default()
}

#[tauri::command]
fn clean_worktree(state: tauri::State<AppState>, repo: String, number: u64) -> Result<(), String> {
    cleanup::remove(&store::worktree_dir(&state.data_dir, &repo, number));
    Ok(())
}

#[tauri::command]
async fn cleanup_worktrees(app: AppHandle, everything: bool) -> Result<cleanup::Report, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let days = state.settings.lock().unwrap().worktree_retention_days;
        let open: HashSet<String> = state.snapshot.lock().unwrap().prs.iter().filter(|p| p.closed_state.is_none()).map(|p| p.key.clone()).collect();
        cleanup::run(&state.data_dir, days, &open, &busy_keys(&state), everything)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn worktree_usage(app: AppHandle) -> Result<cleanup::Usage, String> {
    tauri::async_runtime::spawn_blocking(move || cleanup::usage(&app.state::<AppState>().data_dir))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn merge_info(repo: String, number: u64) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || github::merge_info(&repo, number))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn merge_pr(app: AppHandle, request: github::MergeRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = github::merge(&request)?;
        let state = app.state::<AppState>();
        // The PR is gone from the lists now; drop its worktree and refresh straight away.
        if !request.auto {
            cleanup::remove(&store::worktree_dir(&state.data_dir, &request.repo, request.number));
        }
        let _ = state.wake.lock().unwrap().send(());
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn submit_review(app: AppHandle, request: github::ReviewRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = github::submit_review(&request)?;
        if request.event == "approve" {
            grace::record(&app.state::<AppState>().data_dir, &format!("{}#{}", request.repo, request.number));
        }
        let _ = app.state::<AppState>().wake.lock().unwrap().send(());
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn slack_lookup(app: AppHandle, repo: String, number: u64, login: String, force: Option<bool>) -> Result<slack::Lookup, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let s = state.settings.lock().unwrap().clone();
        slack::lookup(&s, &state.data_dir, &repo, number, &login, force.unwrap_or(false))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn slack_send(app: AppHandle, login: String, user: slack::SlackUser, message: String) -> Result<slack::Sent, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let s = state.settings.lock().unwrap().clone();
        slack::send(&s, &state.data_dir, &login, &user, &message)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn slack_test(app: AppHandle) -> Result<slack::Sent, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<AppState>().settings.lock().unwrap().clone();
        slack::send_test(&s)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn test_notification(app: AppHandle) {
    // macOS only shows banners for apps in the background, and the user just clicked
    // this inside the app: give them time to switch away.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        notify(&app, "Review requested · Attensi/example#123", "This is a test notification from PR Review.");
    });
}

#[tauri::command]
fn slack_forget(state: tauri::State<AppState>, login: String) {
    slack::forget(&state.data_dir, &login);
}

#[tauri::command]
fn pr_chat(state: tauri::State<AppState>, repo: String, number: u64) -> Vec<store::ChatMessage> {
    store::chat(&state.data_dir, &repo, number)
}

#[tauri::command]
fn clear_thread(app: AppHandle, state: tauri::State<AppState>, repo: String, number: u64) -> Result<(), String> {
    store::clear_thread(&state.data_dir, &repo, number)?;
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
fn pr_index(state: tauri::State<AppState>) -> HashMap<String, store::IndexEntry> {
    store::index(&state.data_dir)
}

/// Continue this PR's Claude conversation outside the app.
#[tauri::command]
fn handoff(app: AppHandle, repo: String, number: u64, target: String, url: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let session = store::resumable(&state.data_dir, &repo, number);
    match target.as_str() {
        "desktop" => {
            let link = match &session {
                // Claude Desktop imports the CLI session into its Code tab.
                Some((sid, _)) => format!("claude://resume?session={sid}"),
                None => {
                    let s = state.settings.lock().unwrap().clone();
                    let folder = repos::resolve(&s, &repo).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                    let q = format!("Let's review {url}. Start with `gh pr view {url}` and `gh pr diff {url}`.");
                    let enc = |v: &str| v.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
                    let mut l = format!("claude://code/new?q={}", enc(&q));
                    if !folder.is_empty() {
                        l.push_str(&format!("&folder={}", enc(&folder)));
                    }
                    l
                }
            };
            #[cfg(windows)]
            {
                use tauri_plugin_opener::OpenerExt;
                app.opener().open_url(link, None::<&str>).map_err(|e| e.to_string())
            }
            #[cfg(not(windows))]
            shell::run("open", &[&link], None).map(|_| ())
        }
        "terminal" => {
            #[cfg(windows)]
            {
                // A fresh console runs claude directly: no shell quoting to get wrong, and
                // Windows 11 hosts it in Windows Terminal when that is the default terminal.
                use std::os::windows::process::CommandExt;
                const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
                let s = state.settings.lock().unwrap().clone();
                let claude = jobs::claude_bin(&s)?;
                let (dir, args): (PathBuf, Vec<String>) = match &session {
                    Some((sid, cwd)) => (PathBuf::from(cwd), vec!["--resume".into(), sid.clone()]),
                    None => (
                        repos::resolve(&s, &repo).or_else(dirs::home_dir).unwrap_or_default(),
                        vec![format!("Let's review {url}")],
                    ),
                };
                jobs::claude_command(&claude)
                    .args(&args)
                    .current_dir(&dir)
                    .creation_flags(CREATE_NEW_CONSOLE)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| format!("failed to start claude: {e}"))
            }
            #[cfg(not(windows))]
            {
                let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
                let cmd = match &session {
                    Some((sid, cwd)) => format!("cd {} && claude --resume {}", quote(cwd), quote(sid)),
                    None => {
                        let s = state.settings.lock().unwrap().clone();
                        let dir = repos::resolve(&s, &repo).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "~".into());
                        format!("cd {} && claude {}", quote(&dir), quote(&format!("Let's review {url}")))
                    }
                };
                let script = format!("tell application \"Terminal\"\n activate\n do script {}\nend tell", serde_json::to_string(&cmd).unwrap());
                shell::run("osascript", &["-e", &script], None).map(|_| ())
            }
        }
        t => Err(format!("unknown handoff target {t}")),
    }
}

#[tauri::command]
fn set_ignored(app: AppHandle, key: String, title: String, ignored: bool) {
    let state = app.state::<AppState>();
    ignore::set(&state.data_dir, &key, &title, ignored);
    let snap = {
        let mut snap = state.snapshot.lock().unwrap();
        for p in snap.prs.iter_mut().filter(|p| p.key == key) {
            p.ignored = ignored;
        }
        snap.clone()
    };
    let _ = app.emit("prs-updated", snap);
    tray::refresh(&app);
}

#[tauri::command]
fn run_history(state: tauri::State<AppState>) -> Vec<Value> {
    store::history(&state.data_dir)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let al = app.autolaunch();
    if enabled { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
    al.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn hide_main(app: AppHandle) {
    hide_window(&app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    shell::adopt_login_shell_path();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        // Login launches pass --hidden, so the app starts quietly in the menu bar.
        .plugin(tauri_plugin_autostart::Builder::new().arg(LAUNCHED_AT_LOGIN).build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let s = settings::load(&settings_path(&data_dir));
            slack::init(&data_dir);
            let (tx, rx) = channel();
            app.manage(AppState {
                settings: Arc::new(Mutex::new(s)),
                data_dir,
                jobs: Arc::new(Mutex::new(HashMap::new())),
                snapshot: Arc::new(Mutex::new(Snapshot { loading: true, ..Default::default() })),
                wake: Mutex::new(tx),
            });

            let open = MenuItem::with_id(app, "open", "Open PR Review", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit PR Review", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let id = event.id().as_ref().to_string();
                    match id.as_str() {
                        "open" => show_window(app),
                        "refresh" => {
                            let _ = app.state::<AppState>().wake.lock().unwrap().send(());
                        }
                        "settings" => {
                            show_window(app);
                            let _ = app.emit("open-settings", ());
                        }
                        "quit" => app.exit(0),
                        _ => {
                            if let Some(key) = id.strip_prefix("pr:") {
                                show_window(app);
                                let _ = app.emit("open-pr", key.to_string());
                            }
                        }
                    }
                })
                .build(app)?;

            spawn_poller(app.handle().clone(), rx);
            if std::env::args().any(|a| a == LAUNCHED_AT_LOGIN) {
                hide_window(app.handle());
            } else {
                show_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                hide_window(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            clear_thread,
            refresh,
            get_settings,
            save_settings,
            discover_repos,
            pr_detail,
            pr_diff,
            pr_state,
            save_drafts,
            start_job,
            cancel_job,
            list_jobs,
            job_lines,
            clean_worktree,
            hide_main,
            get_autostart,
            set_autostart,
            run_history,
            cleanup_worktrees,
            worktree_usage,
            merge_info,
            merge_pr,
            submit_review,
            slack_lookup,
            slack_send,
            slack_forget,
            pr_chat,
            set_ignored,
            pr_index,
            handoff,
            slack_test,
            test_notification
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_window(app);
        }
        let _ = (app, &event);
    });
}
