//! Removes review worktrees that are no longer useful: the PR was merged or closed,
//! or nobody has run anything on it for a while. Reports, notes and insights live
//! under `prs/` and are never touched.

use crate::shell;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY_MS: u64 = 86_400_000;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// `owner/repo#n (reason)` for each removal.
    pub removed: Vec<String>,
    pub kept: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub count: usize,
    pub bytes: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn marker(wt: &Path) -> PathBuf {
    // Beside the worktree, not inside it, so it never shows up in the review's `git status`.
    wt.with_extension("last-used")
}

/// Called whenever a job uses a worktree.
pub fn touch(wt: &Path) {
    let _ = std::fs::write(marker(wt), now_ms().to_string());
}

fn last_used(wt: &Path) -> u64 {
    std::fs::read_to_string(marker(wt))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .or_else(|| {
            std::fs::metadata(wt)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
        })
        .unwrap_or(0)
}

/// `Attensi__impact-817` → (`Attensi/impact`, 817). GitHub owners cannot contain `_`,
/// so the first `__` is always the separator.
fn parse_name(name: &str) -> Option<(String, u64)> {
    let (repo_part, num) = name.rsplit_once('-')?;
    let number = num.parse().ok()?;
    let (owner, repo) = repo_part.split_once("__")?;
    Some((format!("{owner}/{repo}"), number))
}

/// The clone a worktree belongs to, from its `.git` file: `gitdir: <clone>/.git/worktrees/<name>`.
fn clone_of(wt: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(wt.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    Some(gitdir.parent()?.parent()?.parent()?.to_path_buf())
}

fn pr_closed(repo: &str, number: u64) -> Option<bool> {
    let out = shell::run(
        "gh",
        &["pr", "view", &number.to_string(), "-R", repo, "--json", "state", "-q", ".state"],
        None,
    )
    .ok()?;
    Some(matches!(out.trim(), "MERGED" | "CLOSED"))
}

pub fn remove(wt: &Path) {
    if let Some(clone) = clone_of(wt).filter(|c| c.exists()) {
        let wt_s = wt.to_string_lossy().to_string();
        let _ = shell::run("git", &["worktree", "remove", "--force", &wt_s], Some(&clone));
        if wt.exists() {
            let _ = std::fs::remove_dir_all(wt);
        }
        let _ = shell::run("git", &["worktree", "prune"], Some(&clone));
    } else {
        let _ = std::fs::remove_dir_all(wt);
    }
    let _ = std::fs::remove_file(marker(wt));
}

fn worktrees(data: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(data.join("worktrees"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

/// `open` holds keys of PRs known to be open (from the last poll), so they skip the
/// GitHub lookup. `busy` holds keys with a job running; those are never removed.
/// `retention_days == 0` keeps unused worktrees until their PR closes.
pub fn run(data: &Path, retention_days: u64, open: &HashSet<String>, busy: &HashSet<String>, everything: bool) -> Report {
    let mut report = Report::default();
    for wt in worktrees(data) {
        let name = wt.file_name().unwrap_or_default().to_string_lossy().to_string();
        let Some((repo, number)) = parse_name(&name) else {
            report.kept += 1;
            continue;
        };
        let key = format!("{repo}#{number}");
        if busy.contains(&key) {
            report.kept += 1;
            continue;
        }
        let idle = now_ms().saturating_sub(last_used(&wt)) > retention_days * DAY_MS;
        let reason = if everything {
            Some("removed by you")
        } else if retention_days > 0 && idle {
            Some("unused")
        } else if !open.contains(&key) && pr_closed(&repo, number) == Some(true) {
            Some("PR closed")
        } else {
            None
        };
        match reason {
            Some(r) => {
                remove(&wt);
                report.removed.push(format!("{key} ({r})"));
            }
            None => report.kept += 1,
        }
    }
    // Stray markers whose worktree is already gone.
    if let Ok(rd) = std::fs::read_dir(data.join("worktrees")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "last-used") && !p.with_extension("").exists() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    report
}

pub fn usage(data: &Path) -> Usage {
    let list = worktrees(data);
    let bytes = list
        .iter()
        .map(|p| dir_bytes(p))
        .sum::<u64>();
    Usage { count: list.len(), bytes }
}

#[cfg(unix)]
fn dir_bytes(p: &Path) -> u64 {
    shell::run("du", &["-sk", &p.to_string_lossy()], None)
        .ok()
        .and_then(|out| out.split_whitespace().next().and_then(|k| k.parse::<u64>().ok()))
        .unwrap_or(0)
        * 1024
}

/// No `du` on Windows: walk the tree, without following junctions or symlinks.
#[cfg(windows)]
fn dir_bytes(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                stack.push(e.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

pub const INTERVAL: Duration = Duration::from_secs(3600);


#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").args(args).current_dir(dir).output().unwrap().status.success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn parses_worktree_names() {
        assert_eq!(parse_name("Attensi__impact-817"), Some(("Attensi/impact".into(), 817)));
        assert_eq!(parse_name("Attensi__creator-web-776"), Some(("Attensi/creator-web".into(), 776)));
        assert_eq!(parse_name("junk"), None);
    }

    #[test]
    fn removes_idle_and_keeps_busy_and_fresh() {
        let root = std::env::temp_dir().join(format!("prr-cleanup-{}", now_ms()));
        let clone = root.join("clone");
        let data = root.join("data");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q"]);
        git(&clone, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "x"]);

        let mk = |name: &str, age_days: u64| {
            let wt = data.join("worktrees").join(name);
            std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
            git(&clone, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "HEAD"]);
            std::fs::write(marker(&wt), (now_ms() - age_days * DAY_MS).to_string()).unwrap();
            wt
        };
        let idle = mk("o__r-1", 10);
        let fresh = mk("o__r-2", 1);
        let busy = mk("o__r-3", 10);

        assert_eq!(clone_of(&idle).unwrap().canonicalize().unwrap(), clone.canonicalize().unwrap());

        // All three count as open, so no GitHub lookups happen.
        let open: HashSet<String> = ["o/r#1", "o/r#2", "o/r#3"].map(String::from).into();
        let busy_set: HashSet<String> = ["o/r#3".to_string()].into();
        let r = run(&data, 7, &open, &busy_set, false);

        assert_eq!(r.removed, vec!["o/r#1 (unused)".to_string()]);
        assert_eq!(r.kept, 2);
        assert!(!idle.exists() && !marker(&idle).exists());
        assert!(fresh.exists() && busy.exists());
        // git forgot the removed worktree too.
        let list = Command::new("git").args(["worktree", "list"]).current_dir(&clone).output().unwrap();
        assert!(!String::from_utf8_lossy(&list.stdout).contains("o__r-1"));

        // 0 days = keep idle ones while the PR is open.
        assert!(run(&data, 0, &open, &busy_set, false).removed.is_empty());

        let all = run(&data, 7, &open, &busy_set, true);
        assert_eq!(all.removed, vec!["o/r#2 (removed by you)".to_string()]);
        assert!(busy.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
