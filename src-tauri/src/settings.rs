use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Categories {
    /// Someone asked me (or my team) to review.
    pub review_requested: bool,
    /// A PR I already reviewed got new commits.
    pub re_review: bool,
    /// Any open PR in `watched_repos`.
    pub watched: bool,
    /// My own PRs: CI results and incoming reviews.
    pub mine: bool,
}

impl Default for Categories {
    fn default() -> Self {
        Self { review_requested: true, re_review: false, watched: true, mine: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub poll_interval_secs: u64,
    pub notifications: bool,
    /// Which categories are fetched and shown.
    pub categories: Categories,
    /// Which categories raise a desktop notification.
    pub notify_categories: Categories,
    /// Hide PRs that are not waiting on me.
    pub actionable_only: bool,
    pub show_drafts: bool,
    /// Hide review requests that reach me only through a team.
    pub direct_requests_only: bool,
    pub watched_repos: Vec<String>,
    /// Folders scanned (two levels deep) for local clones.
    pub repo_roots: Vec<String>,
    /// Manual `owner/repo` -> local path overrides.
    pub repo_paths: HashMap<String, String>,
    /// Empty means "find `claude` on PATH".
    pub claude_path: String,
    /// `{url}`, `{number}` and `{repo}` are substituted.
    pub review_command: String,
    pub permission_mode: String,
    pub review_model: String,
    pub insights_model: String,
    /// Default for "Live check in Chrome" on pr-review runs (passes `--chrome`).
    pub live_check_chrome: bool,
    /// Run the insights pass automatically when a review is requested from me.
    pub auto_insights: bool,
    /// Offer to DM the author on Slack after approving / merging their PR.
    pub slack_on_approve: bool,
    pub slack_on_merge: bool,
    /// {first}, {name}, {title}, {url}, {repo}, {number} are filled in.
    pub slack_approve_template: String,
    pub slack_merge_template: String,
    /// Remove review worktrees unused for this many days (0 = only when the PR closes).
    pub worktree_retention_days: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 120,
            notifications: true,
            categories: Categories::default(),
            notify_categories: Categories::default(),
            actionable_only: true,
            show_drafts: false,
            direct_requests_only: false,
            watched_repos: vec![],
            repo_roots: vec!["~/Developer".into()],
            repo_paths: HashMap::new(),
            claude_path: String::new(),
            review_command: "/chriscole-skills:pr-review {url}".into(),
            permission_mode: "auto".into(),
            review_model: String::new(),
            insights_model: String::new(),
            auto_insights: false,
            live_check_chrome: false,
            worktree_retention_days: 7,
            slack_on_approve: true,
            slack_on_merge: true,
            slack_approve_template: "Hey {first}, I've approved your PR [{repo}#{number}: {title}]({url}) 👍".into(),
            slack_merge_template: "Hey {first}, I've merged your PR [{repo}#{number}: {title}]({url}) 🚀".into(),
        }
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

pub fn load(path: &PathBuf) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &PathBuf, s: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}
