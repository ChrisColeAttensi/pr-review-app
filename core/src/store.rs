//! Per-PR files on disk: the review report, the insights, and local draft comments.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn pr_dir(data: &Path, repo: &str, number: u64) -> PathBuf {
    data.join("prs").join(format!("{}-{number}", repo.replace('/', "__")))
}

pub fn worktree_dir(data: &Path, repo: &str, number: u64) -> PathBuf {
    data.join("worktrees").join(format!("{}-{number}", repo.replace('/', "__")))
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunMeta {
    pub head_sha: String,
    pub finished_at: u64,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub ok: bool,
    /// PR size and model at run time, for estimating future runs.
    #[serde(default)]
    pub lines: Option<u64>,
    #[serde(default)]
    pub files: Option<u64>,
    #[serde(default)]
    pub model: Option<String>,
    /// The claude session and the directory it ran in, so it can be resumed.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// The model claude actually ran, e.g. `claude-opus-5-5` (the setting may be blank).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrState {
    pub review: Option<String>,
    pub review_meta: Option<RunMeta>,
    pub insights: Option<Value>,
    pub insights_meta: Option<RunMeta>,
    pub chat_meta: Option<RunMeta>,
    pub drafts: Value,
}

fn read_json<T: for<'de> Deserialize<'de>>(p: PathBuf) -> Option<T> {
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok())
}

pub fn load(data: &Path, repo: &str, number: u64) -> PrState {
    let dir = pr_dir(data, repo, number);
    PrState {
        review: std::fs::read_to_string(dir.join("review.md")).ok(),
        review_meta: read_json(dir.join("review.meta.json")),
        insights: read_json(dir.join("insights.json")),
        insights_meta: read_json(dir.join("insights.meta.json")),
        chat_meta: read_json(dir.join("chat.meta.json")),
        drafts: read_json(dir.join("drafts.json")).unwrap_or(Value::Array(vec![])),
    }
}

pub fn write(data: &Path, repo: &str, number: u64, name: &str, body: &str) -> Result<(), String> {
    let dir = pr_dir(data, repo, number);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(name), body).map_err(|e| e.to_string())
}

/// One line per finished run, used to estimate how long the next one will take.
pub fn append_history(data: &Path, kind: &str, meta: &RunMeta) {
    use std::io::Write;
    let line = serde_json::json!({
        "kind": kind,
        "lines": meta.lines,
        "files": meta.files,
        "model": meta.model,
        "durationMs": meta.duration_ms,
        "costUsd": meta.cost_usd,
        "finishedAt": meta.finished_at,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data.join("history.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}

pub fn history(data: &Path) -> Vec<Value> {
    std::fs::read_to_string(data.join("history.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// user | assistant | error
    pub role: String,
    pub text: String,
    pub ts: u64,
}

pub fn chat(data: &Path, repo: &str, number: u64) -> Vec<ChatMessage> {
    read_json(pr_dir(data, repo, number).join("chat.json")).unwrap_or_default()
}

pub fn push_chat(data: &Path, repo: &str, number: u64, msg: ChatMessage) {
    let mut all = chat(data, repo, number);
    all.push(msg);
    let _ = write(data, repo, number, "chat.json", &serde_json::to_string_pretty(&all).unwrap_or_default());
}

/// The conversation to continue: the chat's own, else the pr-review run's, else Explain's.
/// Empties the Claude tab: the chat, the pr-review report, and which session to resume.
/// Explain's walkthrough stays, but its session is no longer resumed. Worktree edits stay too.
pub fn clear_thread(data: &Path, repo: &str, number: u64) -> Result<(), String> {
    let dir = pr_dir(data, repo, number);
    for f in ["chat.json", "chat.meta.json", "review.md", "review.meta.json"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("thread-cleared-at"), crate::now_ms().to_string()).map_err(|e| e.to_string())
}

pub fn resumable(data: &Path, repo: &str, number: u64) -> Option<(String, String)> {
    let dir = pr_dir(data, repo, number);
    let cleared: u64 = std::fs::read_to_string(dir.join("thread-cleared-at")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    // The newest conversation: a pr-review re-run after chatting is what the chat continues from.
    ["chat.meta.json", "review.meta.json", "insights.meta.json"]
        .iter()
        .filter_map(|f| read_json::<RunMeta>(dir.join(f)))
        .filter(|m| m.finished_at > cleared)
        .filter_map(|m| Some((m.finished_at, m.session_id?, m.cwd?)))
        .max_by_key(|(t, _, _)| *t)
        .map(|(_, sid, cwd)| (sid, cwd))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    pub review_head: Option<String>,
    /// First heading of the report, e.g. "✅ Approve — …".
    pub verdict: Option<String>,
    pub insights_head: Option<String>,
    pub drafts: usize,
    pub chat: usize,
}

/// Per-PR summary of what has been done locally, keyed `owner/repo#n`.
pub fn index(data: &Path) -> std::collections::HashMap<String, IndexEntry> {
    let mut out = std::collections::HashMap::new();
    let Ok(rd) = std::fs::read_dir(data.join("prs")) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some((repo_part, num)) = name.rsplit_once('-') else { continue };
        let Some((owner, repo)) = repo_part.split_once("__") else { continue };
        let Ok(number) = num.parse::<u64>() else { continue };
        let repo = format!("{owner}/{repo}");
        let st = load(data, &repo, number);
        let verdict = st.review.as_deref().and_then(|r| {
            r.lines().find(|l| l.trim_start().starts_with('#')).map(|l| l.trim_start_matches('#').trim().to_string())
        });
        out.insert(
            format!("{repo}#{number}"),
            IndexEntry {
                review_head: st.review_meta.map(|m| m.head_sha),
                verdict,
                insights_head: st.insights_meta.map(|m| m.head_sha),
                drafts: st.drafts.as_array().map(|a| a.len()).unwrap_or(0),
                chat: chat(data, &repo, number).len(),
            },
        );
    }
    out
}
