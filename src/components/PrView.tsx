import { useCallback, useEffect, useMemo, useState } from "react";
import { parseDiff, type FileData } from "react-diff-view";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, timeAgo } from "../api";
import { useJobs } from "../jobs";
import { DiffFile, filePath } from "./DiffFile";
import { JobLog } from "./JobLog";
import { Markdown } from "./Markdown";
import { ReviewerList, ReviewerStack } from "./Reviewers";
import { MergeButton, MergeCard, MergeDialog, useMergeInfo } from "./Merge";
import { estimate, estimateTooltip, formatEstimate, type Estimate } from "../estimate";
import type { Annotation, Draft, Insights, Pr, PrDetail, PrState } from "../types";

type Tab = "overview" | "walkthrough" | "files" | "review" | "notes";
const TABS: { id: Tab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "walkthrough", label: "Walkthrough" },
  { id: "files", label: "Files" },
  { id: "review", label: "pr-review" },
  { id: "notes", label: "My notes" },
];

const norm = (p: string) => p.replace(/^(\.\/|a\/|b\/)/, "");

function useViewType() {
  const [v, setV] = useState<"unified" | "split">(() => {
    try {
      return (localStorage.getItem("viewType") as any) || "unified";
    } catch {
      return "unified";
    }
  });
  const set = (x: "unified" | "split") => {
    setV(x);
    try { localStorage.setItem("viewType", x); } catch {}
  };
  return [v, set] as const;
}

export function PrView({ pr, me, tab, setTab }: { pr: Pr; me: string; tab: Tab; setTab: (t: Tab) => void }) {
  const [detail, setDetail] = useState<PrDetail>();
  const [files, setFiles] = useState<FileData[]>();
  const [state, setState] = useState<PrState>({ drafts: [] });
  const [err, setErr] = useState<string>();
  const [viewType, setViewType] = useViewType();
  const jobs = useJobs();
  const merge = useMergeInfo(pr);
  const [merging, setMerging] = useState(false);
  const [merged, setMerged] = useState<string>();

  const reviewJob = jobs.latest(pr.key, "review");
  const insightsJob = jobs.latest(pr.key, "insights");
  const lines = pr.additions + pr.deletions;
  const est = {
    insights: estimate(jobs.history, "insights", lines, pr.changedFiles, jobs.settings?.insightsModel ?? ""),
    review: estimate(jobs.history, "review", lines, pr.changedFiles, jobs.settings?.reviewModel ?? ""),
  };
  const Est = ({ e }: { e: Estimate }) => (
    <span className={`est ${e.basis ? "" : "guess"}`} title={estimateTooltip(e)}>{formatEstimate(e)}</span>
  );
  const running = (k: "review" | "insights") => (k === "review" ? reviewJob : insightsJob)?.status === "running";

  useEffect(() => {
    let dead = false;
    setDetail(undefined);
    setFiles(undefined);
    setErr(undefined);
    api.detail(pr.repo, pr.number).then((d) => !dead && setDetail(d)).catch((e) => !dead && setErr(String(e)));
    api.diff(pr.repo, pr.number)
      .then((d) => !dead && setFiles(parseDiff(d, { nearbySequences: "zip" })))
      .catch((e) => !dead && setErr(String(e)));
    return () => { dead = true; };
  }, [pr.key, pr.headSha]);

  useEffect(() => {
    api.state(pr.repo, pr.number).then(setState);
  }, [pr.key, jobs.finishedTick]);

  const insights: Insights | null | undefined = state.insights;
  const annotationsByPath = useMemo(() => {
    const m = new Map<string, Annotation[]>();
    for (const a of insights?.annotations ?? []) {
      const k = norm(a.path);
      m.set(k, [...(m.get(k) ?? []), a]);
    }
    return m;
  }, [insights]);
  const summaryByPath = useMemo(
    () => new Map((insights?.fileSummaries ?? []).map((f) => [norm(f.path), f.summary])),
    [insights],
  );
  const draftsByPath = useMemo(() => {
    const m = new Map<string, Draft[]>();
    for (const d of state.drafts) m.set(d.path, [...(m.get(d.path) ?? []), d]);
    return m;
  }, [state.drafts]);

  const saveDrafts = useCallback(
    (fn: (d: Draft[]) => Draft[]) => {
      setState((s) => {
        const drafts = fn(s.drafts);
        api.saveDrafts(pr.repo, pr.number, drafts);
        return { ...s, drafts };
      });
    },
    [pr.repo, pr.number],
  );
  const onAddDraft = useCallback(
    (d: Omit<Draft, "id" | "createdAt">) =>
      saveDrafts((ds) => [...ds, { ...d, id: crypto.randomUUID(), createdAt: Date.now() }]),
    [saveDrafts],
  );
  const onEditDraft = useCallback(
    (id: string, body: string) => saveDrafts((ds) => ds.map((d) => (d.id === id ? { ...d, body } : d))),
    [saveDrafts],
  );
  const onDeleteDraft = useCallback((id: string) => saveDrafts((ds) => ds.filter((d) => d.id !== id)), [saveDrafts]);

  const renderFile = (f: FileData, opts: { collapsed?: boolean; idPrefix?: string } = {}) => {
    const p = filePath(f);
    return (
      <DiffFile
        key={p}
        id={`${opts.idPrefix ?? "f"}-${p}`}
        file={f}
        viewType={viewType}
        annotations={annotationsByPath.get(p) ?? []}
        drafts={draftsByPath.get(p) ?? []}
        summary={summaryByPath.get(p)}
        onAddDraft={onAddDraft}
        onEditDraft={onEditDraft}
        onDeleteDraft={onDeleteDraft}
        defaultCollapsed={opts.collapsed}
      />
    );
  };

  const stale = (sha?: string) => sha && sha !== pr.headSha;
  const concerns = (insights?.annotations ?? []).filter((a) => a.kind === "concern").length;

  return (
    <div className="prview">
      <header className="pr-head" data-tauri-drag-region>
        <div className="pr-title-row" data-tauri-drag-region>
          <h1 data-tauri-drag-region>
            {pr.title} <span className="muted">#{pr.number}</span>
          </h1>
          <div className="row">
            <button onClick={() => openUrl(pr.url)}>Open on GitHub ↗</button>
            <MergeButton info={merge.info} onClick={() => setMerging(true)} />
            <button
              className={running("insights") ? "busy" : ""}
              onClick={() => { setTab("walkthrough"); if (!running("insights")) jobs.start(pr.repo, pr.number, "insights"); }}
              title="Explain what each part of the diff does and why"
            >
              {running("insights") ? "Explaining…" : insights ? "↻ Re-explain" : "✦ Explain changes"}
              {!running("insights") && <Est e={est.insights} />}
            </button>
            <button
              className={`primary ${running("review") ? "busy" : ""}`}
              onClick={() => { setTab("review"); if (!running("review")) jobs.start(pr.repo, pr.number, "review"); }}
            >
              {running("review") ? "Reviewing…" : state.review ? "↻ Re-run pr-review" : "▶ Run pr-review"}
              {!running("review") && <Est e={est.review} />}
            </button>
          </div>
        </div>
        <div className="pr-meta">
          <span className="repo">{pr.repo}</span>
          <img className="avatar" src={pr.authorAvatar} alt="" />
          <span>{pr.author}</span>
          <span className="branch">{pr.headRef}</span>
          <span className="muted">→</span>
          <span className="branch">{pr.baseRef}</span>
          <span className="add">+{pr.additions}</span>
          <span className="del">−{pr.deletions}</span>
          <span className="muted">{pr.changedFiles} files · updated {timeAgo(pr.updatedAt)}</span>
          <ReviewerStack pr={pr} me={me} />
          {pr.isDraft && <span className="badge">Draft</span>}
          {pr.ci && <span className={`badge ci-${pr.ci.toLowerCase()}`}>CI {pr.ci.toLowerCase()}</span>}
          {pr.reviewDecision && <span className="badge">{pr.reviewDecision.replace("_", " ").toLowerCase()}</span>}
        </div>
        <nav className="tabs">
          {TABS.map((t, i) => (
            <button key={t.id} className={tab === t.id ? "tab on" : "tab"} onClick={() => setTab(t.id)} title={`⌘${i + 1}`}>
              {t.label}
              {t.id === "walkthrough" && running("insights") && <span className="spinner" />}
              {t.id === "review" && running("review") && <span className="spinner" />}
              {t.id === "review" && !running("review") && state.review && <span className="dot ok" />}
              {t.id === "notes" && state.drafts.length > 0 && <span className="count">{state.drafts.length}</span>}
              {t.id === "files" && concerns > 0 && <span className="count warn">{concerns}</span>}
            </button>
          ))}
          <span className="spacer" />
          {(tab === "walkthrough" || tab === "files") && (
            <div className="seg">
              <button className={viewType === "unified" ? "on" : ""} onClick={() => setViewType("unified")}>Unified</button>
              <button className={viewType === "split" ? "on" : ""} onClick={() => setViewType("split")}>Split</button>
            </div>
          )}
        </nav>
      </header>

      {merged && <div className="banner ok">{merged}. It leaves the list on the next refresh.</div>}
      {merging && merge.info && (
        <MergeDialog pr={pr} info={merge.info} onClose={() => setMerging(false)}
          onMerged={(m) => { setMerging(false); setMerged(m); merge.reload(); }} />
      )}
      {jobs.error && <div className="banner error">{jobs.error}</div>}
      {[insightsJob, reviewJob].map((j) => j?.status === "failed" && (
        <div key={j.id} className="banner error row">
          {j.kind === "review" ? "pr-review" : "Explain changes"} failed — see the log in the {j.kind === "review" ? "pr-review" : "Walkthrough"} tab.
          <span className="spacer" />
          <button className="link" onClick={() => setTab(j.kind === "review" ? "review" : "walkthrough")}>Show log</button>
          <button className="link" onClick={() => jobs.start(pr.repo, pr.number, j.kind)}>Retry</button>
        </div>
      ))}
      {err && <div className="banner error">{err}</div>}

      <div className="pr-body">
        {tab === "overview" && (
          <Overview pr={pr} me={me} mergeCard={<MergeCard info={merge.info} err={merge.err} onMerge={() => setMerging(true)} />} detail={detail} insights={insights} state={state} stale={!!stale(state.insightsMeta?.headSha)}
            onExplain={() => { setTab("walkthrough"); jobs.start(pr.repo, pr.number, "insights"); }} explaining={running("insights")} goto={setTab} />
        )}

        {tab === "walkthrough" && (
          <div className="walkthrough">
            {insightsJob && (insightsJob.status === "running" || insightsJob.status === "failed") && <JobLog job={insightsJob} compact={insightsJob.status === "running"} estimate={est.insights} />}
            {!files && <div className="muted pad">Loading diff…</div>}
            {files && !insights && !running("insights") && insightsJob?.status !== "failed" && (
              <div className="empty-cta">
                <p>No walkthrough yet. Claude reads the diff and the surrounding code, orders the changes as a story, and annotates each hunk with what it does and why.</p>
                <button className="primary" onClick={() => jobs.start(pr.repo, pr.number, "insights")}>✦ Explain changes <Est e={est.insights} /></button>
              </div>
            )}
            {files && insights && (
              <Walkthrough insights={insights} files={files} renderFile={renderFile} stale={!!stale(state.insightsMeta?.headSha)} />
            )}
          </div>
        )}

        {tab === "files" && files && <FilesTab files={files} renderFile={renderFile} annotationsByPath={annotationsByPath} draftsByPath={draftsByPath} />}
        {tab === "files" && !files && <div className="muted pad">Loading diff…</div>}

        {tab === "review" && (
          <div className="review-tab">
            {reviewJob && (reviewJob.status === "running" || reviewJob.status === "failed") && <JobLog job={reviewJob} estimate={est.review} />}
            {state.review ? (
              <div className="report card">
                <div className="report-meta muted">
                  pr-review · {state.reviewMeta && timeAgo(state.reviewMeta.finishedAt)}
                  {state.reviewMeta?.costUsd != null && ` · $${state.reviewMeta.costUsd.toFixed(2)}`}
                  {state.reviewMeta?.durationMs != null && ` · ${Math.round(state.reviewMeta.durationMs / 60000)} min`}
                  {stale(state.reviewMeta?.headSha) && <span className="badge warn">new commits since this review</span>}
                  <span className="spacer" />
                  <button className="link" onClick={() => navigator.clipboard.writeText(state.review!)}>Copy markdown</button>
                </div>
                <Markdown>{state.review}</Markdown>
              </div>
            ) : (
              !running("review") && (
                <div className="empty-cta">
                  <p>
                    Runs your <code>pr-review</code> skill headless in a throwaway worktree at the PR head. Nothing is posted,
                    pushed or fixed — the report lands here.
                  </p>
                  <button className="primary" onClick={() => jobs.start(pr.repo, pr.number, "review")}>▶ Run pr-review <Est e={est.review} /></button>
                </div>
              )
            )}
            {reviewJob && reviewJob.status !== "running" && reviewJob.status !== "failed" && (
              <details className="card">
                <summary>Run log</summary>
                <JobLog job={reviewJob} compact />
              </details>
            )}
          </div>
        )}

        {tab === "notes" && <NotesTab pr={pr} drafts={state.drafts} onAdd={onAddDraft} onDelete={onDeleteDraft} onEdit={onEditDraft} />}
      </div>
    </div>
  );
}

function Overview({ pr, me, mergeCard, detail, insights, state, stale, onExplain, explaining, goto }: {
  pr: Pr; me: string; mergeCard: React.ReactNode; detail?: PrDetail; insights?: Insights | null; state: PrState; stale: boolean;
  onExplain: () => void; explaining: boolean; goto: (t: Tab) => void;
}) {
  const checks = detail?.statusCheckRollup ?? [];
  const failing = checks.filter((c) => ["FAILURE", "ERROR", "TIMED_OUT", "CANCELLED"].includes((c.conclusion || c.state || "").toUpperCase()));
  return (
    <div className="overview">
      <div className="col main">
        <div className="card">
          <h3>✦ What this PR does {insights && <span className={`badge risk-${insights.risk}`}>{insights.risk} risk</span>}
            {stale && <span className="badge warn">outdated</span>}</h3>
          {insights ? (
            <>
              <Markdown>{insights.summary}</Markdown>
              {insights.riskReason && <p className="muted small">Risk: {insights.riskReason}</p>}
              <h4>Suggested reading order</h4>
              <ol className="chapters">
                {insights.chapters.map((c, i) => (
                  <li key={i}><a onClick={() => goto("walkthrough")}>{c.title}</a> <span className="muted">· {c.files.length} files</span></li>
                ))}
              </ol>
              {insights.testingNotes && (<><h4>How to test</h4><Markdown>{insights.testingNotes}</Markdown></>)}
            </>
          ) : (
            <div className="row">
              <p className="muted">Not explained yet.</p>
              <button onClick={onExplain} disabled={explaining}>{explaining ? "Explaining…" : "✦ Explain changes"}</button>
            </div>
          )}
        </div>
        <div className="card">
          <h3>Description</h3>
          {detail ? <Markdown>{detail.body || "_No description._"}</Markdown> : <p className="muted">Loading…</p>}
        </div>
        {detail && detail.comments.length + detail.reviews.filter((r) => r.body).length > 0 && (
          <div className="card">
            <h3>Conversation</h3>
            {[...detail.reviews.filter((r) => r.body).map((r) => ({ who: r.author.login, when: r.submittedAt, body: r.body, tag: r.state })),
              ...detail.comments.map((c) => ({ who: c.author.login, when: c.createdAt, body: c.body, tag: "" }))]
              .sort((a, b) => a.when.localeCompare(b.when))
              .map((c, i) => (
                <div key={i} className="comment">
                  <div className="muted small"><b>{c.who}</b> · {timeAgo(c.when)} {c.tag && <span className="badge">{c.tag.toLowerCase().replace("_", " ")}</span>}</div>
                  <Markdown>{c.body}</Markdown>
                </div>
              ))}
          </div>
        )}
      </div>
      <div className="col side">
        {mergeCard}
        <div className="card">
          <h3>pr-review</h3>
          {state.review ? (
            <>
              <p className="verdict">{state.review.split("\n").find((l) => l.startsWith("#"))?.replace(/^#+\s*/, "")}</p>
              <button className="link" onClick={() => goto("review")}>Read report →</button>
            </>
          ) : (
            <p className="muted small">Not run yet.</p>
          )}
        </div>
        <div className="card">
          <h3>Checks {failing.length > 0 && <span className="badge ci-failure">{failing.length} failing</span>}</h3>
          {checks.length === 0 && <p className="muted small">No checks.</p>}
          <ul className="checks">
            {checks.map((c, i) => {
              const st = (c.conclusion || c.state || c.status || "").toLowerCase();
              const url = c.detailsUrl || c.targetUrl;
              return (
                <li key={i} className={`check ck-${st}`}>
                  <span className="dot" /> {url ? <a onClick={() => openUrl(url)}>{c.name || c.context}</a> : c.name || c.context}
                  <span className="muted small"> {st}</span>
                </li>
              );
            })}
          </ul>
        </div>
        {detail && (
          <div className="card">
            <h3>Commits · {detail.commits.length}</h3>
            <ul className="commits">
              {detail.commits.map((c) => (
                <li key={c.oid}><code>{c.oid.slice(0, 7)}</code> {c.messageHeadline}</li>
              ))}
            </ul>
          </div>
        )}
        <ReviewerList pr={pr} me={me} />
      </div>
    </div>
  );
}

function Walkthrough({ insights, files, renderFile, stale }: {
  insights: Insights; files: FileData[]; renderFile: (f: FileData, o?: { collapsed?: boolean; idPrefix?: string }) => React.ReactNode; stale: boolean;
}) {
  const byPath = new Map(files.map((f) => [filePath(f), f]));
  const used = new Set<string>();
  const find = (p: string) => {
    const n = norm(p);
    if (byPath.has(n)) return n;
    return [...byPath.keys()].find((k) => k.endsWith(n) || n.endsWith(k));
  };
  const chapters = insights.chapters.map((c) => {
    const fs = c.files.map(find).filter((p): p is string => !!p && !used.has(p));
    fs.forEach((p) => used.add(p));
    return { ...c, paths: fs };
  });
  const rest = files.map(filePath).filter((p) => !used.has(p));
  return (
    <>
      {stale && <div className="banner warn">New commits were pushed after this walkthrough was written. Notes may point at old lines.</div>}
      <div className="card summary"><Markdown>{insights.summary}</Markdown></div>
      {chapters.map((c, i) => (
        <section key={i} className="chapter">
          <div className="chapter-head">
            <span className="chapter-num">{i + 1}</span>
            <h2>{c.title}</h2>
          </div>
          <Markdown className="chapter-body">{c.explanation}</Markdown>
          {c.paths.map((p) => renderFile(byPath.get(p)!, { idPrefix: "w" }))}
        </section>
      ))}
      {rest.length > 0 && (
        <section className="chapter">
          <div className="chapter-head">
            <span className="chapter-num">+</span>
            <h2>Everything else</h2>
          </div>
          <p className="muted">Files the walkthrough did not place in a chapter — usually lockfiles, generated code or trivial edits.</p>
          {rest.map((p) => renderFile(byPath.get(p)!, { collapsed: true, idPrefix: "w" }))}
        </section>
      )}
    </>
  );
}

function FilesTab({ files, renderFile, annotationsByPath, draftsByPath }: {
  files: FileData[]; renderFile: (f: FileData) => React.ReactNode;
  annotationsByPath: Map<string, Annotation[]>; draftsByPath: Map<string, Draft[]>;
}) {
  const [filter, setFilter] = useState("");
  const shown = files.filter((f) => filePath(f).toLowerCase().includes(filter.toLowerCase()));
  return (
    <div className="files-tab">
      <aside className="file-tree">
        <input placeholder="Filter files…" value={filter} onChange={(e) => setFilter(e.target.value)} />
        {shown.map((f) => {
          const p = filePath(f);
          const notes = annotationsByPath.get(p) ?? [];
          const warn = notes.some((a) => a.kind === "concern");
          return (
            <a key={p} className="file-link" title={p}
              onClick={() => document.getElementById(`f-${p}`)?.scrollIntoView({ behavior: "smooth", block: "start" })}>
              <span className={`ftype ftype-${f.type}`}>{f.type === "add" ? "A" : f.type === "delete" ? "D" : f.type === "rename" ? "R" : "M"}</span>
              <span className="fname">{p.split("/").pop()}</span>
              <span className="fdir muted">{p.split("/").slice(0, -1).join("/")}</span>
              {notes.length > 0 && <span className={warn ? "count warn" : "count"}>{notes.length}</span>}
              {(draftsByPath.get(p)?.length ?? 0) > 0 && <span className="count">✎</span>}
            </a>
          );
        })}
      </aside>
      <div className="file-diffs">
        <p className="muted small hint">Click a line number to add a private note.</p>
        {shown.map((f) => renderFile(f))}
      </div>
    </div>
  );
}

function NotesTab({ pr, drafts, onAdd, onDelete, onEdit }: {
  pr: Pr; drafts: Draft[]; onAdd: (d: Omit<Draft, "id" | "createdAt">) => void;
  onDelete: (id: string) => void; onEdit: (id: string, b: string) => void;
}) {
  const [general, setGeneral] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [text, setText] = useState("");
  const asMarkdown = () => {
    const g = drafts.filter((d) => !d.path);
    const f = drafts.filter((d) => d.path);
    return [
      ...g.map((d) => d.body),
      ...f.map((d) => `**${d.path}:${d.line}**${d.side === "old" ? " (removed line)" : ""}\n${d.body}`),
    ].join("\n\n---\n\n");
  };
  return (
    <div className="notes-tab">
      <div className="row">
        <p className="muted">Private notes for {pr.repo}#{pr.number}. Stored locally, never posted.</p>
        <span className="spacer" />
        <button disabled={!drafts.length} onClick={() => navigator.clipboard.writeText(asMarkdown())}>Copy all as markdown</button>
      </div>
      <div className="card">
        <textarea value={general} placeholder="General note about this PR…" onChange={(e) => setGeneral(e.target.value)} />
        <div className="row end">
          <button className="primary" disabled={!general.trim()} onClick={() => { onAdd({ path: "", line: 0, side: "new", body: general.trim() }); setGeneral(""); }}>Add note</button>
        </div>
      </div>
      {drafts.length === 0 && <p className="muted">No notes yet. Click a line number in any diff to add one.</p>}
      {drafts.map((d) => (
        <div key={d.id} className="card draft-row">
          <div className="row">
            <code>{d.path ? `${d.path}:${d.line}` : "General"}</code>
            <span className="muted small">{timeAgo(d.createdAt)}</span>
            <span className="spacer" />
            <button className="link" onClick={() => navigator.clipboard.writeText(d.body)}>Copy</button>
            <button className="link" onClick={() => { setEditing(d.id); setText(d.body); }}>Edit</button>
            <button className="link danger" onClick={() => onDelete(d.id)}>Delete</button>
          </div>
          {editing === d.id ? (
            <>
              <textarea value={text} onChange={(e) => setText(e.target.value)} autoFocus />
              <div className="row end">
                <button onClick={() => setEditing(null)}>Cancel</button>
                <button className="primary" onClick={() => { onEdit(d.id, text); setEditing(null); }}>Save</button>
              </div>
            </>
          ) : (
            <Markdown>{d.body}</Markdown>
          )}
        </div>
      ))}
    </div>
  );
}

export type { Tab };
