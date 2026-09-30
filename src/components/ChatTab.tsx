import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { useJobs } from "../jobs";
import { JobLog } from "./JobLog";
import { Markdown } from "./Markdown";
import type { ChatMessage, Draft, Pr, PrState } from "../types";

function notesAsMarkdown(drafts: Draft[]) {
  return drafts
    .map((d) => (d.path ? `- \`${d.path}:${d.line}\`${d.side === "old" ? " (removed line)" : ""}: ${d.body}` : `- General: ${d.body}`))
    .join("\n");
}

export function ChatTab({ pr, state }: { pr: Pr; state: PrState }) {
  const jobs = useJobs();
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [text, setText] = useState("");
  const [pending, setPending] = useState<string>();
  const endRef = useRef<HTMLDivElement>(null);
  const job = jobs.latest(pr.key, "chat");
  const running = job?.status === "running";

  useEffect(() => {
    api.chat(pr.repo, pr.number).then((m) => {
      setMessages(m);
      setPending(undefined);
    });
  }, [pr.key, jobs.finishedTick]);
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [messages.length, running, pending]);

  const send = (msg: string) => {
    const m = msg.trim();
    if (!m || running) return;
    setPending(m);
    setText("");
    jobs.start(pr.repo, pr.number, "chat", m);
  };

  const quick: { label: string; message: string; show: boolean }[] = [
    {
      label: "Fix the pr-review findings",
      show: !!state.review,
      message:
        "Fix the findings in the 'Change before merge' section of the pr-review report, one at a time, in this worktree. Run the repo's checks after each. Then summarise what changed per finding. Don't commit or push yet.",
    },
    {
      label: "Turn my notes into review comments",
      show: state.drafts.length > 0,
      message: `Here are my private review notes on this PR:\n\n${notesAsMarkdown(state.drafts)}\n\nRewrite each one as a clear, friendly GitHub review comment anchored to its file and line. Show me the drafts; don't post anything until I say so.`,
    },
    { label: "What's the riskiest part?", show: true, message: "What's the riskiest part of this change, and what would you test by hand before approving?" },
    {
      label: "Draft a PR summary comment",
      show: true,
      message: "Draft a short PR comment summarising my review: what looks good, and anything that must change. Show it to me; don't post it.",
    },
  ];

  const context = messages.length
    ? "Continuing this chat"
    : state.review
      ? "Continues the pr-review conversation"
      : state.insights
        ? "Continues the Explain conversation"
        : "New conversation about this PR";

  return (
    <div className="chat-tab">
      <div className="chat-head row">
        <span className="muted small">{context}. Claude works in this PR's worktree and asks before posting or pushing.</span>
        <span className="spacer" />
        <button onClick={() => api.handoff(pr.repo, pr.number, "desktop", pr.url)} title="Resume this conversation in Claude Desktop's Code tab">
          Continue in Claude Desktop ↗
        </button>
        <button onClick={() => api.handoff(pr.repo, pr.number, "terminal", pr.url)} title="claude --resume in Terminal">
          Open in Terminal
        </button>
      </div>

      <div className="chat-log">
        {messages.length === 0 && !pending && (
          <div className="empty-cta">
            <p>Ask Claude anything about this PR, or have it act: fix findings, draft comments, run checks.</p>
          </div>
        )}
        {messages.map((m, i) => (
          <div key={i} className={`bubble bubble-${m.role}`}>
            {m.role === "user" ? <div className="bubble-text">{m.text}</div> : <Markdown>{m.text}</Markdown>}
            {m.role === "assistant" && (
              <button className="link copy" onClick={() => navigator.clipboard.writeText(m.text)}>Copy</button>
            )}
          </div>
        ))}
        {pending && !messages.some((m) => m.role === "user" && m.text === pending && m.ts > (job?.startedAt ?? 0) - 1000) && (
          <div className="bubble bubble-user"><div className="bubble-text">{pending}</div></div>
        )}
        {running && job && <JobLog job={job} compact />}
        <div ref={endRef} />
      </div>

      <div className="chat-quick">
        {quick.filter((q) => q.show).map((q) => (
          <button key={q.label} className="chip-btn" disabled={running} onClick={() => send(q.message)}>{q.label}</button>
        ))}
      </div>
      <div className="chat-input">
        <textarea
          value={text}
          placeholder={running ? "Claude is working…" : "Message Claude about this PR…  (⌘↩ to send)"}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && e.metaKey) { e.preventDefault(); send(text); } }}
        />
        <button className="primary" disabled={running || !text.trim()} onClick={() => send(text)}>Send</button>
      </div>
    </div>
  );
}
