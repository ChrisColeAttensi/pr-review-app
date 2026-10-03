import { useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, timeAgo } from "../api";
import { useJobs } from "../jobs";
import { JobLog } from "./JobLog";
import { Markdown } from "./Markdown";
import type { Estimate } from "../estimate";
import type { ChatMessage, Draft, Pr, PrState, RunMeta } from "../types";
import { MOD_ENTER, isMac, mod } from "../platform";

function notesAsMarkdown(drafts: Draft[]) {
  return drafts
    .map((d) => (d.path ? `- \`${d.path}:${d.line}\`${d.side === "old" ? " (removed line)" : ""}: ${d.body}` : `- General: ${d.body}`))
    .join("\n");
}

/** `claude-opus-5-5` → `Opus 5.5`; aliases like `opus` → `Opus`. */
export function prettyModel(m?: string | null) {
  if (!m) return undefined;
  const x = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/.exec(m);
  if (x) return `${x[1][0].toUpperCase()}${x[1].slice(1)} ${x[2]}${x[3] ? `.${x[3]}` : ""}`;
  return m[0].toUpperCase() + m.slice(1);
}

/** Claude ends a message with this line when Chrome hits a login it may not pass itself. */
const SIGN_IN = /^\s*`?SIGN-IN NEEDED:\s*<?([^`>\s]+)>?`?\s*$/m;
const hideMarker = (t: string) => t.replace(SIGN_IN, "").trimEnd();

const newest = (...ms: (RunMeta | null | undefined)[]) =>
  ms.filter((m): m is RunMeta => !!m).sort((a, b) => b.finishedAt - a.finishedAt)[0];

/**
 * One conversation per PR: the pr-review report sits in the thread where it ran, and chat
 * resumes the newest session, so "run pr-review → fix the findings → verify" reads top to bottom.
 */
export function ClaudeTab({ pr, state, thread, onClear, chrome, setChrome, runReview, reviewEst, Est }: {
  pr: Pr;
  state: PrState;
  thread: number;
  onClear: () => Promise<void>;
  chrome: boolean;
  setChrome: (on: boolean) => void;
  runReview: () => void;
  reviewEst: Estimate;
  Est: (p: { e: Estimate }) => ReactNode;
}) {
  const jobs = useJobs();
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [text, setText] = useState("");
  const [pending, setPending] = useState<string>();
  const endRef = useRef<HTMLDivElement>(null);
  const chatJob = jobs.latest(pr.key, "chat");
  const reviewJob = jobs.latest(pr.key, "review");
  const chatting = chatJob?.status === "running";
  const reviewing = reviewJob?.status === "running";
  const busy = chatting || reviewing;
  const stop = () => [chatJob, reviewJob].forEach((j) => j?.status === "running" && jobs.cancel(j.id));
  // Two clicks, so a stray one doesn't throw the conversation away.
  const [confirmClear, setConfirmClear] = useState(false);
  useEffect(() => {
    if (!confirmClear) return;
    const t = setTimeout(() => setConfirmClear(false), 4000);
    return () => clearTimeout(t);
  }, [confirmClear]);

  useEffect(() => {
    api.chat(pr.repo, pr.number).then((m) => {
      setMessages(m);
      setPending(undefined);
    });
  }, [pr.key, jobs.finishedTick, thread]);
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [messages.length, busy, pending, state.reviewMeta?.finishedAt]);

  const send = (msg: string) => {
    const m = msg.trim();
    if (!m || busy) return;
    setPending(m);
    setText("");
    jobs.start(pr.repo, pr.number, "chat", m, chrome);
  };

  const quick: { label: string; message: string; show: boolean }[] = [
    {
      label: "Fix the findings",
      show: !!state.review,
      message:
        "Fix the findings in the 'Change before merge' section of the pr-review report, one at a time, in this worktree. Run the repo's checks after each. Then summarise what changed per finding. Don't commit or push yet.",
    },
    {
      label: "Verify in Chrome",
      show: chrome && !!state.review,
      message:
        "Verify the change in Claude in Chrome: check the claims the diff and CI couldn't settle, and anything you just fixed. Tell me what you saw, and which parts you couldn't reach.",
    },
    {
      label: "Turn my notes into review comments",
      show: state.drafts.length > 0,
      message: `Here are my private review notes on this PR:\n\n${notesAsMarkdown(state.drafts)}\n\nRewrite each one as a clear, friendly GitHub review comment anchored to its file and line. Show me the drafts; don't post anything until I say so.`,
    },
    { label: "What's the riskiest part?", show: !state.review, message: "What's the riskiest part of this change, and what would you test by hand before approving?" },
    {
      label: "Draft a PR summary comment",
      show: true,
      message: "Draft a short PR comment summarising my review: what looks good, and anything that must change. Show it to me; don't post it.",
    },
  ];

  const meta = newest(state.chatMeta, state.reviewMeta);
  const model = prettyModel(meta?.resolvedModel || jobs.settings?.reviewModel) ?? "default model";

  // The report goes in the thread at the moment it finished.
  const reportAt = state.review ? state.reviewMeta?.finishedAt ?? 0 : undefined;
  type Item = { k: "msg"; m: ChatMessage; ts: number } | { k: "report"; ts: number };
  const items: Item[] = [
    ...messages.map((m) => ({ k: "msg" as const, m, ts: m.ts })),
    ...(reportAt != null ? [{ k: "report" as const, ts: reportAt }] : []),
  ].sort((a, b) => a.ts - b.ts);
  const showPending = pending && !messages.some((m) => m.role === "user" && m.text === pending && m.ts > (chatJob?.startedAt ?? 0) - 1000);
  const last = items[items.length - 1];
  const lastText = !last ? "" : last.k === "report" ? state.review ?? "" : last.m.role === "assistant" ? last.m.text : "";
  const signIn = !busy ? SIGN_IN.exec(lastText)?.[1] : undefined;
  const signInHost = signIn ? (() => { try { return new URL(signIn).host; } catch { return signIn; } })() : "";
  const empty = items.length === 0 && !pending && !reviewing && reviewJob?.status !== "failed";
  const stale = state.reviewMeta && state.reviewMeta.headSha !== pr.headSha;

  return (
    <div className="chat-tab">
      <div className="chat-head row">
        <span className="model-chip" title="The model this conversation runs on (Settings → Claude)">{model}</span>
        <span className="muted small">Works in this PR's worktree. Asks before posting or pushing.</span>
        <span className="spacer" />
        <button
          className={confirmClear ? "danger" : ""}
          disabled={busy || (messages.length === 0 && !state.review)}
          title="Start over: removes the chat and the pr-review report. Edits in the worktree stay."
          onClick={() => { if (confirmClear) { setConfirmClear(false); onClear(); } else setConfirmClear(true); }}
        >
          {confirmClear ? "Click again to clear" : "Clear"}
        </button>
        <button onClick={() => api.handoff(pr.repo, pr.number, "desktop", pr.url)} title="Resume this conversation in Claude Desktop's Code tab">
          Continue in Claude Desktop ↗
        </button>
        <button onClick={() => api.handoff(pr.repo, pr.number, "terminal", pr.url)} title={isMac ? "claude --resume in Terminal" : "claude --resume in a new terminal window"}>
          {isMac ? "Open in Terminal" : "Open in terminal"}
        </button>
      </div>

      <div className="chat-log">
        {empty && (
          <div className="empty-cta">
            <p>
              Run your <code>pr-review</code> skill headless in a throwaway worktree at the PR head. Nothing is posted,
              pushed or fixed; the report lands here, and you can carry on the conversation from it.
            </p>
            <button className="primary" onClick={runReview}>▶ Run pr-review <Est e={reviewEst} /></button>
            <p className="muted small">Or just ask Claude something about this PR below.</p>
          </div>
        )}
        {items.map((it, i) =>
          it.k === "report" ? (
            <details key={`r${it.ts}`} className="report card" open>
              <summary className="report-meta muted">
                <b>pr-review</b> · {timeAgo(it.ts)}
                {state.reviewMeta?.costUsd != null && ` · $${state.reviewMeta.costUsd.toFixed(2)}`}
                {state.reviewMeta?.durationMs != null && ` · ${Math.round(state.reviewMeta.durationMs / 60000)} min`}
                {stale && <span className="badge warn">new commits since this review</span>}
                <span className="spacer" />
                <button className="link" onClick={(e) => { e.preventDefault(); navigator.clipboard.writeText(state.review!); }}>Copy markdown</button>
              </summary>
              <Markdown>{hideMarker(state.review!)}</Markdown>
              {reviewJob && reviewJob.status === "done" && (
                <details className="run-log">
                  <summary className="muted small">Run log</summary>
                  <JobLog job={reviewJob} compact />
                </details>
              )}
            </details>
          ) : (
            <div key={i} className={`bubble bubble-${it.m.role}`}>
              {it.m.role === "user" ? <div className="bubble-text">{it.m.text}</div> : <Markdown>{hideMarker(it.m.text)}</Markdown>}
              {it.m.role === "assistant" && (
                <button className="link copy" onClick={() => navigator.clipboard.writeText(it.m.text)}>Copy</button>
              )}
            </div>
          ),
        )}
        {showPending && <div className="bubble bubble-user"><div className="bubble-text">{pending}</div></div>}
        {reviewJob && (reviewing || reviewJob.status === "failed") && <JobLog job={reviewJob} estimate={reviewEst} />}
        {reviewJob?.status === "failed" && (
          <div className="row"><button onClick={runReview}>Retry pr-review</button></div>
        )}
        {chatting && chatJob && <JobLog job={chatJob} compact />}
        {chatJob?.status === "failed" && <JobLog job={chatJob} compact />}
        <div ref={endRef} />
      </div>

      <div className="chat-dock">
        {signIn && (
          <div className="banner warn row sign-in">
            <span>🔐 Claude needs you to sign in to <b>{signInHost}</b> in Chrome. Its tab is waiting on the login page.</span>
            <span className="spacer" />
            <button className="link" onClick={() => openUrl(signIn)} title="Opens in your default browser">Open login page</button>
            <button className="primary" onClick={() => send(`I've signed in to ${signInHost} in Chrome. Carry on with the check from where you stopped.`)}>
              I've signed in, continue
            </button>
          </div>
        )}
        <div className="chat-quick">
          {!empty && (
            <button className="chip-btn chip-primary" disabled={busy} onClick={runReview}
              title="Clears this thread, then runs pr-review fresh at the PR head. Chat continues from the new report.">
              {reviewing ? "Reviewing…" : state.review ? "↻ Re-run pr-review" : "▶ Run pr-review"}
              {!reviewing && <Est e={reviewEst} />}
            </button>
          )}
          {quick.filter((q) => q.show).map((q) => (
            <button key={q.label} className="chip-btn" disabled={busy} onClick={() => send(q.message)}>{q.label}</button>
          ))}
        </div>
        <div className="chat-input">
          <textarea
            value={text}
            placeholder={reviewing ? "pr-review is running…" : chatting ? "Claude is working…" : `Message Claude about this PR…  (${MOD_ENTER} to send)`}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && mod(e)) { e.preventDefault(); send(text); } }}
          />
          {busy ? (
            <button className="danger" onClick={stop} title={reviewing ? "Stop pr-review" : "Stop Claude"}>■ Stop</button>
          ) : (
            <button className="primary" disabled={!text.trim()} onClick={() => send(text)}>Send</button>
          )}
        </div>
        <label className="check live-check" title="Starts claude with --chrome, for pr-review and for chat. pr-review only opens Chrome for changes the diff and CI can't settle.">
          <input type="checkbox" checked={chrome} onChange={(e) => setChrome(e.target.checked)} />
          Allow Claude to verify in Chrome when needed <span className="muted small">(Claude in Chrome)</span>
        </label>
      </div>
    </div>
  );
}
