//! PR Bar: the menu bar half of PR Review. Polls GitHub, shows what needs you, and sends
//! the notifications. PR Review (the desktop app) reads what it fetches; see `pr_core::sync`.

mod tray;

use pr_core::notify::Notifier;
use pr_core::poll::{self, Poller};
use pr_core::settings::{self, Settings};
use pr_core::sync::{self, Snapshot};
use pr_core::{ignore, repos, shell, url_encode};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

/// Whether PR Review is installed, for the menu's "Open in PR Review".
#[derive(Clone, Copy, Default)]
pub struct Apps {
    pub desktop: bool,
}

pub struct BarState {
    pub settings: Arc<Mutex<Settings>>,
    pub data_dir: PathBuf,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub apps: Mutex<Apps>,
    /// What the menu last showed, to skip rebuilding an identical one.
    pub menu_sig: Mutex<String>,
    wake: Mutex<Sender<()>>,
}

fn detect_apps() -> Apps {
    Apps { desktop: shell::app_installed("PR Review", "prreview") }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn wake(app: &AppHandle) {
    let _ = app.state::<BarState>().wake.lock().unwrap().send(());
}

/// Settings fields that change what is fetched; anything else only redraws the menu.
fn needs_refetch(old: &Settings, new: &Settings) -> bool {
    old.categories != new.categories
        || old.watched_repos != new.watched_repos
        || old.poll_interval_secs != new.poll_interval_secs
        || old.approved_grace_minutes != new.approved_grace_minutes
}

// ---------- polling ----------

fn spawn_poller(app: AppHandle, rx: Receiver<()>) {
    std::thread::spawn(move || {
        let state = app.state::<BarState>();
        let data = state.data_dir.clone();
        let mut poller = Poller::default();
        let mut notifier = Notifier::load(&data);
        loop {
            let s = state.settings.lock().unwrap().clone();
            let prev = state.snapshot.lock().unwrap().clone();
            let snap = poller.cycle(&data, &s, &prev);
            if snap.errors.is_empty() || !snap.prs.is_empty() {
                for n in notifier.diff(&s, &snap.me, &snap.prs).0 {
                    notify(&app, &n.title, &n.body);
                }
                sync::write_snapshot(&data, &snap);
            }
            *state.snapshot.lock().unwrap() = snap;
            tray::refresh(&app);

            let _ = rx.recv_timeout(Duration::from_secs(s.poll_interval_secs.max(30)));
            while rx.try_recv().is_ok() {}
        }
    });
}

/// Heartbeat, plus reacting to PR Review: refresh requests, settings edits, finished runs, ignores.
fn spawn_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let state = app.state::<BarState>();
        let data = state.data_dir.clone();
        let mut watch = sync::Watch::new(&data, &[sync::REFRESH, sync::SETTINGS, sync::IGNORED]);
        let mut beat: Option<Instant> = None;
        let mut minute = Instant::now();
        let mut apps_checked = Instant::now();
        loop {
            if beat.is_none_or(|t| t.elapsed() >= Duration::from_secs(15)) {
                sync::touch(&data, sync::BAR_ALIVE);
                beat = Some(Instant::now());
            }
            let changed = watch.changed(&data);
            let mut redraw = false;
            for c in &changed {
                match c.as_str() {
                    sync::REFRESH => wake(&app),
                    sync::SETTINGS => {
                        let new = settings::load(&data.join(sync::SETTINGS));
                        let old = std::mem::replace(&mut *state.settings.lock().unwrap(), new.clone());
                        if needs_refetch(&old, &new) { wake(&app) } else { redraw = true }
                    }
                    sync::IGNORED => {
                        let mut snap = state.snapshot.lock().unwrap();
                        poll::reapply_ignores(&data, &mut snap);
                        sync::write_snapshot(&data, &snap);
                        redraw = true;
                    }
                    _ => redraw = true,
                }
            }
            // Keeps ages and the tooltip's "updated n min ago" honest.
            if minute.elapsed() >= Duration::from_secs(60) {
                minute = Instant::now();
                redraw = true;
            }
            if apps_checked.elapsed() >= Duration::from_secs(300) {
                apps_checked = Instant::now();
                *state.apps.lock().unwrap() = detect_apps();
            }
            if redraw {
                tray::refresh(&app);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
}

// ---------- menu actions ----------

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    #[cfg(target_os = "macos")]
    let child = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn();
    #[cfg(windows)]
    let child = shell::command("clip").stdin(std::process::Stdio::piped()).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let child: std::io::Result<std::process::Child> = Err(std::io::Error::other("no clipboard"));
    if let Ok(mut c) = child {
        if let Some(mut stdin) = c.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = c.wait();
    }
}

fn open_url(app: &AppHandle, url: &str) {
    let _ = app.opener().open_url(url, None::<&str>);
}

fn on_menu(app: &AppHandle, id: &str) {
    let state = app.state::<BarState>();
    let pr = |key: &str| state.snapshot.lock().unwrap().prs.iter().find(|p| p.key == key).cloned();
    match id {
        "open" => open_url(app, "prreview://open"),
        "refresh" => wake(app),
        "settings" => show_settings(app),
        "quit" => app.exit(0),
        _ => {
            let Some((action, key)) = id.split_once(':') else { return };
            match action {
                "open" => open_url(app, &format!("prreview://open?pr={}", url_encode(key))),
                "gh" => {
                    if let Some(p) = pr(key) {
                        open_url(app, &p.url);
                    }
                }
                "copy" => {
                    // Ready to paste into Claude Desktop, a terminal, wherever.
                    let Some(p) = pr(key) else { return };
                    let s = state.settings.lock().unwrap().clone();
                    copy_to_clipboard(&s.review_command.replace("{url}", &p.url).replace("{number}", &p.number.to_string()).replace("{repo}", &p.repo));
                }
                "ignore" => {
                    if let Some(p) = pr(key) {
                        ignore::set(&state.data_dir, &p.key, &p.title, true);
                        let mut snap = state.snapshot.lock().unwrap();
                        poll::reapply_ignores(&state.data_dir, &mut snap);
                        sync::write_snapshot(&state.data_dir, &snap);
                        drop(snap);
                        tray::refresh(app);
                    }
                }
                _ => {}
            }
        }
    }
}

fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("bar.html".into()))
        .title("PR Bar Settings")
        .inner_size(760.0, 820.0)
        .min_inner_size(560.0, 480.0)
        .focused(true)
        .build()
        .map(|w| w.set_focus());
}

// ---------- commands (for the settings window) ----------

#[tauri::command]
fn get_settings(state: tauri::State<BarState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, state: tauri::State<BarState>, settings: Settings) -> Result<(), String> {
    settings::save(&state.data_dir.join(sync::SETTINGS), &settings)?;
    let old = std::mem::replace(&mut *state.settings.lock().unwrap(), settings.clone());
    if needs_refetch(&old, &settings) { wake(&app) } else { tray::refresh(&app) }
    Ok(())
}

#[tauri::command]
async fn discover_repos(state: tauri::State<'_, BarState>) -> Result<Value, String> {
    let s = state.settings.lock().unwrap().clone();
    let found = tauri::async_runtime::spawn_blocking(move || repos::discover(&s)).await.map_err(|e| e.to_string())?;
    Ok(json!(found))
}

#[tauri::command]
fn refresh(app: AppHandle) {
    wake(&app);
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
fn test_notification(app: AppHandle) {
    // macOS only shows banners for apps in the background: give the user time to switch away.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        notify(&app, "Review requested · Attensi/example#123", "This is a test notification from PR Bar.");
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    shell::adopt_login_shell_path();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let data_dir = pr_core::data_dir();
            std::fs::create_dir_all(&data_dir)?;
            let s = settings::load(&data_dir.join(sync::SETTINGS));
            // Start from the last list (ours or PR Review's) while the first fetch runs.
            let snap = sync::read_snapshot(&data_dir).unwrap_or_default();
            let (tx, rx) = channel();
            app.manage(BarState {
                settings: Arc::new(Mutex::new(s)),
                data_dir: data_dir.clone(),
                snapshot: Arc::new(Mutex::new(snap)),
                apps: Mutex::new(detect_apps()),
                menu_sig: Mutex::new(String::new()),
                wake: Mutex::new(tx),
            });

            // A menu bar app should start at login; switch it on once, then it's the user's call.
            let marker = data_dir.join("bar.autostart-set");
            if !marker.exists() {
                let _ = app.autolaunch().enable();
                let _ = std::fs::write(&marker, "");
            }

            TrayIconBuilder::with_id("main")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
                .build(app)?;
            tray::refresh(app.handle());

            spawn_poller(app.handle().clone(), rx);
            spawn_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            discover_repos,
            refresh,
            get_autostart,
            set_autostart,
            test_notification
        ])
        .build(tauri::generate_context!())
        .expect("error while building PR Bar")
        .run(|_, event| {
            // Closing the Settings window closes the last window, which would quit the app.
            // Only Quit (an explicit exit code) should.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
