//! Decides what is worth a desktop notification, by comparing each fetch with what the
//! user was last told about. The app that polls shows them.

use crate::github::Pr;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What we last told the user about each PR, so we only notify on change.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seen {
    head: String,
    ci: Option<String>,
    reviews: usize,
    categories: Vec<String>,
    #[serde(default)]
    draft: bool,
}

pub struct Note {
    pub title: String,
    pub body: String,
    /// The PR it is about, `owner/repo#n`.
    pub key: String,
}

pub struct Notifier {
    path: PathBuf,
    seen: HashMap<String, Seen>,
    /// No notifications for what is already there the first time we ever look.
    first_run: bool,
}

impl Notifier {
    pub fn load(data: &Path) -> Self {
        let path = data.join("seen.json");
        let first_run = !path.exists();
        let seen = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Notifier { path, seen, first_run }
    }

    /// Returns the notifications to show, and the PRs newly requested from me.
    pub fn diff(&mut self, s: &Settings, me: &str, prs: &[Pr]) -> (Vec<Note>, Vec<String>) {
        let nc = &s.notify_categories;
        let mut notes = vec![];
        let mut new_requests = vec![];
        let first_run = self.first_run;
        for pr in prs.iter().filter(|p| p.closed_state.is_none()) {
            let has = |c: &str| pr.categories.iter().any(|x| x == c);
            let reviews_by_others = pr.reviews.iter().filter(|r| r.author != me).count();
            let prev = self.seen.get(&pr.key).cloned();
            self.seen.insert(
                pr.key.clone(),
                Seen { head: pr.head_sha.clone(), ci: pr.ci.clone(), reviews: reviews_by_others, categories: pr.categories.clone(), draft: pr.is_draft },
            );
            if pr.ignored || (pr.team_only && s.direct_requests_only) || pr.just_approved {
                continue;
            }
            if first_run || (pr.is_draft && !s.show_drafts) {
                continue;
            }
            // A hidden draft that turns ready counts as new.
            let was_hidden = prev.as_ref().is_some_and(|p| p.draft && !s.show_drafts);
            let was = |c: &str| !was_hidden && prev.as_ref().map(|p| p.categories.iter().any(|x| x == c)).unwrap_or(false);
            let label = format!("{}#{}", pr.repo, pr.number);
            let mut note = |title: String, body: String| notes.push(Note { title, body, key: pr.key.clone() });

            if has("reviewRequested") && !was("reviewRequested") {
                new_requests.push(pr.key.clone());
                if nc.review_requested {
                    note(format!("Review requested · {label}"), format!("{} — by {}", pr.title, pr.author));
                }
                continue;
            }
            if has("reReview") && nc.re_review {
                if let Some(p) = &prev {
                    if p.head != pr.head_sha && !p.head.is_empty() {
                        note(format!("New commits since your review · {label}"), pr.title.clone());
                        continue;
                    }
                }
            }
            if has("watched") && prev.is_none() && nc.watched && pr.author != me {
                note(format!("New PR · {label}"), format!("{} — by {}", pr.title, pr.author));
                continue;
            }
            if has("mine") && nc.mine {
                if let Some(p) = &prev {
                    if p.ci != pr.ci {
                        match pr.ci.as_deref() {
                            Some("SUCCESS") => note(format!("CI passed · {label}"), pr.title.clone()),
                            Some("FAILURE") | Some("ERROR") => note(format!("CI failed · {label}"), pr.title.clone()),
                            _ => {}
                        }
                    }
                    if reviews_by_others > p.reviews {
                        if let Some(r) = pr.reviews.iter().rev().find(|r| r.author != me) {
                            let verb = match r.state.as_str() {
                                "APPROVED" => "approved",
                                "CHANGES_REQUESTED" => "requested changes on",
                                _ => "commented on",
                            };
                            note(format!("{} {verb} your PR", r.author), format!("{label} · {}", pr.title));
                        }
                    }
                }
            }
        }
        self.first_run = false;
        let _ = std::fs::write(&self.path, serde_json::to_string(&self.seen).unwrap_or_default());
        if !s.notifications {
            notes.clear();
        }
        (notes, new_requests)
    }
}
