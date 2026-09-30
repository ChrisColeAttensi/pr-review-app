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

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

const HEADLESS_NOTE: &str = "You are running headless inside the PR Review desktop app. \
Nobody is watching the terminal and nobody can answer questions. \
Run the review through step 7 and make your final message the report itself, in the 7d layout, as markdown. \
Do NOT do step 8: never post or draft GitHub comments, never push, never edit or fix files, never run `git config`. \
Where a step would ask the user something, take the default the skill recommends and say so in one line in the report. \
The working directory is a throwaway worktree checked out at the PR head.";

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
}

pub fn start(app: AppHandle, repo: String, number: u64, kind: String) -> Result<JobInfo, String> {
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

    let settings: Settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let data_dir = state.data_dir.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(&app2, &jobs, &id, &settings, &data_dir, &repo, number, &kind) {
            emit_line(&app2, &jobs, &id, json!({ "type": "app_error", "message": e }).to_string());
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
) -> Result<(), String> {
    let started = now_ms();
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
            match repos::prepare_worktree(&clone, &wt, number, &base) {
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
    std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;

    let claude = if settings.claude_path.trim().is_empty() {
        crate::shell::which("claude").ok_or("`claude` not found on PATH; set its path in Settings")?
    } else {
        settings.claude_path.clone()
    };

    let mut args: Vec<String> = vec![];
    let model;
    if kind == "review" {
        let prompt = settings
            .review_command
            .replace("{url}", &url)
            .replace("{number}", &number.to_string())
            .replace("{repo}", repo);
        args.extend(["-p".into(), prompt, "--append-system-prompt".into(), HEADLESS_NOTE.into()]);
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

    if jobs.lock().ok().and_then(|m| m.get(id).map(|j| j.info.status == "cancelled")).unwrap_or(false) {
        finish(app, jobs, id, "cancelled", None);
        return Ok(());
    }
    progress(app, jobs, id, &format!("Starting claude in {}", cwd.display()));
    let mut cmd = Command::new(&claude);
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
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v.get("type").and_then(|t| t.as_str()) == Some("result") {
                    result = Some(v);
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
    };
    let text = res.get("result").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if is_error {
        return Err(format!("claude reported an error: {text}"));
    }

    if kind == "review" {
        store::write(data_dir, repo, number, "review.md", &text)?;
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
