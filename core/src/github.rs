use crate::settings::Settings;
use crate::shell;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const SEARCH_QUERY: &str = r#"
query($q: String!) {
  viewer { login }
  search(query: $q, type: ISSUE, first: 50) {
    nodes {
      ... on PullRequest {
        number title url isDraft createdAt updatedAt
        additions deletions changedFiles
        headRefOid headRefName baseRefName reviewDecision mergeStateStatus
        author { login avatarUrl }
        repository { nameWithOwner }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
        reviews(last: 30) { nodes { author { login } state submittedAt commit { oid } } }
        latestReviews(first: 20) { nodes { author { login avatarUrl } state } }
        reviewRequests(first: 20) {
          nodes {
            requestedReviewer {
              __typename
              ... on User { login avatarUrl }
              ... on Team { name slug organization { login } }
              ... on Bot { login avatarUrl }
              ... on Mannequin { login avatarUrl }
            }
          }
        }
      }
    }
  }
}"#;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: String,
    pub state: String,
    pub submitted_at: Option<String>,
    pub commit: Option<String>,
}

/// Someone asked to review who has not answered yet: a person, or a whole team.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestedReviewer {
    pub name: String,
    pub is_team: bool,
    pub avatar: String,
}

/// Each reviewer's most recent verdict.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestReview {
    pub author: String,
    pub avatar: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub key: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub author_avatar: String,
    pub is_draft: bool,
    pub created_at: String,
    pub updated_at: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub review_decision: Option<String>,
    /// GitHub's mergeability: CLEAN, BLOCKED, BEHIND, DIRTY (conflicts), UNSTABLE, UNKNOWN…
    #[serde(default)]
    pub merge_state: Option<String>,
    pub ci: Option<String>,
    pub reviews: Vec<Review>,
    pub requested_reviewers: Vec<RequestedReviewer>,
    pub latest_reviews: Vec<LatestReview>,
    /// reviewRequested | reReview | watched | mine
    pub categories: Vec<String>,
    /// I reviewed an older commit than the current head.
    pub needs_re_review: bool,
    /// Waiting on me: a review request, or my PR is approved (merge it), has changes
    /// requested, or fails CI.
    pub needs_action: bool,
    /// Hidden by the user; set by `ignore::apply` after each fetch.
    #[serde(default)]
    pub ignored: bool,
    /// Requested from one of my teams but not from me by name.
    #[serde(default)]
    pub team_only: bool,
    /// Teams asked to review, as `org/slug`.
    #[serde(default)]
    pub via_teams: Vec<String>,
    /// I approved it in the app recently; kept under "Review requested" for the grace period.
    #[serde(default)]
    pub just_approved: bool,
    /// MERGED or CLOSED: it left the open lists since the last fetch. Kept briefly so the
    /// change is visible instead of the row just vanishing.
    #[serde(default)]
    pub closed_state: Option<String>,
}

/// `reviewDecision` is null in repos without required reviews, so fall back to the
/// latest reviews. Returns APPROVED, CHANGES_REQUESTED or REVIEW_REQUIRED.
pub fn approval(p: &Pr) -> &str {
    if let Some(d) = p.review_decision.as_deref() {
        return d;
    }
    let states: Vec<&str> = p.latest_reviews.iter().map(|r| r.state.as_str()).collect();
    if states.contains(&"CHANGES_REQUESTED") {
        "CHANGES_REQUESTED"
    } else if states.contains(&"APPROVED") {
        "APPROVED"
    } else {
        "REVIEW_REQUIRED"
    }
}

pub fn ci_failed(p: &Pr) -> bool {
    matches!(p.ci.as_deref(), Some("FAILURE") | Some("ERROR"))
}

/// Approved, checks green, and GitHub would merge it now (no conflicts, not behind or blocked).
/// UNKNOWN (GitHub hasn't computed it yet) counts as mergeable when the rest is fine.
pub fn ready_to_merge(p: &Pr) -> bool {
    approval(p) == "APPROVED"
        && matches!(p.ci.as_deref(), None | Some("SUCCESS"))
        && !matches!(p.merge_state.as_deref(), Some("DIRTY" | "BLOCKED" | "BEHIND" | "DRAFT"))
}

pub fn has_conflicts(p: &Pr) -> bool {
    p.merge_state.as_deref() == Some("DIRTY")
}

fn needs_action(p: &Pr) -> bool {
    let has = |c: &str| p.categories.iter().any(|x| x == c);
    has("reviewRequested")
        || has("reReview")
        || (has("mine") && (approval(p) != "REVIEW_REQUIRED" || ci_failed(p) || has_conflicts(p)))
}

fn s(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

fn search(q: &str) -> Result<(String, Vec<Value>), String> {
    let out = shell::run(
        "gh",
        &["api", "graphql", "-f", &format!("query={SEARCH_QUERY}"), "-F", &format!("q={q}")],
        None,
    )?;
    let v: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    let login = s(&v, "/data/viewer/login");
    let nodes = v
        .pointer("/data/search/nodes")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    Ok((login, nodes))
}

fn to_pr(n: &Value, me: &str) -> Option<Pr> {
    let number = n.get("number")?.as_u64()?;
    let repo = s(n, "/repository/nameWithOwner");
    let reviews: Vec<Review> = n
        .pointer("/reviews/nodes")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| Review {
                    author: s(r, "/author/login"),
                    state: s(r, "/state"),
                    submitted_at: r.get("submittedAt").and_then(|x| x.as_str()).map(String::from),
                    commit: r.pointer("/commit/oid").and_then(|x| x.as_str()).map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();
    let nodes = |ptr: &str| n.pointer(ptr).and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let requested_reviewers = nodes("/reviewRequests/nodes")
        .iter()
        .filter_map(|r| {
            let rr = r.get("requestedReviewer")?;
            if rr.get("__typename")?.as_str()? == "Team" {
                Some(RequestedReviewer {
                    name: format!("{}/{}", s(rr, "/organization/login"), s(rr, "/slug")),
                    is_team: true,
                    avatar: String::new(),
                })
            } else {
                Some(RequestedReviewer { name: s(rr, "/login"), is_team: false, avatar: s(rr, "/avatarUrl") })
            }
        })
        .collect();
    let latest_reviews = nodes("/latestReviews/nodes")
        .iter()
        .map(|r| LatestReview { author: s(r, "/author/login"), avatar: s(r, "/author/avatarUrl"), state: s(r, "/state") })
        .collect();
    let head_sha = s(n, "/headRefOid");
    let my_last = reviews.iter().rev().find(|r| r.author == me && r.state != "PENDING");
    let needs_re_review = my_last
        .and_then(|r| r.commit.as_ref())
        .map(|c| *c != head_sha)
        .unwrap_or(false);
    Some(Pr {
        key: format!("{repo}#{number}"),
        repo,
        number,
        title: s(n, "/title"),
        url: s(n, "/url"),
        author: s(n, "/author/login"),
        author_avatar: s(n, "/author/avatarUrl"),
        is_draft: n.get("isDraft").and_then(|x| x.as_bool()).unwrap_or(false),
        created_at: s(n, "/createdAt"),
        updated_at: s(n, "/updatedAt"),
        additions: n.get("additions").and_then(|x| x.as_u64()).unwrap_or(0),
        deletions: n.get("deletions").and_then(|x| x.as_u64()).unwrap_or(0),
        changed_files: n.get("changedFiles").and_then(|x| x.as_u64()).unwrap_or(0),
        head_sha,
        head_ref: s(n, "/headRefName"),
        base_ref: s(n, "/baseRefName"),
        review_decision: n.get("reviewDecision").and_then(|x| x.as_str()).map(String::from),
        merge_state: n.get("mergeStateStatus").and_then(|x| x.as_str()).map(String::from),
        ci: n
            .pointer("/commits/nodes/0/commit/statusCheckRollup/state")
            .and_then(|x| x.as_str())
            .map(String::from),
        reviews,
        requested_reviewers,
        latest_reviews,
        categories: vec![],
        needs_re_review,
        needs_action: false,
        just_approved: false,
        closed_state: None,
        ignored: false,
        team_only: false,
        via_teams: vec![],
    })
}

pub struct FetchResult {
    pub me: String,
    pub prs: Vec<Pr>,
    pub errors: Vec<String>,
}

/// `grace`: keys of PRs I approved recently, kept in the list even though no longer requested.
pub fn fetch_all(settings: &Settings, grace: &[String]) -> FetchResult {
    let base = "is:pr is:open archived:false";
    let mut queries: Vec<(&str, String)> = vec![];
    let c = &settings.categories;
    if c.review_requested {
        // All requests (direct and via teams), then direct-only, to tell them apart.
        queries.push(("reviewRequested", format!("{base} review-requested:@me")));
        queries.push(("directRequest", format!("{base} user-review-requested:@me")));
    }
    if c.re_review {
        queries.push(("reReview", format!("{base} reviewed-by:@me -author:@me")));
    }
    if c.watched && !settings.watched_repos.is_empty() {
        let repos: Vec<String> = settings
            .watched_repos
            .iter()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .map(|r| format!("repo:{r}"))
            .collect();
        if !repos.is_empty() {
            queries.push(("watched", format!("{base} {}", repos.join(" "))));
        }
    }
    if c.mine {
        queries.push(("mine", format!("{base} author:@me")));
    }

    // Approved PRs leave `review-requested:@me`; this brings the recent ones back.
    let want_grace = c.review_requested && !grace.is_empty();
    if want_grace {
        queries.push(("approved", format!("{base} reviewed-by:@me -author:@me")));
    }

    // Each search is a `gh` round trip of a second or more: run them side by side.
    let results: Vec<(&str, Result<(String, Vec<Value>), String>)> = std::thread::scope(|sc| {
        let handles: Vec<_> = queries.iter().map(|(cat, q)| (*cat, sc.spawn(move || search(q)))).collect();
        handles.into_iter().map(|(cat, h)| (cat, h.join().unwrap_or_else(|_| Err("search thread panicked".into())))).collect()
    });

    let mut me = String::new();
    let mut by_key: BTreeMap<String, Pr> = BTreeMap::new();
    let mut errors = vec![];
    let mut approved_nodes: Vec<Value> = vec![];
    for (cat, r) in results {
        match r {
            Ok((login, nodes)) => {
                me = login;
                if cat == "approved" {
                    approved_nodes = nodes;
                    continue;
                }
                for n in nodes {
                    let Some(pr) = to_pr(&n, &me) else { continue };
                    // Only PRs whose head moved since my review count as re-review.
                    if cat == "reReview" && !pr.needs_re_review {
                        continue;
                    }
                    by_key
                        .entry(pr.key.clone())
                        .or_insert(pr)
                        .categories
                        .push(cat.to_string());
                }
            }
            Err(e) => errors.push(format!("{cat}: {e}")),
        }
    }
    if want_grace {
        for n in approved_nodes {
            let Some(pr) = to_pr(&n, &me) else { continue };
            if !grace.contains(&pr.key) {
                continue;
            }
            let p = by_key.entry(pr.key.clone()).or_insert(pr);
            if !p.categories.iter().any(|c| c == "reviewRequested") {
                p.categories.push("reviewRequested".into());
                // Counts as direct: it was asked of me, so the team filter mustn't hide it.
                p.categories.push("directRequest".into());
            }
        }
        // Search lags GitHub by a minute or so: a PR can still look requested right after
        // approving. Either way, it's one I just approved.
        for k in grace {
            if let Some(p) = by_key.get_mut(k) {
                p.just_approved = true;
            }
        }
    }
    let direct_ok = !errors.iter().any(|e| e.starts_with("directRequest"));
    let mut prs: Vec<Pr> = by_key.into_values().filter(|p| p.categories.iter().any(|c| c != "directRequest")).collect();
    for p in &mut prs {
        let direct = p.categories.iter().any(|c| c == "directRequest");
        p.categories.retain(|c| c != "directRequest");
        // Unknown if the direct search failed: never hide on a guess.
        p.team_only = direct_ok && !direct && p.categories.iter().any(|c| c == "reviewRequested");
        p.via_teams = p.requested_reviewers.iter().filter(|r| r.is_team).map(|r| r.name.clone()).collect();
        p.needs_action = needs_action(p);
    }
    prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    FetchResult { me, prs, errors }
}

/// OPEN, MERGED or CLOSED.
pub fn pr_state(repo: &str, number: u64) -> Option<String> {
    let out = shell::run("gh", &["pr", "view", &number.to_string(), "-R", repo, "--json", "state", "--jq", ".state"], None).ok()?;
    let s = out.trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub fn pr_detail(repo: &str, number: u64) -> Result<Value, String> {
    let out = shell::run(
        "gh",
        &[
            "pr", "view", &number.to_string(), "-R", repo, "--json",
            "number,title,body,url,state,isDraft,author,headRefName,baseRefName,headRefOid,\
             additions,deletions,changedFiles,files,commits,reviews,comments,labels,\
             statusCheckRollup,reviewDecision,createdAt,updatedAt,mergeable",
        ],
        None,
    )?;
    serde_json::from_str(&out).map_err(|e| e.to_string())
}

pub fn pr_diff(repo: &str, number: u64) -> Result<String, String> {
    shell::run("gh", &["pr", "diff", &number.to_string(), "-R", repo, "--color=never"], None)
}

const MERGE_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    viewerPermission squashMergeAllowed mergeCommitAllowed rebaseMergeAllowed
    deleteBranchOnMerge autoMergeAllowed
    pullRequest(number: $number) {
      state isDraft mergeable mergeStateStatus headRefOid baseRefName
      viewerCanMergeAsAdmin viewerCanEnableAutoMerge
      autoMergeRequest { enabledAt mergeMethod }
    }
  }
}"#;

/// Everything the merge box needs: allowed methods, whether it can merge now, and
/// whether the viewer may bypass branch protection.
pub fn merge_info(repo: &str, number: u64) -> Result<Value, String> {
    let (owner, name) = repo.split_once('/').ok_or("bad repo")?;
    let out = shell::run(
        "gh",
        &[
            "api", "graphql",
            "-f", &format!("query={MERGE_QUERY}"),
            "-f", &format!("owner={owner}"),
            "-f", &format!("name={name}"),
            "-F", &format!("number={number}"),
        ],
        None,
    )?;
    let v: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    let mut repo_v = v.pointer("/data/repository").cloned().ok_or("no repository in response")?;
    let base = repo_v.pointer("/pullRequest/baseRefName").and_then(|b| b.as_str()).unwrap_or_default().to_string();
    let bypass = can_bypass_rulesets(repo, &base);
    if let Some(pr) = repo_v.get_mut("pullRequest").and_then(|p| p.as_object_mut()) {
        pr.insert("viewerCanBypassRulesets".into(), Value::Bool(bypass));
    }
    Ok(repo_v)
}

/// `viewerCanMergeAsAdmin` only reflects classic branch protection. Repos on
/// rulesets say who may bypass per ruleset; the viewer can override a PR merge when
/// every ruleset on the base branch lets them bypass for pull requests.
fn can_bypass_rulesets(repo: &str, base: &str) -> bool {
    if base.is_empty() {
        return false;
    }
    let Ok(out) = shell::run("gh", &["api", &format!("repos/{repo}/rules/branches/{base}"), "--jq", "[.[].ruleset_id] | unique | .[]"], None) else {
        return false;
    };
    let ids: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    !ids.is_empty()
        && ids.iter().all(|id| {
            shell::run("gh", &["api", &format!("repos/{repo}/rulesets/{id}"), "--jq", ".current_user_can_bypass"], None)
                .map(|b| matches!(b.trim(), "always" | "pull_requests_only"))
                .unwrap_or(false)
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequest {
    pub repo: String,
    pub number: u64,
    /// approve | request_changes | comment
    pub event: String,
    pub body: String,
}

pub fn submit_review(r: &ReviewRequest) -> Result<String, String> {
    let n = r.number.to_string();
    let flag = match r.event.as_str() {
        "approve" => "--approve",
        "request_changes" => "--request-changes",
        "comment" => "--comment",
        e => return Err(format!("unknown review event {e}")),
    };
    let mut args = vec!["pr", "review", &n, "-R", &r.repo, flag];
    if !r.body.trim().is_empty() {
        args.extend(["--body", r.body.as_str()]);
    }
    shell::run("gh", &args, None)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    pub repo: String,
    pub number: u64,
    /// squash | merge | rebase
    pub method: String,
    pub admin: bool,
    pub auto: bool,
    pub delete_branch: bool,
    /// Refuse to merge if the head moved since the user looked at it.
    pub head_sha: String,
    pub subject: String,
    pub body: String,
}

pub fn merge(r: &MergeRequest) -> Result<String, String> {
    let n = r.number.to_string();
    let method = match r.method.as_str() {
        "squash" => "--squash",
        "rebase" => "--rebase",
        "merge" => "--merge",
        m => return Err(format!("unknown merge method {m}")),
    };
    let mut args = vec!["pr", "merge", &n, "-R", &r.repo, method, "--match-head-commit", &r.head_sha];
    if r.admin {
        args.push("--admin");
    } else if r.auto {
        args.push("--auto");
    }
    if r.delete_branch {
        args.push("--delete-branch");
    }
    // Rebase merges have no merge commit to title.
    if r.method != "rebase" && !r.subject.trim().is_empty() {
        args.extend(["--subject", r.subject.as_str()]);
    }
    if r.method != "rebase" {
        args.extend(["--body", r.body.as_str()]);
    }
    // No cwd: with -R, gh leaves every local checkout alone.
    shell::run("gh", &args, None)
}
