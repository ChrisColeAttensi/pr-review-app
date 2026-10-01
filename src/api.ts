import { invoke } from "@tauri-apps/api/core";
import type { HistoryRun } from "./estimate";
import type { MergeInfo } from "./components/Merge";
import type { ChatMessage, IndexEntry, JobInfo, JobKind, PrDetail, PrState, Settings, Snapshot, Draft } from "./types";

export interface SlackUser {
  userId: string;
  name: string;
  title: string;
  email: string;
}

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
  startJob: (repo: string, number: number, kind: JobKind, message?: string, chrome = false) =>
    invoke<JobInfo>("start_job", { repo, number, kind, message: message ?? null, chrome }),
  chat: (repo: string, number: number) => invoke<ChatMessage[]>("pr_chat", { repo, number }),
  index: () => invoke<Record<string, IndexEntry>>("pr_index"),
  setIgnored: (key: string, title: string, ignored: boolean) => invoke<void>("set_ignored", { key, title, ignored }),
  handoff: (repo: string, number: number, target: "desktop" | "terminal", url: string) =>
    invoke<void>("handoff", { repo, number, target, url }),
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  listJobs: () => invoke<JobInfo[]>("list_jobs"),
  jobLines: (id: string) => invoke<string[]>("job_lines", { id }),
  history: () => invoke<HistoryRun[]>("run_history"),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  cleanupWorktrees: (everything: boolean) =>
    invoke<{ removed: string[]; kept: number }>("cleanup_worktrees", { everything }),
  worktreeUsage: () => invoke<{ count: number; bytes: number }>("worktree_usage"),
  mergeInfo: (repo: string, number: number) => invoke<MergeInfo>("merge_info", { repo, number }),
  mergePr: (request: {
    repo: string; number: number; method: string; admin: boolean; auto: boolean;
    deleteBranch: boolean; headSha: string; subject: string; body: string;
  }) => invoke<string>("merge_pr", { request }),
  submitReview: (request: { repo: string; number: number; event: "approve" | "request_changes" | "comment"; body: string }) =>
    invoke<string>("submit_review", { request }),
  slackLookup: (repo: string, number: number, login: string, force = false) =>
    invoke<{ user: SlackUser | null; cached: boolean; emails: string[] }>("slack_lookup", { repo, number, login, force }),
  slackSend: (login: string, user: SlackUser, message: string) => invoke<string>("slack_send", { login, user, message }),
  testNotification: () => invoke<void>("test_notification"),
  slackTest: () => invoke<string>("slack_test"),
  slackForget: (login: string) => invoke<void>("slack_forget", { login }),
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
