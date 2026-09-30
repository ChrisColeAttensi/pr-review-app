import type { JobKind } from "./types";

export interface HistoryRun {
  kind: JobKind;
  lines?: number;
  files?: number;
  model?: string;
  durationMs?: number;
  costUsd?: number;
  finishedAt: number;
}

export interface Estimate {
  seconds: number;
  costUsd?: number;
  /** How many past runs it is based on; 0 means a built-in guess. */
  basis: number;
}

const MIN_SAMPLES = 3;
const NEIGHBOURS = 5;

// pr-review skips its agents below this size, so the two sides of the line take very
// different times and must not be mixed.
const smallReview = (lines: number, files: number) => files < 5 && lines < 150;

function fallback(kind: JobKind, lines: number, files: number): number {
  if (kind === "insights") return Math.min(600, 60 + 0.25 * lines);
  return smallReview(lines, files) ? 300 : Math.min(1800, 600 + 0.5 * lines);
}

const median = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};

export function estimate(
  history: HistoryRun[],
  kind: JobKind,
  lines: number,
  files: number,
  model: string,
): Estimate {
  let runs = history.filter((r) => r.kind === kind && r.durationMs && r.lines != null);
  if (kind === "review") {
    const small = smallReview(lines, files);
    runs = runs.filter((r) => smallReview(r.lines!, r.files ?? 0) === small);
  }
  // Prefer runs on the same model, when there are enough of them.
  const sameModel = runs.filter((r) => (r.model ?? "") === model);
  if (sameModel.length >= MIN_SAMPLES) runs = sameModel;
  if (runs.length < MIN_SAMPLES) return { seconds: fallback(kind, lines, files), basis: 0 };

  // Nearest by size on a log scale: 40 vs 80 lines is as far apart as 400 vs 800.
  const target = Math.log(lines + 1);
  const near = runs
    .map((r) => ({ r, d: Math.abs(Math.log(r.lines! + 1) - target) }))
    .sort((a, b) => a.d - b.d)
    .slice(0, NEIGHBOURS)
    .map((x) => x.r);
  const costs = near.map((r) => r.costUsd).filter((c): c is number => c != null);
  return {
    seconds: median(near.map((r) => r.durationMs! / 1000)),
    costUsd: costs.length ? median(costs) : undefined,
    basis: near.length,
  };
}

export function formatDuration(seconds: number): string {
  if (seconds < 90) return "~1 min";
  const m = Math.round(seconds / 60);
  if (m < 60) return `~${m} min`;
  return `~${(m / 60).toFixed(1).replace(/\.0$/, "")} h`;
}

export function formatEstimate(e: Estimate): string {
  const parts = [formatDuration(e.seconds)];
  if (e.costUsd != null) parts.push(`~$${e.costUsd < 1 ? e.costUsd.toFixed(2) : e.costUsd.toFixed(1)}`);
  return parts.join(" · ");
}

export function estimateTooltip(e: Estimate): string {
  return e.basis
    ? `Based on your ${e.basis} most similar past runs`
    : `Rough guess. Estimates come from your own runs once you have ${MIN_SAMPLES}.`;
}
