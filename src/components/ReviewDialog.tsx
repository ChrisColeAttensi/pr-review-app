import { useState } from "react";
import { api } from "../api";
import type { Pr } from "../types";

type Event = "comment" | "approve" | "request_changes";

const OPTIONS: { id: Event; label: string; hint: string }[] = [
  { id: "comment", label: "Comment", hint: "Submit general feedback without explicit approval." },
  { id: "approve", label: "Approve", hint: "Submit feedback and approve merging these changes." },
  { id: "request_changes", label: "Request changes", hint: "Submit feedback that must be addressed before merging." },
];

/** The latest verdict I gave this PR, if any. */
export function myReviewState(pr: Pr, me: string): string | undefined {
  return (pr.latestReviews ?? []).find((r) => r.author === me)?.state;
}

export function ReviewButton({ pr, me, onClick }: { pr: Pr; me: string; onClick: () => void }) {
  const mine = myReviewState(pr, me);
  const label = mine === "APPROVED" ? "✓ Approved" : mine === "CHANGES_REQUESTED" ? "✕ Changes requested" : "Review…";
  return <button className={mine === "APPROVED" ? "reviewed-ok" : ""} onClick={onClick} title="Submit a review on GitHub">{label}</button>;
}

export function ReviewDialog({ pr, me, onClose, onDone }: { pr: Pr; me: string; onClose: () => void; onDone: (msg: string) => void }) {
  const own = pr.author === me;
  const [event, setEvent] = useState<Event>(own ? "comment" : "approve");
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  // GitHub requires a body for these two.
  const needsBody = event !== "approve";
  const ready = !needsBody || body.trim().length > 0;

  const go = async () => {
    setBusy(true);
    setErr(undefined);
    try {
      await api.submitReview({ repo: pr.repo, number: pr.number, event, body });
      onDone(event === "approve" ? "Approved" : event === "request_changes" ? "Changes requested" : "Review comment posted");
    } catch (e) {
      setErr(String(e).replace(/^.*?gh pr: /, ""));
      setBusy(false);
    }
  };

  return (
    <div className="modal-back" onClick={onClose}>
      <div className="modal merge-modal" onClick={(e) => e.stopPropagation()}>
        <header className="row">
          <h2>Review {pr.repo}#{pr.number}</h2>
        </header>
        <div className="modal-body">
          <textarea
            autoFocus
            rows={5}
            value={body}
            placeholder={event === "approve" ? "Leave a comment (optional)" : "Leave a comment"}
            onChange={(e) => setBody(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && e.metaKey && ready && !busy) go(); }}
          />
          <div className="review-options">
            {OPTIONS.map((o) => {
              const disabled = own && o.id !== "comment";
              return (
                <label key={o.id} className={`review-opt ${disabled ? "disabled" : ""}`}>
                  <input type="radio" name="event" disabled={disabled} checked={event === o.id} onChange={() => setEvent(o.id)} />
                  <span>
                    <b>{o.label}</b>
                    <span className="muted small">{disabled ? " — you can't approve or request changes on your own PR." : ` — ${o.hint}`}</span>
                  </span>
                </label>
              );
            })}
          </div>
          <p className="muted small">This is posted to GitHub as you. Your private notes are not included.</p>
          {err && <div className="banner error">{err}</div>}
        </div>
        <footer className="row end modal-foot">
          <button onClick={onClose} disabled={busy}>Cancel</button>
          <button className={event === "approve" ? "merge-btn" : event === "request_changes" ? "danger-fill" : "primary"} disabled={!ready || busy} onClick={go}>
            {busy ? "Submitting…" : "Submit review"}
          </button>
        </footer>
      </div>
    </div>
  );
}
