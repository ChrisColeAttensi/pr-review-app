//! Maps `owner/repo` to a local clone, so reviews run with the real codebase at hand.

use crate::settings::{expand_tilde, Settings};
use crate::shell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn parse_remote(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    Some(format!("{owner}/{name}").to_lowercase())
}

fn origin_of(dir: &Path) -> Option<String> {
    // Read .git/config directly: far cheaper than a git process per folder.
    let cfg = std::fs::read_to_string(dir.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in cfg.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_origin = t == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(url) = t.strip_prefix("url =").or_else(|| t.strip_prefix("url=")) {
                return parse_remote(url);
            }
        }
    }
    None
}

/// Scan roots two levels deep. Worktrees (`.git` is a file) are skipped.
pub fn discover(settings: &Settings) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for root in &settings.repo_roots {
        let root = expand_tilde(root);
        let mut stack: Vec<(PathBuf, u8)> = vec![(root, 0)];
        while let Some((dir, depth)) = stack.pop() {
            if dir.join(".git").is_dir() {
                if let Some(repo) = origin_of(&dir) {
                    found.entry(repo).or_insert_with(|| dir.to_string_lossy().to_string());
                }
                continue;
            }
            if depth >= 2 {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    let name = e.file_name().to_string_lossy().to_string();
                    if p.is_dir() && !name.starts_with('.') && name != "node_modules" {
                        stack.push((p, depth + 1));
                    }
                }
            }
        }
    }
    found
}

pub fn resolve(settings: &Settings, repo: &str) -> Option<PathBuf> {
    if let Some(p) = settings.repo_paths.get(repo) {
        let p = expand_tilde(p);
        if p.exists() {
            return Some(p);
        }
    }
    discover(settings).get(&repo.to_lowercase()).map(PathBuf::from)
}

static CLONE_LOCKS: std::sync::Mutex<Vec<(PathBuf, std::sync::Arc<std::sync::Mutex<()>>)>> = std::sync::Mutex::new(Vec::new());

fn clone_lock(clone: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    let mut locks = CLONE_LOCKS.lock().unwrap();
    if let Some((_, l)) = locks.iter().find(|(p, _)| p == clone) {
        return l.clone();
    }
    let l = std::sync::Arc::new(std::sync::Mutex::new(()));
    locks.push((clone.to_path_buf(), l.clone()));
    l
}

/// Check out the PR head in a worktree of its own, so the user's checkout is never touched.
/// Each PR fetches into its own ref (never the shared FETCH_HEAD), and git setup on one
/// clone runs one at a time, so concurrent reviews can't check out each other's code.
pub fn prepare_worktree(clone: &Path, wt: &Path, number: u64, base: &str) -> Result<(), String> {
    let lock = clone_lock(clone);
    let _one_at_a_time = lock.lock().unwrap();
    let local = format!("refs/pr-review/{number}");
    let refspec = format!("+pull/{number}/head:{local}");
    if wt.join(".git").exists() {
        // Edits from a chat would be lost by the forced checkout: stash them, recoverably.
        let dirty = shell::run("git", &["status", "--porcelain"], Some(wt)).map(|s| !s.trim().is_empty()).unwrap_or(false);
        if dirty {
            shell::run("git", &["stash", "push", "--include-untracked", "--quiet", "-m", "pr-review-app: before re-checkout"], Some(wt))?;
        }
        shell::run("git", &["fetch", "--quiet", "origin", &refspec], Some(wt))?;
        shell::run("git", &["checkout", "--quiet", "--detach", "--force", &local], Some(wt))?;
    } else {
        if let Some(parent) = wt.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // A stale registration from a deleted folder blocks `worktree add`.
        let _ = shell::run("git", &["worktree", "prune"], Some(clone));
        shell::run("git", &["fetch", "--quiet", "origin", &refspec], Some(clone))?;
        let wt_s = wt.to_string_lossy().to_string();
        shell::run("git", &["worktree", "add", "--detach", &wt_s, &local], Some(clone))?;
    }
    if !base.is_empty() {
        let _ = shell::run("git", &["fetch", "--quiet", "origin", base], Some(wt));
    }
    Ok(())
}
