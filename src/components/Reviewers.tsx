import type { Pr } from "../types";

type State = "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" | "REQUESTED";

interface Row {
  name: string;
  avatar: string;
  isTeam: boolean;
  state: State;
  /** Asked again after already reviewing: the verdict shown is the old one. */
  reRequested: boolean;
}

const LABEL: Record<State, string> = {
  APPROVED: "approved",
  CHANGES_REQUESTED: "changes requested",
  COMMENTED: "commented",
  DISMISSED: "dismissed",
  REQUESTED: "awaiting review",
};
const ICON: Record<State, string> = {
  APPROVED: "✓",
  CHANGES_REQUESTED: "✕",
  COMMENTED: "💬",
  DISMISSED: "–",
  REQUESTED: "●",
};
const ORDER: State[] = ["CHANGES_REQUESTED", "REQUESTED", "APPROVED", "COMMENTED", "DISMISSED"];

export function reviewerRows(pr: Pr): Row[] {
  const rows: Row[] = (pr.latestReviews ?? [])
    .filter((r) => r.author && r.author !== pr.author)
    .map((r) => ({ name: r.author, avatar: r.avatar, isTeam: false, state: r.state as State, reRequested: false }));
  for (const q of pr.requestedReviewers ?? []) {
    const existing = rows.find((r) => r.name === q.name);
    if (existing) existing.reRequested = true;
    else rows.push({ name: q.name, avatar: q.avatar, isTeam: q.isTeam, state: "REQUESTED", reRequested: false });
  }
  const rank = (r: Row) => (r.reRequested ? 1 : ORDER.indexOf(r.state));
  return rows.sort((a, b) => rank(a) - rank(b));
}

function Avatar({ r, size = 20 }: { r: Row; size?: number }) {
  if (r.isTeam || !r.avatar)
    return <span className="avatar team" style={{ width: size, height: size, fontSize: size * 0.55 }}>{r.isTeam ? "👥" : r.name[0]}</span>;
  return <img className="avatar" src={r.avatar} alt="" style={{ width: size, height: size }} />;
}

/** Compact stack for the PR header. */
export function ReviewerStack({ pr, me }: { pr: Pr; me: string }) {
  const rows = reviewerRows(pr);
  if (!rows.length) return <span className="muted small">No reviewers</span>;
  return (
    <span className="rv-stack" title={rows.map((r) => `${r.name}${r.name === me ? " (you)" : ""}: ${r.reRequested ? "re-requested" : LABEL[r.state]}`).join("\n")}>
      {rows.slice(0, 6).map((r) => (
        <span key={r.name} className={`rv-chip rv-${r.reRequested ? "REQUESTED" : r.state}`}>
          <Avatar r={r} size={18} />
          <span className="rv-badge">{ICON[r.reRequested ? "REQUESTED" : r.state]}</span>
        </span>
      ))}
      {rows.length > 6 && <span className="muted small">+{rows.length - 6}</span>}
    </span>
  );
}

/** Full list for the Overview side column. */
export function ReviewerList({ pr, me }: { pr: Pr; me: string }) {
  const rows = reviewerRows(pr);
  return (
    <div className="card">
      <h3>Reviewers</h3>
      {!rows.length && <p className="muted small">Nobody is set to review.</p>}
      <ul className="rv-list">
        {rows.map((r) => (
          <li key={r.name}>
            <Avatar r={r} />
            <span className="rv-name">
              {r.name}
              {r.name === me && <span className="badge tiny">you</span>}
            </span>
            <span className={`rv-state rv-${r.reRequested ? "REQUESTED" : r.state}`}>
              {r.reRequested ? `re-requested · was ${LABEL[r.state]}` : `${ICON[r.state]} ${LABEL[r.state]}`}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
