//! One round of fetching: GitHub search, then the local layers on top (ignored PRs, the
//! approval grace period, and PRs that just merged or closed).

use crate::github::{self, Pr};
use crate::settings::Settings;
use crate::sync::Snapshot;
use crate::{grace, ignore, now_ms};
use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub struct Poller {
    /// PRs that left the open lists, by key: (last known PR with `closed_state` set, when).
    gone: HashMap<String, (Pr, u64)>,
}

impl Poller {
    /// `prev` is the last snapshot, to spot PRs that disappeared.
    pub fn cycle(&mut self, data: &Path, s: &Settings, prev: &Snapshot) -> Snapshot {
        let grace = if s.approved_grace_minutes > 0 { grace::active(data, s.approved_grace_minutes) } else { vec![] };
        let mut res = github::fetch_all(s, &grace);
        ignore::apply(data, &mut res.prs);

        // A PR that vanished was merged or closed (or I was un-requested). Ask which, and
        // keep merged/closed ones on show for the grace period.
        let window = s.approved_grace_minutes * 60_000;
        let now = now_ms();
        if res.errors.is_empty() && window > 0 {
            for p in prev.prs.iter().filter(|p| p.closed_state.is_none()) {
                if res.prs.iter().any(|x| x.key == p.key) || self.gone.contains_key(&p.key) {
                    continue;
                }
                if let Some(st) = github::pr_state(&p.repo, p.number).filter(|st| st != "OPEN") {
                    let mut p = p.clone();
                    p.closed_state = Some(st);
                    p.needs_action = false;
                    p.just_approved = false;
                    self.gone.insert(p.key.clone(), (p, now));
                }
            }
        }
        self.gone.retain(|k, (_, at)| now.saturating_sub(*at) < window && !res.prs.iter().any(|x| &x.key == k));
        res.prs.extend(self.gone.values().map(|(p, _)| p.clone()));

        Snapshot {
            me: if res.me.is_empty() { prev.me.clone() } else { res.me },
            prs: res.prs,
            errors: res.errors,
            fetched_at: now_ms(),
            loading: false,
        }
    }
}

/// Re-read ignores into an existing snapshot (after the user ignores a PR elsewhere).
pub fn reapply_ignores(data: &Path, snap: &mut Snapshot) {
    ignore::flag(data, &mut snap.prs);
}
