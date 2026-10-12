mod jobs;
mod slack;

// The shared modules keep their old paths (`crate::store`, …) for the code that moved out.
pub use pr_core::{cleanup, github, grace, ignore, repos, settings, shell, store};

use pr_core::sync::{self, Snapshot};
use serde_json::{json, Value};
use settings::Settings;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_deep_link::DeepLinkExt;

pub struct AppState {
    pub settings: Arc<Mutex<Settings>>,
    pub data_dir: PathBuf,
    pub jobs: jobs::Jobs,
    snapshot: Arc<Mutex<Snapshot>>,
    wake: Mutex<Sender<()>>,
}

fn settings_path(data: &PathBuf) -> PathBuf {
    data.join("settings.json")
}

// ---------- PR list ----------

/// Fetch now: through PR Bar when it is running (it owns polling), else ourselves.
fn request_refresh(state: &AppState) {
    if sync::bar_alive(&state.data_dir) {
        sync::touch(&state.data_dir, sync::REFRESH);
    }
    let _ = state.wake.lock().unwrap().send(());
}

fn publish(app: &AppHandle, snap: Snapshot) {
    *app.state::<AppState>().snapshot.lock().unwrap() = snap.clone();
    let _ = app.emit("prs-updated", snap);
}

/// While PR Bar runs, mirror its snapshot. Otherwise poll GitHub ourselves, without
/// notifications (those are PR Bar's job), and share what we fetched.
fn spawn_poller(app: AppHandle, rx: std::sync::mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let data = state.data_dir.clone();
        let mut poller = pr_core::poll::Poller::default();
        let mut watch = sync::Watch::new(&data, &[sync::SNAPSHOT, sync::SETTINGS, sync::IGNORED]);
        let mut last_fetch: Option<Instant> = None;
        let mut last_cleanup: Option<Instant> = None;
        let mut known_requests: Option<HashSet<String>> = None;
        let mut loaded_snapshot = false;
        loop {
            let changed = watch.changed(&data);
            if changed.iter().any(|c| c == sync::SETTINGS) {
                *state.settings.lock().unwrap() = settings::load(&data.join(sync::SETTINGS));
            }
            let s = state.settings.lock().unwrap().clone();
            let bar = sync::bar_alive(&data);
            let due = last_fetch.is_none_or(|t| t.elapsed() >= Duration::from_secs(s.poll_interval_secs.max(30)));

            let mut fresh: Option<Snapshot> = None;
            if bar {
                if !loaded_snapshot || changed.iter().any(|c| c == sync::SNAPSHOT) {
                    fresh = sync::read_snapshot(&data);
                }
            } else if due {
                let prev = state.snapshot.lock().unwrap().clone();
                if prev.fetched_at == 0 {
                    // First look: show what PR Bar (or our last run) saw while we fetch.
                    if let Some(old) = sync::read_snapshot(&data) {
                        publish(&app, Snapshot { loading: true, ..old });
                    }
                }
                let mut loading = state.snapshot.lock().unwrap().clone();
                loading.loading = true;
                let _ = app.emit("prs-updated", loading);
                let base = sync::read_snapshot(&data).filter(|_| prev.fetched_at == 0).unwrap_or(prev);
                let snap = poller.cycle(&data, &s, &base);
                if snap.errors.is_empty() || !snap.prs.is_empty() {
                    sync::write_snapshot(&data, &snap);
                    watch.changed(&data);
                }
                last_fetch = Some(Instant::now());
                fresh = Some(snap);
            } else if changed.iter().any(|c| c == sync::IGNORED) {
                let mut snap = state.snapshot.lock().unwrap().clone();
                pr_core::poll::reapply_ignores(&data, &mut snap);
                fresh = Some(snap);
            }

            if let Some(snap) = fresh {
                loaded_snapshot = true;
                auto_insights(&app, &s, &snap, &mut known_requests);
                publish(&app, snap);
            }

            let snap = state.snapshot.lock().unwrap().clone();
            if snap.fetched_at > 0 && snap.errors.is_empty() && last_cleanup.is_none_or(|t| t.elapsed() >= cleanup::INTERVAL) {
                last_cleanup = Some(Instant::now());
                let app2 = app.clone();
                let open: HashSet<String> = snap.prs.iter().filter(|p| p.closed_state.is_none()).map(|p| p.key.clone()).collect();
                std::thread::spawn(move || {
                    let state = app2.state::<AppState>();
                    let days = state.settings.lock().unwrap().worktree_retention_days;
                    let _ = cleanup::run(&state.data_dir, days, &open, &busy_keys(&state), false);
                });
            }

            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(()) => {
                    while rx.try_recv().is_ok() {}
                    last_fetch = None;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

/// "Explain new review requests automatically": start Explain for requests we haven't seen.
fn auto_insights(app: &AppHandle, s: &Settings, snap: &Snapshot, known: &mut Option<HashSet<String>>) {
    let now: HashSet<String> = snap
        .prs
        .iter()
        .filter(|p| p.closed_state.is_none() && !p.just_approved && p.categories.iter().any(|c| c == "reviewRequested"))
        .map(|p| p.key.clone())
        .collect();
    if let Some(before) = known.as_ref() {
        if s.auto_insights {
            for p in snap.prs.iter().filter(|p| now.contains(&p.key) && !before.contains(&p.key) && !p.ignored) {
                let _ = jobs::start(app.clone(), p.repo.clone(), p.number, "insights".into(), None, false);
            }
        }
    }
    *known = Some(now);
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
}

/// `prreview://open?pr=owner/repo%23123`, from PR Bar.
fn handle_link(app: &AppHandle, url: &str) {
    show_window(app);
    let Some(q) = url.split_once('?').map(|(_, q)| q) else { return };
    for (k, v) in q.split('&').filter_map(|kv| kv.split_once('=')) {
        if k == "pr" {
            let _ = app.emit("open-pr", pr_core::url_decode(v));
        }
    }
}

// ---------- commands ----------

#[tauri::command]
fn get_snapshot(state: tauri::State<AppState>) -> Snapshot {
    state.snapshot.lock().unwrap().clone()
}

#[tauri::command]
fn refresh(state: tauri::State<AppState>) {
    request_refresh(&state);
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: tauri::State<AppState>, settings: Settings) -> Result<(), String> {
    settings::save(&settings_path(&state.data_dir), &settings)?;
    let old = std::mem::replace(&mut *state.settings.lock().unwrap(), settings.clone());
    // Only a change to what is fetched needs GitHub again. Filters and toggles (needs
    // action, drafts, team, Chrome…) just redraw the menu, which PR Bar picks up from the file.
    let refetch = old.categories != settings.categories
        || old.watched_repos != settings.watched_repos
        || old.poll_interval_secs != settings.poll_interval_secs
        || old.approved_grace_minutes != settings.approved_grace_minutes;
    if refetch {
        request_refresh(&state);
    }
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
        request_refresh(&state);
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn submit_review(app: AppHandle, request: github::ReviewRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = github::submit_review(&request)?;
        let state = app.state::<AppState>();
        let key = format!("{}#{}", request.repo, request.number);
        if request.event == "approve" {
            grace::record(&state.data_dir, &key);
        }
        // Show it now: the refetch below can take seconds, and GitHub's search lags behind.
        let snap = {
            let mut snap = state.snapshot.lock().unwrap();
            let me = snap.me.clone();
            if let Some(p) = snap.prs.iter_mut().find(|p| p.key == key) {
                let st = match request.event.as_str() { "approve" => "APPROVED", "request_changes" => "CHANGES_REQUESTED", _ => "COMMENTED" };
                p.latest_reviews.retain(|r| r.author != me);
                p.latest_reviews.push(github::LatestReview { author: me.clone(), avatar: String::new(), state: st.into() });
                p.just_approved = request.event == "approve";
            }
            snap.clone()
        };
        let _ = app.emit("prs-updated", snap);
        request_refresh(&state);
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
async fn slack_send(app: AppHandle, login: String, user: slack::SlackUser, message: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let s = state.settings.lock().unwrap().clone();
        slack::send(&s, &state.data_dir, &login, &user, &message)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn slack_test(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<AppState>().settings.lock().unwrap().clone();
        slack::send_test(&s)
    })
    .await
    .map_err(|e| e.to_string())?
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
fn clear_thread(state: tauri::State<AppState>, repo: String, number: u64) -> Result<(), String> {
    store::clear_thread(&state.data_dir, &repo, number)?;
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
}

#[tauri::command]
fn run_history(state: tauri::State<AppState>) -> Vec<Value> {
    store::history(&state.data_dir)
}

/// Whether PR Bar is running (it owns the menu bar and notifications), and installed.
#[tauri::command]
fn bar_status(state: tauri::State<AppState>) -> Value {
    json!({ "alive": sync::bar_alive(&state.data_dir), "installed": shell::app_installed("PR Bar", "prbar") })
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
        .plugin(tauri_plugin_deep_link::init())
        // Only to switch off the login item older versions set up: PR Bar starts at login now.
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .setup(|app| {
            let data_dir = pr_core::data_dir();
            std::fs::create_dir_all(&data_dir)?;
            let s = settings::load(&data_dir.join(sync::SETTINGS));
            slack::init(&data_dir);
            let (tx, rx) = channel();
            app.manage(AppState {
                settings: Arc::new(Mutex::new(s)),
                data_dir,
                jobs: Arc::new(Mutex::new(HashMap::new())),
                snapshot: Arc::new(Mutex::new(Snapshot { loading: true, ..Default::default() })),
                wake: Mutex::new(tx),
            });
            if app.autolaunch().is_enabled().unwrap_or(false) {
                let _ = app.autolaunch().disable();
            }

            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    handle_link(&handle, url.as_str());
                }
            });
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for url in urls {
                    handle_link(app.handle(), url.as_str());
                }
            }

            spawn_poller(app.handle().clone(), rx);
            show_window(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app (and any Claude runs) going; the dock icon brings it back.
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
            bar_status,
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
            slack_test
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
