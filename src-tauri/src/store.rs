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
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrState {
    pub review: Option<String>,
    pub review_meta: Option<RunMeta>,
    pub insights: Option<Value>,
    pub insights_meta: Option<RunMeta>,
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
