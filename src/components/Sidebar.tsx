import { useState } from "react";
import { timeAgo } from "../api";
import { useJobs } from "../jobs";
import type { Category, Pr, Snapshot } from "../types";

export const SECTIONS: { id: Category; label: string }[] = [
  { id: "reviewRequested", label: "Review requested" },
  { id: "reReview", label: "New commits since my review" },
  { id: "mine", label: "My PRs" },
  { id: "watched", label: "Watched repos" },
];

/** Each PR appears once, under its most urgent section. */
export function sectionOf(pr: Pr): Category {
  return SECTIONS.find((s) => pr.categories.includes(s.id))!.id;
}

export function orderedPrs(prs: Pr[], filter: string): Pr[] {
  const f = filter.toLowerCase();
  const match = (p: Pr) => !f || `${p.title} ${p.repo} ${p.author} #${p.number}`.toLowerCase().includes(f);
  return SECTIONS.flatMap((s) => prs.filter((p) => sectionOf(p) === s.id && match(p)));
}

export function Sidebar({ snap, selected, onSelect, onSettings, onRefresh, filter, setFilter }: {
  snap: Snapshot; selected?: string; onSelect: (k: string) => void; onSettings: () => void; onRefresh: () => void;
  filter: string; setFilter: (s: string) => void;
}) {
  const { jobs } = useJobs();
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const f = filter.toLowerCase();
  const match = (p: Pr) => !f || `${p.title} ${p.repo} ${p.author} #${p.number}`.toLowerCase().includes(f);
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
        {!snap.loading && snap.prs.length === 0 && <p className="muted pad">No open PRs match your settings.</p>}
        {snap.errors.map((e, i) => (
          <p key={i} className="banner error small">{e}</p>
        ))}
      </div>
      <div className="side-foot muted small">
        {snap.me && <>@{snap.me} · </>}
        {snap.fetchedAt ? `updated ${timeAgo(snap.fetchedAt)}` : "loading…"}
      </div>
    </aside>
  );
}
