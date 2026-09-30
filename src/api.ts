import { invoke } from "@tauri-apps/api/core";
import type { HistoryRun } from "./estimate";
import type { JobInfo, JobKind, PrDetail, PrState, Settings, Snapshot, Draft } from "./types";

export const api = {
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  refresh: () => invoke<void>("refresh"),
  settings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  discoverRepos: () => invoke<Record<string, string>>("discover_repos"),
  detail: (repo: string, number: number) => invoke<PrDetail>("pr_detail", { repo, number }),
  diff: (repo: string, number: number) => invoke<string>("pr_diff", { repo, number }),
  state: (repo: string, number: number) => invoke<PrState>("pr_state", { repo, number }),
  saveDrafts: (repo: string, number: number, drafts: Draft[]) =>
    invoke<void>("save_drafts", { repo, number, drafts }),
  startJob: (repo: string, number: number, kind: JobKind) =>
    invoke<JobInfo>("start_job", { repo, number, kind }),
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  listJobs: () => invoke<JobInfo[]>("list_jobs"),
  jobLines: (id: string) => invoke<string[]>("job_lines", { id }),
  history: () => invoke<HistoryRun[]>("run_history"),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  cleanupWorktrees: (everything: boolean) =>
    invoke<{ removed: string[]; kept: number }>("cleanup_worktrees", { everything }),
  worktreeUsage: () => invoke<{ count: number; bytes: number }>("worktree_usage"),
  cleanWorktree: (repo: string, number: number) => invoke<void>("clean_worktree", { repo, number }),
};

export function timeAgo(input: string | number): string {
  const t = typeof input === "number" ? input : Date.parse(input);
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
