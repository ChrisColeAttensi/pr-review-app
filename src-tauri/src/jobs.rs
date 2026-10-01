//! Headless `claude -p` runs: the pr-review skill, and the insights pass.
//! Output is stream-json; each line is forwarded to the UI as it arrives.

use crate::settings::Settings;
use crate::{github, repos, store, AppState};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: String,
    pub pr_key: String,
    pub repo: String,
    pub number: u64,
    pub kind: String,
    pub started_at: u64,
    pub status: String,
}

pub struct Job {
    pub info: JobInfo,
    pub pid: Option<u32>,
    pub lines: Vec<String>,
}

pub type Jobs = Arc<Mutex<HashMap<String, Job>>>;

pub fn claude_bin(settings: &Settings) -> Result<String, String> {
    if settings.claude_path.trim().is_empty() {
        crate::shell::which("claude").ok_or_else(|| "`claude` not found on PATH; set its path in Settings".to_string())
    } else {
        Ok(settings.claude_path.clone())
    }
}

pub fn claude_command(claude: &str) -> Command {
    let mut cmd = Command::new(claude);
    // Launched from inside a Claude Code session (e.g. `tauri dev` run by Claude), the
    // child would inherit that session's host-auth wiring and fail to log in.
    if std::env::var_os("CLAUDECODE").is_some() {
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_string();
            if k == "CLAUDECODE" || k == "ANTHROPIC_BASE_URL" || k.starts_with("CLAUDE_CODE_")
                || k.starts_with("CLAUDE_AGENT_") || k == "CLAUDE_PID" || k == "CLAUDE_EFFORT"
            {
                cmd.env_remove(k);
            }
        }
    }
    cmd
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

const HEADLESS_NOTE: &str = "You are running headless inside the PR Review desktop app. \
Nobody is watching the terminal and nobody can answer questions. \
Run the review through step 7 and make your final message the report itself, in the 7d layout, as markdown. \
Do NOT do step 8: never post or draft GitHub comments, never push, never edit or fix files, never run `git config`. \
Where a step would ask the user something, take the default the skill recommends and say so in one line in the report. \
The working directory is a throwaway worktree checked out at the PR head.";

const CHROME_NOTE: &str = "Claude in Chrome is connected, and the user allows you to verify this PR in it when needed. \
Decide that yourself: run the live check only for claims the diff, the build and CI cannot settle (a flow, a screen, \
something a user must see); skip it for refactors, docs, config, backend-only or test-only changes, and say in one line \
why it was skipped. When you do run it, treat ask-first live passes as approved, and name Claude in Chrome as the \
driver in the live checker's brief (the built-in browser is not available here). Stay on the app under test in a tab \
of your own, and never sign in, submit forms that send real data, or change account settings.";

pub fn insights_schema() -> Value {
    json!({
      "type": "object",
      "required": ["summary", "risk", "riskReason", "chapters", "annotations", "fileSummaries"],
      "properties": {
        "summary": { "type": "string", "description": "3-5 plain sentences: what this PR changes, and why the author did it." },
        "risk": { "type": "string", "enum": ["low", "medium", "high"] },
        "riskReason": { "type": "string" },
        "chapters": {
          "type": "array",
          "description": "The PR retold as a story, in the order a reviewer should read it. Every changed file belongs to exactly one chapter.",
          "items": {
            "type": "object",
            "required": ["title", "explanation", "files"],
            "properties": {
              "title": { "type": "string" },
              "explanation": { "type": "string", "description": "Markdown. What this group of changes does, and why it is needed." },
              "files": { "type": "array", "items": { "type": "string" } }
            }
          }
        },
        "annotations": {
          "type": "array",
          "description": "Inline notes anchored to diff lines. Explain intent on every non-trivial hunk; flag concerns sparingly and concretely.",
          "items": {
            "type": "object",
            "required": ["path", "line", "side", "kind", "title", "body"],
            "properties": {
              "path": { "type": "string" },
              "line": { "type": "integer", "description": "Line number on the chosen side, as shown in the diff hunk." },
              "side": { "type": "string", "enum": ["new", "old"] },
              "kind": { "type": "string", "enum": ["explain", "why", "concern", "question", "suggestion", "nit"] },
              "title": { "type": "string" },
              "body": { "type": "string", "description": "Markdown." }
            }
          }
        },
        "fileSummaries": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["path", "summary"],
            "properties": { "path": { "type": "string" }, "summary": { "type": "string" } }
          }
        },
        "testingNotes": { "type": "string", "description": "How to exercise this change by hand, and what tests cover it." }
      }
    })
}

fn chat_note(url: &str) -> String {
    format!(
"The user is now talking to you directly, through the chat panel of the PR Review desktop app, about pull request {url}. \
Any earlier instruction that this was an unattended run, or that you must not edit files, applied only to that automated run: \
the user may now ask you to change code, run checks, or explain things. The working directory is a git worktree at the PR head. \
Never post to GitHub (comments, reviews, approvals, merges) and never push until you have shown exactly what you will send \
and the user has said yes in a later message. Keep replies short and concrete; use markdown."
    )
}

fn fresh_chat_prompt(url: &str, report: Option<&str>, message: &str) -> String {
    let mut p = format!("We are reviewing pull request {url}. Read it with `gh pr view {url}` and `gh pr diff {url}` as needed.\n");
    if let Some(r) = report {
        p.push_str(&format!("\nThe pr-review report for it:\n<review-report>\n{r}\n</review-report>\n"));
    }
    p.push_str(&format!("\nThe user says:\n{message}"));
    p
}

fn insights_prompt(url: &str, report: Option<&str>) -> String {
    let mut p = format!(
"Explain the pull request {url} to a reviewer who is about to read it.

1. Read it: `gh pr view {url}` and `gh pr diff {url}`. The working directory is the repo checked out at the PR head, so open the surrounding code, callers and tests whenever a hunk's purpose is not obvious from the diff alone.
2. Work out the author's intent and group the changes into chapters that read as a story (foundations first, then the code that uses them, then tests and config).
3. Annotate the diff. For every non-trivial hunk add an `explain` or `why` note that says what the code does and *why* it is needed in terms of the PR's goal, not a paraphrase of the syntax. Anchor each note to a line that actually appears in the diff (added or context lines on the `new` side; removed lines on the `old` side).
4. Only add `concern`, `question` or `suggestion` notes when you can name the concrete input or situation that goes wrong. No style nits unless they hide a bug.

These notes are private to the reviewer and are never posted. Do not post, push, or edit anything.");
    if let Some(r) = report {
        p.push_str(&format!(
"

A pr-review report already exists for this PR. Anchor each of its findings as a `concern` annotation on the most relevant diff line, quoting its title. Here is the report:

<review-report>
{r}
</review-report>"));
    }
    p
}

fn emit_line(app: &AppHandle, jobs: &Jobs, id: &str, line: String) {
    if let Ok(mut map) = jobs.lock() {
        if let Some(j) = map.get_mut(id) {
            j.lines.push(line.clone());
        }
    }
    let _ = app.emit("job-event", json!({ "jobId": id, "line": line }));
}

fn finish(app: &AppHandle, jobs: &Jobs, id: &str, status: &str, error: Option<String>) {
    let info = jobs.lock().ok().and_then(|mut m| {
        m.get_mut(id).map(|j| {
            j.info.status = status.to_string();
            j.pid = None;
            j.info.clone()
        })
    });
    let _ = app.emit("job-done", json!({ "jobId": id, "status": status, "error": error, "job": info }));
    crate::tray::refresh(app);
}

pub fn start(app: AppHandle, repo: String, number: u64, kind: String, message: Option<String>, chrome: bool) -> Result<JobInfo, String> {
    let state = app.state::<AppState>();
    let jobs = state.jobs.clone();
    {
        let map = jobs.lock().map_err(|e| e.to_string())?;
        if let Some(j) = map
            .values()
            .find(|j| j.info.status == "running" && j.info.repo == repo && j.info.number == number && j.info.kind == kind)
        {
            return Ok(j.info.clone());
        }
    }
    let id = format!("{kind}-{}-{number}-{}", repo.replace('/', "_"), now_ms());
    let info = JobInfo {
        id: id.clone(),
        pr_key: format!("{repo}#{number}"),
        repo: repo.clone(),
        number,
        kind: kind.clone(),
        started_at: now_ms(),
        status: "running".into(),
    };
    jobs.lock().map_err(|e| e.to_string())?.insert(
        id.clone(),
        Job { info: info.clone(), pid: None, lines: vec![] },
    );
    let _ = app.emit("job-started", &info);
    crate::tray::refresh(&app);

    let settings: Settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let data_dir = state.data_dir.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(&app2, &jobs, &id, &settings, &data_dir, &repo, number, &kind, message.as_deref(), chrome) {
            emit_line(&app2, &jobs, &id, json!({ "type": "app_error", "message": e }).to_string());
            if kind == "chat" {
                store::push_chat(&data_dir, &repo, number, store::ChatMessage { role: "error".into(), text: e.clone(), ts: now_ms() });
            }
            finish(&app2, &jobs, &id, "failed", Some(e));
        }
    });
    Ok(info)
}

fn progress(app: &AppHandle, jobs: &Jobs, id: &str, msg: &str) {
    emit_line(app, jobs, id, json!({ "type": "app_progress", "message": msg }).to_string());
}

#[allow(clippy::too_many_arguments)]
fn run(
    app: &AppHandle,
    jobs: &Jobs,
    id: &str,
    settings: &Settings,
    data_dir: &PathBuf,
    repo: &str,
    number: u64,
    kind: &str,
    message: Option<&str>,
    chrome: bool,
) -> Result<(), String> {
    let started = now_ms();
    if kind == "chat" {
        let text = message.filter(|m| !m.trim().is_empty()).ok_or("empty message")?;
        store::push_chat(data_dir, repo, number, store::ChatMessage { role: "user".into(), text: text.into(), ts: started });
    }
    progress(app, jobs, id, "Fetching PR metadata");
    let detail = github::pr_detail(repo, number)?;
    let url = detail.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let head = detail.get("headRefOid").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let base = detail.get("baseRefName").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let title = detail.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let n = |k: &str| detail.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let lines = n("additions") + n("deletions");
    let files = n("changedFiles");

    // Where claude runs: a worktree of the local clone, or a bare scratch dir.
    let cwd = match repos::resolve(settings, repo) {
        Some(clone) => {
            progress(app, jobs, id, &format!("Checking out PR head from {}", clone.display()));
            let wt = store::worktree_dir(data_dir, repo, number);
            // Chat keeps working in the existing checkout: a fresh checkout would throw
            // away edits made earlier in the conversation.
            let prepared = if kind == "chat" && wt.join(".git").exists() {
                Ok(())
            } else {
                repos::prepare_worktree(&clone, &wt, number, &base)
            };
            match prepared {
                Ok(()) => {
                    crate::cleanup::touch(&wt);
                    wt
                }
                Err(e) => {
                    if kind == "review" {
                        return Err(format!("could not prepare worktree: {e}"));
                    }
                    progress(app, jobs, id, &format!("Worktree failed ({e}); explaining from the diff only"));
                    store::pr_dir(data_dir, repo, number)
                }
            }
        }
        None => {
            if kind == "review" {
                return Err(format!(
                    "No local clone of {repo} found. Add its path in Settings → Repositories, or add a repo root that contains it."
                ));
            }
            progress(app, jobs, id, "No local clone found; explaining from the diff only");
            store::pr_dir(data_dir, repo, number)
        }
    };
    let resume = if kind == "chat" { store::resumable(data_dir, repo, number) } else { None };
    // A session can only be resumed from the directory it ran in.
    let cwd = match &resume {
        Some((_, dir)) if PathBuf::from(dir).exists() => PathBuf::from(dir),
        _ => cwd,
    };
    std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;

    let claude = claude_bin(settings)?;

    let mut args: Vec<String> = vec![];
    let model;
    if kind == "review" {
        let prompt = settings
            .review_command
            .replace("{url}", &url)
            .replace("{number}", &number.to_string())
            .replace("{repo}", repo);
        let note = if chrome { format!("{HEADLESS_NOTE} {CHROME_NOTE}") } else { HEADLESS_NOTE.to_string() };
        args.extend(["-p".into(), prompt, "--append-system-prompt".into(), note]);
        model = settings.review_model.clone();
    } else if kind == "chat" {
        let text = message.unwrap_or_default();
        let prompt = match &resume {
            Some(_) => text.to_string(),
            None => fresh_chat_prompt(&url, store::load(data_dir, repo, number).review.as_deref(), text),
        };
        args.extend(["-p".into(), prompt, "--append-system-prompt".into(), chat_note(&url)]);
        if let Some((sid, _)) = &resume {
            args.extend(["--resume".into(), sid.clone()]);
        }
        model = settings.review_model.clone();
    } else {
        let report = store::load(data_dir, repo, number).review;
        args.extend([
            "-p".into(),
            insights_prompt(&url, report.as_deref()),
            "--json-schema".into(),
            insights_schema().to_string(),
        ]);
        model = settings.insights_model.clone();
    }
    args.extend([
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--permission-mode".into(),
        settings.permission_mode.clone(),
    ]);
    if !model.trim().is_empty() {
        args.extend(["--model".into(), model.clone()]);
    }
    if chrome {
        args.push("--chrome".into());
        progress(app, jobs, id, "Live checks enabled through Claude in Chrome");
    }

    if jobs.lock().ok().and_then(|m| m.get(id).map(|j| j.info.status == "cancelled")).unwrap_or(false) {
        finish(app, jobs, id, "cancelled", None);
        return Ok(());
    }
    progress(app, jobs, id, &format!("Starting claude in {}", cwd.display()));
    let mut cmd = claude_command(&claude);
    let mut child = cmd
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("failed to start claude: {e}"))?;

    if let Ok(mut m) = jobs.lock() {
        if let Some(j) = m.get_mut(id) {
            j.pid = Some(child.id());
        }
    }

    let stderr = child.stderr.take();
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let sb = stderr_buf.clone();
    let err_thread = std::thread::spawn(move || {
        if let Some(e) = stderr {
            for line in BufReader::new(e).lines().map_while(Result::ok) {
                if let Ok(mut s) = sb.lock() {
                    s.push_str(&line);
                    s.push('\n');
                }
            }
        }
    });

    let mut result: Option<Value> = None;
    // The final message isn't always the report: a background agent finishing after the
    // report wakes the session for one more (often empty) turn. Keep the last main-thread
    // text that reads as a report, and the last main-thread text of any kind.
    let mut last_report: Option<String> = None;
    let mut last_text: Option<String> = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("result") => result = Some(v),
                    Some("assistant") if v.get("parent_tool_use_id").map_or(true, |p| p.is_null()) => {
                        let blocks = v.pointer("/message/content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                        for b in blocks {
                            if let Some(t) = b.get("text").and_then(|t| t.as_str()).map(str::trim).filter(|t| !t.is_empty()) {
                                if t.starts_with("# ") {
                                    last_report = Some(t.to_string());
                                }
                                last_text = Some(t.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            emit_line(app, jobs, id, line);
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = err_thread.join();

    let cancelled = jobs
        .lock()
        .ok()
        .and_then(|m| m.get(id).map(|j| j.info.status == "cancelled"))
        .unwrap_or(false);
    if cancelled {
        finish(app, jobs, id, "cancelled", None);
        return Ok(());
    }

    let Some(res) = result else {
        let err = stderr_buf.lock().map(|s| s.clone()).unwrap_or_default();
        return Err(format!("claude exited ({status}) without a result. {}", err.trim()));
    };
    let is_error = res.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
    let meta = store::RunMeta {
        head_sha: head,
        finished_at: now_ms(),
        cost_usd: res.get("total_cost_usd").and_then(|v| v.as_f64()),
        // Wall clock, including the worktree checkout: that is what the user waits for.
        duration_ms: Some(now_ms() - started),
        ok: !is_error,
        lines: Some(lines),
        files: Some(files),
        model: Some(model.clone()),
        session_id: res.get("session_id").and_then(|v| v.as_str()).map(String::from),
        cwd: Some(cwd.to_string_lossy().to_string()),
    };
    let text = res.get("result").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if is_error {
        return Err(format!("claude reported an error: {text}"));
    }

    if kind == "chat" {
        let reply = if text.trim().is_empty() { last_text.clone().unwrap_or_default() } else { text.clone() };
        store::push_chat(data_dir, repo, number, store::ChatMessage { role: "assistant".into(), text: reply, ts: now_ms() });
        store::write(data_dir, repo, number, "chat.meta.json", &serde_json::to_string(&meta).unwrap())?;
        finish(app, jobs, id, "done", None);
        let _ = app.notification().builder().title(format!("Claude replied · {repo}#{number}")).body(title).show();
        return Ok(());
    }
    if kind == "review" {
        let report = if text.trim_start().starts_with("# ") { Some(text.clone()) } else { last_report.or(last_text) }
            .filter(|t| !t.trim().is_empty())
            .ok_or("pr-review finished without writing a report; see the run log")?;
        store::write(data_dir, repo, number, "review.md", &report)?;
        store::write(data_dir, repo, number, "review.meta.json", &serde_json::to_string(&meta).unwrap())?;
    } else {
        let structured = res
            .get("structured_output")
            .cloned()
            .filter(|v| v.is_object())
            .or_else(|| extract_json(&text))
            .ok_or("insights run finished but returned no structured output")?;
        store::write(data_dir, repo, number, "insights.json", &serde_json::to_string_pretty(&structured).unwrap())?;
        store::write(data_dir, repo, number, "insights.meta.json", &serde_json::to_string(&meta).unwrap())?;
    }

    store::append_history(data_dir, kind, &meta);
    finish(app, jobs, id, "done", None);
    let label = if kind == "review" { "Review ready" } else { "Insights ready" };
    let _ = app
        .notification()
        .builder()
        .title(format!("{label}: {repo}#{number}"))
        .body(title)
        .show();
    Ok(())
}

/// Fallback when structured output is missing: the first `{ ... }` block in the text.
fn extract_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

pub fn cancel(jobs: &Jobs, id: &str) -> Result<(), String> {
    let mut map = jobs.lock().map_err(|e| e.to_string())?;
    let job = map.get_mut(id).ok_or("no such job")?;
    job.info.status = "cancelled".into();
    if let Some(pid) = job.pid {
        // The whole process group: claude plus the agents and tools it spawned.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    Ok(())
}
