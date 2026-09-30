export type Category = "reviewRequested" | "reReview" | "watched" | "mine";

export interface Review {
  author: string;
  state: string;
  submittedAt?: string;
  commit?: string;
}

export interface Pr {
  key: string;
  repo: string;
  number: number;
  title: string;
  url: string;
  author: string;
  authorAvatar: string;
  isDraft: boolean;
  createdAt: string;
  updatedAt: string;
  additions: number;
  deletions: number;
  changedFiles: number;
  headSha: string;
  headRef: string;
  baseRef: string;
  reviewDecision?: string | null;
  ci?: string | null;
  reviews: Review[];
  requestedReviewers?: { name: string; isTeam: boolean; avatar: string }[];
  latestReviews?: { author: string; avatar: string; state: string }[];
  categories: Category[];
  needsReReview: boolean;
}

export interface Snapshot {
  me: string;
  prs: Pr[];
  errors: string[];
  fetchedAt: number;
  loading: boolean;
}

export interface Categories {
  reviewRequested: boolean;
  reReview: boolean;
  watched: boolean;
  mine: boolean;
}

export interface Settings {
  pollIntervalSecs: number;
  notifications: boolean;
  categories: Categories;
  notifyCategories: Categories;
  directRequestsOnly: boolean;
  watchedRepos: string[];
  repoRoots: string[];
  repoPaths: Record<string, string>;
  claudePath: string;
  reviewCommand: string;
  permissionMode: string;
  reviewModel: string;
  insightsModel: string;
  autoInsights: boolean;
  worktreeRetentionDays: number;
}

export type AnnotationKind = "explain" | "why" | "concern" | "question" | "suggestion" | "nit";

export interface Annotation {
  path: string;
  line: number;
  side: "new" | "old";
  kind: AnnotationKind;
  title: string;
  body: string;
}

export interface Chapter {
  title: string;
  explanation: string;
  files: string[];
}

export interface Insights {
  summary: string;
  risk: "low" | "medium" | "high";
  riskReason: string;
  chapters: Chapter[];
  annotations: Annotation[];
  fileSummaries: { path: string; summary: string }[];
  testingNotes?: string;
}

export interface RunMeta {
  headSha: string;
  finishedAt: number;
  costUsd?: number;
  durationMs?: number;
  ok: boolean;
}

export interface Draft {
  id: string;
  path: string;
  line: number;
  side: "new" | "old";
  body: string;
  createdAt: number;
}

export interface PrState {
  review?: string | null;
  reviewMeta?: RunMeta | null;
  insights?: Insights | null;
  insightsMeta?: RunMeta | null;
  drafts: Draft[];
}

export type JobKind = "review" | "insights";

export interface JobInfo {
  id: string;
  prKey: string;
  repo: string;
  number: number;
  kind: JobKind;
  startedAt: number;
  status: "running" | "done" | "failed" | "cancelled";
}

export interface PrDetail {
  number: number;
  title: string;
  body: string;
  url: string;
  state: string;
  isDraft: boolean;
  author: { login: string };
  headRefName: string;
  baseRefName: string;
  headRefOid: string;
  additions: number;
  deletions: number;
  changedFiles: number;
  files: { path: string; additions: number; deletions: number }[];
  commits: { oid: string; messageHeadline: string; authors: { login: string }[] }[];
  reviews: { author: { login: string }; state: string; body: string; submittedAt: string }[];
  comments: { author: { login: string }; body: string; createdAt: string }[];
  labels: { name: string; color: string }[];
  statusCheckRollup: { name?: string; context?: string; conclusion?: string; state?: string; status?: string; detailsUrl?: string; targetUrl?: string }[];
  reviewDecision?: string;
}
