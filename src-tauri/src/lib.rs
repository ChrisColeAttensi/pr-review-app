mod github;
mod jobs;
mod repos;
mod settings;
mod shell;
mod store;

use github::Pr;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent, Wry};
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
        };
        seen.insert(pr.key.clone(), now);
        if first_run || !s.notifications {
            continue;
        }
        let was = |c: &str| prev.as_ref().map(|p| p.categories.iter().any(|x| x == c)).unwrap_or(false);
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
            let _ = jobs::start(app.clone(), pr.repo.clone(), pr.number, "insights".into());
        }
    }
}

fn attention_count(prs: &[Pr]) -> usize {
    prs.iter()
        .filter(|p| p.categories.iter().any(|c| c == "reviewRequested" || c == "reReview"))
        .count()
}

fn rebuild_tray(app: &AppHandle, snap: &Snapshot) {
    let Some(tray) = app.tray_by_id("main") else { return };
    let n = attention_count(&snap.prs);
    let _ = tray.set_title(if n > 0 { Some(n.to_string()) } else { None::<String> });
    let _ = tray.set_tooltip(Some(format!("PR Review — {n} need you")));

    let build = || -> tauri::Result<Menu<Wry>> {
        let mut items: Vec<MenuItem<Wry>> = vec![];
        items.push(MenuItem::with_id(app, "open", "Open PR Review", true, None::<&str>)?);
        items.push(MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?);
        let mut pr_items: Vec<MenuItem<Wry>> = vec![];
        let needing: Vec<&Pr> = snap
            .prs
            .iter()
            .filter(|p| p.categories.iter().any(|c| c == "reviewRequested" || c == "reReview"))
            .take(10)
            .collect();
        if needing.is_empty() {
            pr_items.push(MenuItem::with_id(app, "none", "Nothing waiting on you", false, None::<&str>)?);
        }
        for p in needing {
            let mark = if p.categories.iter().any(|c| c == "reReview") { "↻" } else { "●" };
            let mut t = p.title.clone();
            if t.chars().count() > 48 {
                t = t.chars().take(47).collect::<String>() + "…";
            }
            pr_items.push(MenuItem::with_id(
                app,
                format!("pr:{}", p.key),
                format!("{mark} {t}  ({}#{})", p.repo.split('/').last().unwrap_or(""), p.number),
                true,
                None::<&str>,
            )?);
        }
        let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?;
        let quit = MenuItem::with_id(app, "quit", "Quit PR Review", true, Some("Cmd+Q"))?;
        let sep1 = PredefinedMenuItem::separator(app)?;
        let sep2 = PredefinedMenuItem::separator(app)?;
        let mut refs: Vec<&dyn IsMenuItem<Wry>> = vec![];
        for i in &items {
            refs.push(i);
        }
        refs.push(&sep1);
        for i in &pr_items {
            refs.push(i);
        }
        refs.push(&sep2);
        refs.push(&settings);
        refs.push(&quit);
        Menu::with_items(app, &refs)
    };
    if let Ok(menu) = build() {
        let _ = tray.set_menu(Some(menu));
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
        loop {
            let s = state.settings.lock().unwrap().clone();
            {
                let mut snap = state.snapshot.lock().unwrap();
                snap.loading = true;
                let _ = app.emit("prs-updated", snap.clone());
            }
            let res = github::fetch_all(&s);
            if res.errors.is_empty() || !res.prs.is_empty() {
                diff_and_notify(&app, &s, &res.me, &res.prs, &mut seen, first_run);
                first_run = false;
                let _ = std::fs::write(&seen_path, serde_json::to_string(&seen).unwrap_or_default());
            }
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
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || rebuild_tray(&app2, &snap));

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
fn start_job(app: AppHandle, repo: String, number: u64, kind: String) -> Result<jobs::JobInfo, String> {
    jobs::start(app, repo, number, kind)
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
    let s = state.settings.lock().unwrap().clone();
    let wt = store::worktree_dir(&state.data_dir, &repo, number);
    if let Some(clone) = repos::resolve(&s, &repo) {
        repos::remove_worktree(&clone, &wt);
    }
    Ok(())
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
            run_history
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            show_window(app);
        }
        let _ = (app, &event);
    });
}
