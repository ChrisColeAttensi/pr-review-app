import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { JobInfo, JobKind, Settings } from "./types";
import type { HistoryRun } from "./estimate";

interface JobsCtx {
  jobs: JobInfo[];
  lines: Record<string, string[]>;
  start: (repo: string, number: number, kind: JobKind, message?: string) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  latest: (prKey: string, kind: JobKind) => JobInfo | undefined;
  /** Bumped whenever a job finishes, so views can reload their PR state. */
  finishedTick: number;
  /** Past runs on this machine, for estimates. */
  history: HistoryRun[];
  settings?: Settings;
  error?: string;
}

const Ctx = createContext<JobsCtx>(null!);
export const useJobs = () => useContext(Ctx);

export function JobsProvider({ children }: { children: ReactNode }) {
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [lines, setLines] = useState<Record<string, string[]>>({});
  const [finishedTick, setFinishedTick] = useState(0);
  const [error, setError] = useState<string>();
  const [history, setHistory] = useState<HistoryRun[]>([]);
  const [settings, setSettings] = useState<Settings>();
  const pending = useRef<Record<string, string[]>>({});

  useEffect(() => {
    // Restore jobs that were running before a webview reload.
    api.listJobs().then(async (js) => {
      setJobs(js);
      const all: Record<string, string[]> = {};
      for (const j of js) all[j.id] = await api.jobLines(j.id);
      setLines((l) => ({ ...all, ...l }));
    });

    // Batch line updates so a chatty run doesn't re-render per line.
    const flush = setInterval(() => {
      const p = pending.current;
      if (Object.keys(p).length === 0) return;
      pending.current = {};
      setLines((l) => {
        const next = { ...l };
        for (const [id, ls] of Object.entries(p)) next[id] = [...(next[id] ?? []), ...ls];
        return next;
      });
    }, 250);

    const uns = [
      listen<JobInfo>("job-started", (e) =>
        setJobs((js) => [...js.filter((j) => j.id !== e.payload.id), e.payload]),
      ),
      listen<{ jobId: string; line: string }>("job-event", (e) => {
        (pending.current[e.payload.jobId] ??= []).push(e.payload.line);
      }),
      listen<{ jobId: string; job: JobInfo | null }>("job-done", (e) => {
        const job = e.payload.job;
        if (job) setJobs((js) => js.map((j) => (j.id === job.id ? job : j)));
        setFinishedTick((t) => t + 1);
      }),
    ];
    return () => {
      clearInterval(flush);
      uns.forEach((u) => u.then((f) => f()));
    };
  }, []);

  // Reload after every finished run, and after settings change (the model affects estimates).
  useEffect(() => {
    api.history().then(setHistory).catch(() => {});
    api.settings().then(setSettings).catch(() => {});
  }, [finishedTick]);
  useEffect(() => {
    const u = listen("prs-updated", () => api.settings().then(setSettings).catch(() => {}));
    return () => { u.then((f) => f()); };
  }, []);

  const value = useMemo<JobsCtx>(
    () => ({
      jobs,
      lines,
      finishedTick,
      history,
      settings,
      error,
      start: async (repo, number, kind, message) => {
        setError(undefined);
        try {
          const j = await api.startJob(repo, number, kind, message);
          setJobs((js) => [...js.filter((x) => x.id !== j.id), j]);
        } catch (e) {
          setError(String(e));
        }
      },
      cancel: (id) => api.cancelJob(id),
      latest: (prKey, kind) =>
        [...jobs].reverse().find((j) => j.prKey === prKey && j.kind === kind),
    }),
    [jobs, lines, finishedTick, error, history, settings],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

// ---------- stream-json → readable log ----------

export interface LogEntry {
  tone: "info" | "tool" | "text" | "error" | "done" | "agent";
  text: string;
  detail?: string;
  nested?: boolean;
}

function toolSummary(name: string, input: Record<string, unknown>): string {
  const s = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : "");
  switch (name) {
    case "Bash":
      return s("description") || s("command");
    case "Read":
    case "Write":
    case "Edit":
      return s("file_path").split("/").slice(-3).join("/");
    case "Grep":
    case "Glob":
      return s("pattern");
    case "Agent":
    case "Task":
      return `${s("subagent_type") || "agent"} — ${s("description")}`;
    case "Skill":
      return s("skill");
    default:
      return JSON.stringify(input).slice(0, 140);
  }
}

export function parseLog(raw: string[]): LogEntry[] {
  const out: LogEntry[] = [];
  for (const line of raw) {
    let v: any;
    try {
      v = JSON.parse(line);
    } catch {
      continue;
    }
    const nested = !!v.parent_tool_use_id;
    switch (v.type) {
      case "app_progress":
        out.push({ tone: "info", text: v.message });
        break;
      case "app_error":
        out.push({ tone: "error", text: v.message });
        break;
      case "system":
        if (v.subtype === "init") out.push({ tone: "info", text: `Claude session started · ${v.model}` });
        break;
      case "assistant":
        for (const c of v.message?.content ?? []) {
          if (c.type === "tool_use") {
            const isAgent = c.name === "Agent" || c.name === "Task";
            out.push({
              tone: isAgent ? "agent" : "tool",
              text: `${c.name}`,
              detail: toolSummary(c.name, c.input ?? {}),
              nested,
            });
          } else if (c.type === "text" && c.text?.trim()) {
            out.push({ tone: "text", text: c.text.trim(), nested });
          }
        }
        break;
      case "user":
        for (const c of v.message?.content ?? []) {
          if (c.type === "tool_result" && c.is_error) {
            const t = typeof c.content === "string" ? c.content : JSON.stringify(c.content);
            out.push({ tone: "error", text: "Tool error", detail: t.slice(0, 300), nested });
          }
        }
        break;
      case "result": {
        const cost = v.total_cost_usd != null ? ` · $${v.total_cost_usd.toFixed(2)}` : "";
        const dur = v.duration_ms ? ` · ${Math.round(v.duration_ms / 1000)}s` : "";
        out.push({ tone: v.is_error ? "error" : "done", text: `Finished${cost}${dur}` });
        break;
      }
    }
  }
  return out;
}
