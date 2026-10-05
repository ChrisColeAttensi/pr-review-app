import { useState } from "react";
import { timeAgo } from "../api";
import { useJobs } from "../jobs";
import { useEffect } from "react";
import { api } from "../api";
import { Menu, MenuItem, PredefinedMenuItem } from "@tauri-apps/api/menu";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Category, IndexEntry, Pr, Snapshot } from "../types";
import { keys } from "../platform";

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
  if (!e) e = { drafts: 0, chat: 0 };
  const chip = e.verdict ? verdictChip(e.verdict) : null;
  const stale = !!e.reviewHead && e.reviewHead !== p.headSha;
  if (!chip && !e.insightsHead && !e.drafts && !e.chat && !p.teamOnly && !p.justApproved) return null;
  return (
    <div className="pr-flags">
      {p.justApproved && <span className="flag tone-ok" title="You approved this recently. It stays here for a while so you can merge it (Settings → Monitoring).">👍 approved</span>}
      {chip && (
        <span className={`flag tone-${stale ? "muted" : chip.tone}`} title={`pr-review: ${e.verdict}${stale ? " (new commits since)" : ""}`}>
          {chip.text}{stale && " ↻"}
        </span>
      )}
      {e.insightsHead && <span className="flag" title={e.insightsHead === p.headSha ? "Explained" : "Explained (new commits since)"}>✦</span>}
      {p.teamOnly && <span className="flag" title={`Requested from your team${(p.viaTeams ?? []).length ? ": " + p.viaTeams!.join(", ") : ""}`}>👥 {(p.viaTeams ?? [])[0]?.split("/").pop() ?? "team"}</span>}
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
  /** Saved as settings.directRequestsOnly. */
  hideTeam: boolean;
  /** Local only: ignored PRs never reach the tray or notifications. */
  showIgnored?: boolean;
}

/** Same rule as the tray menu, plus the local "show ignored" escape hatch. */
export const isVisible = (p: Pr, v: ViewFilter) =>
  (v.showIgnored ? true : !p.ignored) && !(v.hideTeam && p.teamOnly) && (v.showDrafts || !p.isDraft) && (!v.actionableOnly || p.needsAction || !!p.ignored);

export function orderedPrs(prs: Pr[], filter: string, view: ViewFilter): Pr[] {
  const f = filter.toLowerCase();
  const match = (p: Pr) =>
    isVisible(p, view) && (!f || `${p.title} ${p.repo} ${p.author} #${p.number}`.toLowerCase().includes(f));
  return SECTIONS.flatMap((s) => prs.filter((p) => sectionOf(p) === s.id && match(p)));
}

async function showContextMenu(p: Pr, start: (kind: "review" | "insights") => void) {
  try {
    const menu = await Menu.new({
      items: [
        await MenuItem.new({
          text: p.ignored ? "Stop ignoring" : "Ignore this PR",
          action: () => api.setIgnored(p.key, p.title, !p.ignored),
        }),
        await PredefinedMenuItem.new({ item: "Separator" }),
        await MenuItem.new({ text: "Open on GitHub", action: () => openUrl(p.url) }),
        await MenuItem.new({ text: "Copy link", action: () => navigator.clipboard.writeText(p.url) }),
        await PredefinedMenuItem.new({ item: "Separator" }),
        await MenuItem.new({ text: "Explain changes", action: () => start("insights") }),
        await MenuItem.new({ text: "Run pr-review", action: () => start("review") }),
      ],
    });
    await menu.popup();
  } catch {
    // Browser preview has no native menus.
  }
}

export function Sidebar({ snap, selected, onSelect, onSettings, onRefresh, filter, setFilter, view, setView }: {
  view: ViewFilter; setView: (v: ViewFilter) => void;
  snap: Snapshot; selected?: string; onSelect: (k: string) => void; onSettings: () => void; onRefresh: () => void;
  filter: string; setFilter: (s: string) => void;
}) {
  const { jobs, finishedTick, start, settings } = useJobs();
  const ignoredCount = snap.prs.filter((p) => p.ignored).length;
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
        <button className="icon" title={`Refresh (${keys("R")})`} onClick={onRefresh}>
          <span className={snap.loading ? "spin" : ""}>↻</span>
        </button>
        <button className="icon" title={`Settings (${keys(",")})`} onClick={onSettings}>⚙</button>
      </div>
      <div className="side-search">
        <input placeholder="Filter PRs…  (/)" value={filter} onChange={(e) => setFilter(e.target.value)} id="pr-filter" />
      </div>
      <div className="side-filters">
        <label><input type="checkbox" checked={view.actionableOnly} onChange={(e) => setView({ ...view, actionableOnly: e.target.checked })} /> Needs action only</label>
        <label title="Requests that reach you only through a team"><input type="checkbox" checked={!view.hideTeam} onChange={(e) => setView({ ...view, hideTeam: !e.target.checked })} /> Team requests</label>
        <label><input type="checkbox" checked={view.showDrafts} onChange={(e) => setView({ ...view, showDrafts: e.target.checked })} /> Show drafts</label>
        {(ignoredCount > 0 || view.showIgnored) && (
          <label><input type="checkbox" checked={!!view.showIgnored} onChange={(e) => setView({ ...view, showIgnored: e.target.checked })} /> Ignored ({ignoredCount})</label>
        )}
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
                  <div key={p.key} className={`pr-item ${selected === p.key ? "sel" : ""} ${p.ignored ? "ignored" : ""}`}
                    onClick={() => onSelect(p.key)}
                    onContextMenu={(e) => { e.preventDefault(); showContextMenu(p, (k) => start(p.repo, p.number, k, undefined, k === "review" && (settings?.allowChromeVerify ?? true))); }}>
                    <img className="avatar" src={p.authorAvatar} alt="" />
                    <div className="pr-item-main">
                      <div className="pr-item-title">
                        {p.ignored && <span className="badge tiny">ignored</span>}
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
