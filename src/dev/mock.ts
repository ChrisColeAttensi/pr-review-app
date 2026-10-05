// Browser-only preview: fakes the Tauri backend from local fixtures (src/dev/fixtures, gitignored)
// so the UI can be iterated on in a plain browser via `bun run dev`.
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";

const fx = import.meta.glob("./fixtures/*", { eager: true, query: "?raw", import: "default" }) as Record<string, string>;
const get = (name: string) => fx[`./fixtures/${name}`];
const json = (name: string, fallback: unknown) => (get(name) ? JSON.parse(get(name)) : fallback);

export function installMock() {
  const snapshot = json("snapshot.json", { me: "me", prs: [], errors: [], fetchedAt: Date.now(), loading: false });
  snapshot.fetchedAt = Date.now();
  for (const p of snapshot.prs) {
    const approved = p.reviewDecision === "APPROVED" || p.reviewDecision === "CHANGES_REQUESTED";
    p.needsAction = p.categories.includes("reviewRequested") || (p.categories.includes("mine") && (approved || p.ci === "FAILURE"));
  }
  let drafts: unknown[] = [];
  const chat: { role: string; text: string; ts: number }[] = [];
  const settings = {
    pollIntervalSecs: 120, notifications: true,
    categories: { reviewRequested: true, reReview: true, watched: true, mine: true },
    notifyCategories: { reviewRequested: true, reReview: true, watched: false, mine: true },
    actionableOnly: true, showDrafts: false, directRequestsOnly: false, watchedRepos: [], repoRoots: ["~/Developer"], repoPaths: {},
    claudePath: "", reviewCommand: "/chriscole-skills:pr-review {url}", permissionMode: "auto",
    reviewModel: "", insightsModel: "", autoInsights: false, allowChromeVerify: true, worktreeRetentionDays: 7, approvedGraceMinutes: 60,
    slackOnApprove: true, slackOnMerge: true,
    slackApproveTemplate: "I've approved your PR [{repo}#{number}: {title}]({url}) 👍",
    slackMergeTemplate: "I've merged your PR [{repo}#{number}: {title}]({url}) 🚀", slackDelivery: "draft",
  };

  mockIPC(
    async (cmd, args: any) => {
      switch (cmd) {
        case "get_snapshot": return snapshot;
        case "get_settings": return settings;
        case "save_settings": Object.assign(settings, args.settings); return null;
        case "discover_repos": return { "attensi/impact": "~/Developer/impact" };
        case "pr_detail": return json("detail.json", {});
        case "pr_diff": return get("diff.patch") ?? "";
        case "pr_state":
          return {
            review: get("review.md") ?? null,
            reviewMeta: get("review.md") ? { headSha: "x", finishedAt: Date.now() - 600000, costUsd: 1.42, durationMs: 380000, ok: true } : null,
            insights: json("insights.json", null),
            insightsMeta: { headSha: snapshot.prs.find((p: any) => p.number === args.number)?.headSha, finishedAt: Date.now(), ok: true },
            drafts,
          };
        case "save_drafts": drafts = args.drafts; return null;
        case "list_jobs": return [];
        case "get_autostart": return false;
        case "merge_info": return {
          viewerPermission: "ADMIN", squashMergeAllowed: true, mergeCommitAllowed: true, rebaseMergeAllowed: false,
          deleteBranchOnMerge: false, autoMergeAllowed: true,
          pullRequest: { state: "OPEN", isDraft: false, mergeable: "MERGEABLE", mergeStateStatus: "BLOCKED", headRefOid: "ca8b7d1f00",
            viewerCanMergeAsAdmin: false, viewerCanBypassRulesets: true, viewerCanEnableAutoMerge: true, autoMergeRequest: null },
        };
        case "slack_lookup": await new Promise((r) => setTimeout(r, 1200));
          return { user: { userId: "U01045HJK8X", name: "Johan Helsing", title: "Staff Unity Software Engineer", email: "johan.helsing@attensi.com" }, cached: false, emails: [] };
        case "slack_send": await new Promise((r) => setTimeout(r, 1000)); return { drafted: true, link: "slack://channel?team=T0&id=D0" };
        case "slack_forget": return null;
        case "pr_chat": return chat;
        case "pr_index": return {
          [snapshot.prs[0]?.key]: { reviewHead: snapshot.prs[0]?.headSha, verdict: "🛠️ Change before merge · 2", insightsHead: snapshot.prs[0]?.headSha, drafts: 1, chat: 2 },
          [snapshot.prs[1]?.key]: { reviewHead: "old", verdict: "✅ Approve — small and safe", insightsHead: null, drafts: 0, chat: 0 },
        };
        case "handoff": return null;
        case "set_ignored": {
          const p = snapshot.prs.find((x: any) => x.key === args.key); if (p) p.ignored = args.ignored;
          setTimeout(() => emit("prs-updated", { ...snapshot }), 10); return null;
        }
        case "slack_test": return { drafted: true, link: "" };
        case "test_notification": return null;
        case "submit_review": await new Promise((r) => setTimeout(r, 500)); return "";
        case "merge_pr": {
          await new Promise((r) => setTimeout(r, 600));
          // Like the real refresh: the merged PR leaves the list shortly after.
          setTimeout(() => {
            snapshot.prs = snapshot.prs.filter((p: any) => !(p.repo === args.request.repo && p.number === args.request.number));
            emit("prs-updated", { ...snapshot });
          }, 300);
          return "";
        }
        case "worktree_usage": return { count: 3, bytes: 1.4e9 };
        case "cleanup_worktrees": return { removed: args.everything ? ["Attensi/impact#817 (removed by you)"] : [], kept: args.everything ? 0 : 3 };
        case "run_history": return [
          { kind: "insights", lines: 180, files: 5, model: "", durationMs: 95000, costUsd: 0.31, finishedAt: 1 },
          { kind: "insights", lines: 240, files: 4, model: "", durationMs: 130000, costUsd: 0.42, finishedAt: 2 },
          { kind: "insights", lines: 90, files: 2, model: "", durationMs: 70000, costUsd: 0.2, finishedAt: 3 },
        ];
        case "set_autostart": return args.enabled;
        case "start_job": {
          if (args.kind === "chat") {
            chat.push({ role: "user", text: args.message, ts: Date.now() });
            setTimeout(() => { chat.push({ role: "assistant", text: "Here's what I found:\n\n- The **riskiest** part is the retry loop in `bootstrap.ts`.\n- I'd test a rejected refresh token by hand.", ts: Date.now() }); emit("job-done", { jobId: "c1", job: { id: "c1", prKey: `${args.repo}#${args.number}`, repo: args.repo, number: args.number, kind: "chat", startedAt: Date.now(), status: "done" } }); }, 2500);
            const job = { id: "c1", prKey: `${args.repo}#${args.number}`, repo: args.repo, number: args.number, kind: "chat", startedAt: Date.now(), status: "running" };
            setTimeout(() => emit("job-started", job), 20);
            return job;
          }
          const job = { id: `j${Date.now()}`, prKey: `${args.repo}#${args.number}`, repo: args.repo, number: args.number, kind: args.kind, startedAt: Date.now(), status: "running" };
          setTimeout(() => {
            emit("job-started", job);
            const lines = [
              { type: "app_progress", message: "Checking out PR head from ~/Developer/impact" },
              { type: "system", subtype: "init", model: "claude-opus-5-5" },
              { type: "assistant", message: { content: [{ type: "tool_use", name: "Skill", input: { skill: "chriscole-skills:pr-review" } }] } },
              { type: "assistant", message: { content: [{ type: "tool_use", name: "Bash", input: { description: "Read PR metadata and diff" } }] } },
              { type: "assistant", message: { content: [{ type: "text", text: "Diff is 203 lines over 5 files — small path, reading it myself." }] } },
              { type: "assistant", message: { content: [{ type: "tool_use", name: "Agent", input: { subagent_type: "pr-review-verifier", description: "Refute: tags cleared twice" } }] } },
            ];
            lines.forEach((l, i) => setTimeout(() => emit("job-event", { jobId: job.id, line: JSON.stringify(l) }), 400 * (i + 1)));
          }, 50);
          return job;
        }
        default: return null;
      }
    },
    { shouldMockEvents: true },
  );
}
