import { useEffect, useMemo, useRef, useState } from "react";
import { parseLog, useJobs } from "../jobs";
import type { JobInfo } from "../types";
import { estimateTooltip, formatDuration, type Estimate } from "../estimate";

function useElapsed(start: number, running: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [running]);
  const s = Math.floor((now - start) / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function JobLog({ job, compact, estimate }: { job: JobInfo; compact?: boolean; estimate?: Estimate }) {
  const { lines, cancel } = useJobs();
  const entries = useMemo(() => parseLog(lines[job.id] ?? []), [lines, job.id]);
  const running = job.status === "running";
  const elapsed = useElapsed(job.startedAt, running);
  const [showText, setShowText] = useState(!compact);
  const endRef = useRef<HTMLDivElement>(null);
  const agents = entries.filter((e) => e.tone === "agent").length;
  const tools = entries.filter((e) => e.tone === "tool").length;

  useEffect(() => {
    if (running) endRef.current?.scrollIntoView({ block: "nearest" });
  }, [entries.length, running]);

  const shown = showText ? entries : entries.filter((e) => e.tone !== "text");
  return (
    <div className={`joblog card ${job.status}`}>
      <div className="joblog-head">
        {running && <span className="spinner" />}
        <b>{job.kind === "review" ? "pr-review" : "Explaining changes"}</b>
        <span className="muted">
          {job.status} · {elapsed}
          {running && estimate && (
            <span title={estimateTooltip(estimate)}> of {formatDuration(estimate.seconds)}{estimate.basis ? "" : "?"}</span>
          )}{" "}
          · {tools} tool calls{agents ? ` · ${agents} agents` : ""}
        </span>
        <span className="spacer" />
        <label className="small muted">
          <input type="checkbox" checked={showText} onChange={(e) => setShowText(e.target.checked)} /> narration
        </label>
        {running && <button className="danger" onClick={() => cancel(job.id)}>Stop</button>}
      </div>
      <div className={`joblog-body ${compact ? "compact" : ""}`}>
        {shown.map((e, i) => (
          <div key={i} className={`le le-${e.tone} ${e.nested ? "nested" : ""}`}>
            <span className="le-text">{e.tone === "text" ? e.text.slice(0, 600) : e.text}</span>
            {e.detail && <span className="le-detail">{e.detail}</span>}
          </div>
        ))}
        <div ref={endRef} />
      </div>
    </div>
  );
}
