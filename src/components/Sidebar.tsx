import { useState } from "react";
import { timeAgo } from "../api";
import { useJobs } from "../jobs";
import { useEffect } from "react";
import { api } from "../api";
import type { Category, IndexEntry, Pr, Snapshot } from "../types";

export const SECTIONS: { id: Category; label: string }[] = [
  { id: "reviewRequested", label: "Review requested" },
  { id: "reReview", label: "New commits since my review" },
  { id: "mine", label: "My PRs" },
  { id: "watched", label: "Watched repos" },
];

export type Approval = "approved" | "changes" | "waiting";

/** Mirrors the tray: reviewDecision, or the latest reviews where no review is required. */
export function approvalOf(p: Pr): Approval {
  const states = (p.latestReviews ?? []).map((r) => r.state);
  const d = p.reviewDecision ?? (states.includes("CHANGES_REQUESTED") ? "CHANGES_REQUESTED" : states.includes("APPROVED") ? "APPROVED" : "");
  return d === "APPROVED" ? "approved" : d === "CHANGES_REQUESTED" ? "changes" : "waiting";
}

const APPROVAL_LABEL: Record<Approval, string> = {
  approved: "✓ Approved",
  changes: "✕ Changes requested",
  waiting: "Awaiting review",
};

/** Compact label for a pr-review verdict heading like "✅ Approve — reason". */
function verdictChip(v: string): { text: string; tone: string } {
  if (/approve/i.test(v)) return { text: "✓ Reviewed", tone: "ok" };
  if (/change before merge|changes/i.test(v)) return { text: "🛠 Changes", tone: "bad" };
  if (/decision/i.test(v)) return { text: "🤔 Decide", tone: "warn" };
  return { text: "✓ Reviewed", tone: "ok" };
}

function Flags({ p, e }: { p: Pr; e?: IndexEntry }) {
  if (!e) return null;
  const chip = e.verdict ? verdictChip(e.verdict) : null;
  const stale = !!e.reviewHead && e.reviewHead !== p.headSha;
  if (!chip && !e.insightsHead && !e.drafts && !e.chat) return null;
  return (
    <div className="pr-flags">
      {chip && (
        <span className={`flag tone-${stale ? "muted" : chip.tone}`} title={`pr-review: ${e.verdict}${stale ? " (new commits since)" : ""}`}>
          {chip.text}{stale && " ↻"}
        </span>
      )}
      {e.insightsHead && <span className="flag" title={e.insightsHead === p.headSha ? "Explained" : "Explained (new commits since)"}>✦</span>}
      {e.drafts > 0 && <span className="flag" title={`${e.drafts} private notes`}>✎ {e.drafts}</span>}
      {e.chat > 0 && <span className="flag" title="Claude conversation">💬</span>}
    </div>
  );
}

/** Each PR appears once, under its most urgent section. */
export function sectionOf(pr: Pr): Category {
  return SECTIONS.find((s) => pr.categories.includes(s.id))!.id;
}

export interface ViewFilter {
  actionableOnly: boolean;
  showDrafts: boolean;
}

/** Same rule as the tray menu. */
export const isVisible = (p: Pr, v: ViewFilter) => (v.showDrafts || !p.isDraft) && (!v.actionableOnly || p.needsAction);

export function orderedPrs(prs: Pr[], filter: string, view: ViewFilter): Pr[] {
  const f = filter.toLowerCase();
  const match = (p: Pr) =>
    isVisible(p, view) && (!f || `${p.title} ${p.repo} ${p.author} #${p.number}`.toLowerCase().includes(f));
  return SECTIONS.flatMap((s) => prs.filter((p) => sectionOf(p) === s.id && match(p)));
}

export function Sidebar({ snap, selected, onSelect, onSettings, onRefresh, filter, setFilter, view, setView }: {
  view: ViewFilter; setView: (v: ViewFilter) => void;
  snap: Snapshot; selected?: string; onSelect: (k: string) => void; onSettings: () => void; onRefresh: () => void;
  filter: string; setFilter: (s: string) => void;
}) {
  const { jobs, finishedTick } = useJobs();
  const [index, setIndex] = useState<Record<string, IndexEntry>>({});
  useEffect(() => {
    api.index().then(setIndex).catch(() => {});
  }, [finishedTick, snap.fetchedAt, selected]);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const f = filter.toLowerCase();
  const match = (p: Pr) =>
    isVisible(p, view) && (!f || `${p.title} ${p.repo} ${p.author} #${p.number}`.toLowerCase().includes(f));
  const hidden = snap.prs.filter((p) => !isVisible(p, view)).length;
  const shownCount = snap.prs.length - hidden;
  const runningFor = (key: string) => jobs.some((j) => j.prKey === key && j.status === "running");

  return (
    <aside className="sidebar">
      <div className="side-top" data-tauri-drag-region>
        <span className="app-name" data-tauri-drag-region>PR Review</span>
        <button className="icon" title="Refresh (⌘R)" onClick={onRefresh}>
          <span className={snap.loading ? "spin" : ""}>↻</span>
        </button>
        <button className="icon" title="Settings (⌘,)" onClick={onSettings}>⚙</button>
      </div>
      <div className="side-search">
        <input placeholder="Filter PRs…  (/)" value={filter} onChange={(e) => setFilter(e.target.value)} id="pr-filter" />
      </div>
      <div className="side-filters">
        <label><input type="checkbox" checked={view.actionableOnly} onChange={(e) => setView({ ...view, actionableOnly: e.target.checked })} /> Needs action only</label>
        <label><input type="checkbox" checked={view.showDrafts} onChange={(e) => setView({ ...view, showDrafts: e.target.checked })} /> Show drafts</label>
      </div>
      <div className="side-list">
        {SECTIONS.map((s) => {
          const items = snap.prs.filter((p) => sectionOf(p) === s.id && match(p));
          if (!items.length) return null;
          return (
            <div key={s.id} className="section">
              <div className="section-head" onClick={() => setCollapsed({ ...collapsed, [s.id]: !collapsed[s.id] })}>
                <span>{collapsed[s.id] ? "▸" : "▾"} {s.label}</span>
                <span className="count">{items.length}</span>
              </div>
              {!collapsed[s.id] &&
                items.map((p) => (
                  <div key={p.key} className={`pr-item ${selected === p.key ? "sel" : ""}`} onClick={() => onSelect(p.key)}>
                    <img className="avatar" src={p.authorAvatar} alt="" />
                    <div className="pr-item-main">
                      <div className="pr-item-title">
                        {p.isDraft && <span className="badge tiny">draft</span>}
                        {p.title}
                      </div>
                      {s.id === "mine" && (
                        <span className={`approval approval-${approvalOf(p)}`}>
                          {APPROVAL_LABEL[approvalOf(p)]}
                          {(p.ci === "FAILURE" || p.ci === "ERROR") && <span className="approval-changes"> · CI failing</span>}
                        </span>
                      )}
                      <Flags p={p} e={index[p.key]} />
                      <div className="pr-item-sub">
                        <span>{p.repo.split("/")[1]}#{p.number}</span>
                        <span>· {p.author}</span>
                        <span>· {timeAgo(p.updatedAt)}</span>
                      </div>
                    </div>
                    <div className="pr-item-side">
                      {runningFor(p.key) && <span className="spinner" title="Claude is working on this" />}
                      {p.ci && <span className={`ci-dot ci-${p.ci.toLowerCase()}`} title={`CI ${p.ci.toLowerCase()}`} />}
                      <span className="sz">
                        <span className="add">+{p.additions}</span> <span className="del">−{p.deletions}</span>
                      </span>
                    </div>
                  </div>
                ))}
            </div>
          );
        })}
        {!snap.loading && shownCount === 0 && (
          <p className="muted pad">{hidden ? "Nothing needs your action." : "No open PRs match your settings."}</p>
        )}
        {snap.errors.map((e, i) => (
          <p key={i} className="banner error small">{e}</p>
        ))}
      </div>
      <div className="side-foot muted small">
        {snap.me && <>@{snap.me} · </>}
        {snap.fetchedAt ? `updated ${timeAgo(snap.fetchedAt)}` : "loading…"}
        {hidden > 0 && <> · {hidden} hidden</>}
      </div>
    </aside>
  );
}
