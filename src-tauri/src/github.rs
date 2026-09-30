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
        headRefOid headRefName baseRefName reviewDecision
        author { login avatarUrl }
        repository { nameWithOwner }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
        reviews(last: 30) { nodes { author { login } state submittedAt commit { oid } } }
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
    pub ci: Option<String>,
    pub reviews: Vec<Review>,
    /// reviewRequested | reReview | watched | mine
    pub categories: Vec<String>,
    /// I reviewed an older commit than the current head.
    pub needs_re_review: bool,
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
        ci: n
            .pointer("/commits/nodes/0/commit/statusCheckRollup/state")
            .and_then(|x| x.as_str())
            .map(String::from),
        reviews,
        categories: vec![],
        needs_re_review,
    })
}

pub struct FetchResult {
    pub me: String,
    pub prs: Vec<Pr>,
    pub errors: Vec<String>,
}

pub fn fetch_all(settings: &Settings) -> FetchResult {
    let base = "is:pr is:open archived:false";
    let mut queries: Vec<(&str, String)> = vec![];
    let c = &settings.categories;
    if c.review_requested {
        let q = if settings.direct_requests_only { "user-review-requested:@me" } else { "review-requested:@me" };
        queries.push(("reviewRequested", format!("{base} {q}")));
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

    let mut me = String::new();
    let mut by_key: BTreeMap<String, Pr> = BTreeMap::new();
    let mut errors = vec![];
    for (cat, q) in queries {
        match search(&q) {
            Ok((login, nodes)) => {
                me = login;
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
    let mut prs: Vec<Pr> = by_key.into_values().collect();
    prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    FetchResult { me, prs, errors }
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
