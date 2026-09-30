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
  const settings = {
    pollIntervalSecs: 120, notifications: true,
    categories: { reviewRequested: true, reReview: true, watched: true, mine: true },
    notifyCategories: { reviewRequested: true, reReview: true, watched: false, mine: true },
    actionableOnly: true, showDrafts: false, directRequestsOnly: false, watchedRepos: [], repoRoots: ["~/Developer"], repoPaths: {},
    claudePath: "", reviewCommand: "/chriscole-skills:pr-review {url}", permissionMode: "auto",
    reviewModel: "", insightsModel: "", autoInsights: false, worktreeRetentionDays: 7,
    slackOnApprove: true, slackOnMerge: true,
    slackApproveTemplate: "Hey {first}, I've approved your PR [{repo}#{number}: {title}]({url}) 👍",
    slackMergeTemplate: "Hey {first}, I've merged your PR [{repo}#{number}: {title}]({url}) 🚀",
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
        case "slack_send": await new Promise((r) => setTimeout(r, 1000)); return "https://attensi.slack.com/archives/D0/p1";
        case "slack_forget": return null;
        case "slack_test": return "";
        case "submit_review": await new Promise((r) => setTimeout(r, 500)); return "";
        case "merge_pr": await new Promise((r) => setTimeout(r, 600)); return "";
        case "worktree_usage": return { count: 3, bytes: 1.4e9 };
        case "cleanup_worktrees": return { removed: args.everything ? ["Attensi/impact#817 (removed by you)"] : [], kept: args.everything ? 0 : 3 };
        case "run_history": return [
          { kind: "insights", lines: 180, files: 5, model: "", durationMs: 95000, costUsd: 0.31, finishedAt: 1 },
          { kind: "insights", lines: 240, files: 4, model: "", durationMs: 130000, costUsd: 0.42, finishedAt: 2 },
          { kind: "insights", lines: 90, files: 2, model: "", durationMs: 70000, costUsd: 0.2, finishedAt: 3 },
        ];
        case "set_autostart": return args.enabled;
        case "start_job": {
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
