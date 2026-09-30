import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { Pr } from "../types";

export interface MergeInfo {
  viewerPermission: string;
  squashMergeAllowed: boolean;
  mergeCommitAllowed: boolean;
  rebaseMergeAllowed: boolean;
  deleteBranchOnMerge: boolean;
  autoMergeAllowed: boolean;
  pullRequest: {
    state: string;
    isDraft: boolean;
    mergeable: string;
    mergeStateStatus: string;
    headRefOid: string;
    viewerCanMergeAsAdmin: boolean;
    viewerCanEnableAutoMerge: boolean;
    autoMergeRequest: { enabledAt: string; mergeMethod: string } | null;
  };
}

type Method = "squash" | "merge" | "rebase";
const METHOD_LABEL: Record<Method, string> = { squash: "Squash and merge", merge: "Create a merge commit", rebase: "Rebase and merge" };

type Tone = "ok" | "warn" | "bad" | "muted";
/** What GitHub's merge box would say, and whether a plain merge is allowed. */
function status(info: MergeInfo): { text: string; tone: Tone; canMerge: boolean } {
  const pr = info.pullRequest;
  if (pr.state === "MERGED") return { text: "Merged", tone: "muted", canMerge: false };
  if (pr.state === "CLOSED") return { text: "Closed", tone: "muted", canMerge: false };
  if (pr.autoMergeRequest) return { text: "Auto-merge is on — it merges once requirements are met", tone: "ok", canMerge: false };
  switch (pr.mergeStateStatus) {
    case "CLEAN":
    case "HAS_HOOKS":
      return { text: "Ready to merge", tone: "ok", canMerge: true };
    case "UNSTABLE":
      return { text: "Some non-required checks are failing, but it can merge", tone: "warn", canMerge: true };
    case "BLOCKED":
      return { text: "Blocked: required reviews or checks are missing", tone: "bad", canMerge: false };
    case "BEHIND":
      return { text: "Branch is behind its base; update it first", tone: "bad", canMerge: false };
    case "DIRTY":
      return { text: "Merge conflicts must be resolved first", tone: "bad", canMerge: false };
    case "DRAFT":
      return { text: "Draft — mark it ready for review first", tone: "muted", canMerge: false };
    default:
      return { text: "GitHub is still checking whether this can merge…", tone: "muted", canMerge: false };
  }
}

const canWrite = (info: MergeInfo) => ["ADMIN", "MAINTAIN", "WRITE"].includes(info.viewerPermission);
// Conflicts and drafts stop an admin merge too.
const canBypass = (info: MergeInfo) =>
  info.pullRequest.viewerCanMergeAsAdmin && !["CLEAN", "HAS_HOOKS", "DIRTY", "DRAFT"].includes(info.pullRequest.mergeStateStatus);
const canAuto = (info: MergeInfo) =>
  info.autoMergeAllowed && info.pullRequest.viewerCanEnableAutoMerge && !info.pullRequest.autoMergeRequest;

export function useMergeInfo(pr: Pr) {
  const [info, setInfo] = useState<MergeInfo>();
  const [err, setErr] = useState<string>();
  const load = useCallback(() => {
    setErr(undefined);
    api.mergeInfo(pr.repo, pr.number).then(setInfo).catch((e) => setErr(String(e)));
  }, [pr.repo, pr.number]);
  // Reload when the head or CI moves, since both change mergeability.
  useEffect(load, [load, pr.headSha, pr.ci, pr.reviewDecision]);
  // GitHub computes mergeability lazily; ask again shortly if it hasn't yet.
  useEffect(() => {
    if (info?.pullRequest.mergeStateStatus !== "UNKNOWN") return;
    const t = setTimeout(load, 4000);
    return () => clearTimeout(t);
  }, [info, load]);
  return { info, err, reload: load };
}

/** Header button: green when it can merge now, neutral when only an override or auto-merge would. */
export function MergeButton({ info, onClick }: { info?: MergeInfo; onClick: () => void }) {
  if (!info || !canWrite(info) || info.pullRequest.state !== "OPEN") return null;
  const st = status(info);
  if (!st.canMerge && !canBypass(info) && !canAuto(info)) return null;
  return (
    <button className={st.canMerge ? "merge-btn" : ""} onClick={onClick} title={st.text}>
      {st.canMerge ? "Merge…" : canBypass(info) ? "Merge (override)…" : "Enable auto-merge…"}
    </button>
  );
}

export function MergeCard({ info, err, onMerge }: { info?: MergeInfo; err?: string; onMerge: () => void }) {
  if (err) return <div className="card"><h3>Merge</h3><p className="muted small">{err}</p></div>;
  if (!info) return <div className="card"><h3>Merge</h3><p className="muted small">Checking…</p></div>;
  const st = status(info);
  const actionable = canWrite(info) && info.pullRequest.state === "OPEN" && (st.canMerge || canBypass(info) || canAuto(info));
  return (
    <div className="card">
      <h3>Merge</h3>
      <p className={`merge-status tone-${st.tone}`}>{st.text}</p>
      {!canWrite(info) && <p className="muted small">You don't have write access to this repo.</p>}
      {actionable && (
        <button className={st.canMerge ? "merge-btn" : ""} onClick={onMerge}>
          {st.canMerge ? "Merge…" : canBypass(info) ? "Merge with override…" : "Enable auto-merge…"}
        </button>
      )}
    </div>
  );
}

function allowed(info: MergeInfo): Method[] {
  return (["squash", "merge", "rebase"] as Method[]).filter((m) =>
    m === "squash" ? info.squashMergeAllowed : m === "merge" ? info.mergeCommitAllowed : info.rebaseMergeAllowed,
  );
}

function rememberedMethod(repo: string, options: Method[]): Method {
  try {
    const m = localStorage.getItem(`mergeMethod:${repo}`) as Method | null;
    if (m && options.includes(m)) return m;
  } catch {}
  return options[0];
}

export function MergeDialog({ pr, info, onClose, onMerged }: {
  pr: Pr; info: MergeInfo; onClose: () => void; onMerged: (msg: string) => void;
}) {
  const options = allowed(info);
  const st = status(info);
  const [method, setMethod] = useState<Method>(rememberedMethod(pr.repo, options));
  const defaultSubject = (m: Method) =>
    m === "squash" ? `${pr.title} (#${pr.number})` : `Merge pull request #${pr.number} from ${pr.repo.split("/")[0]}/${pr.headRef}`;
  const [subject, setSubject] = useState(defaultSubject(method));
  const [body, setBody] = useState(method === "merge" ? pr.title : "");
  const [deleteBranch, setDeleteBranch] = useState(!info.deleteBranchOnMerge);
  const [admin, setAdmin] = useState(false);
  const [auto, setAuto] = useState(!st.canMerge && !canBypass(info) && canAuto(info));
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();

  const pick = (m: Method) => {
    setMethod(m);
    setSubject(defaultSubject(m));
    setBody(m === "merge" ? pr.title : "");
  };
  // A plain merge is only possible when GitHub says so; otherwise one of the two
  // options has to be ticked.
  const ready = st.canMerge || admin || auto;
  const verb = auto ? "Enable auto-merge" : `Confirm ${METHOD_LABEL[method].toLowerCase()}`;

  const go = async () => {
    setBusy(true);
    setErr(undefined);
    try {
      await api.mergePr({
        repo: pr.repo, number: pr.number, method, admin, auto: auto && !admin,
        deleteBranch, headSha: info.pullRequest.headRefOid, subject, body,
      });
      try { localStorage.setItem(`mergeMethod:${pr.repo}`, method); } catch {}
      onMerged(auto && !admin ? "Auto-merge enabled" : "Merged");
    } catch (e) {
      setErr(String(e).replace(/^.*?gh pr: /, ""));
      setBusy(false);
    }
  };

  return (
    <div className="modal-back" onClick={onClose}>
      <div className="modal merge-modal" onClick={(e) => e.stopPropagation()}>
        <header className="row">
          <h2>Merge {pr.repo}#{pr.number}</h2>
        </header>
        <div className="modal-body">
          <p className={`merge-status tone-${st.tone}`}>{st.text}</p>
          <div className="seg method-seg">
            {options.map((m) => (
              <button key={m} className={method === m ? "on" : ""} onClick={() => pick(m)}>{METHOD_LABEL[m]}</button>
            ))}
          </div>
          {method !== "rebase" && (
            <>
              <label className="field col">
                <span>Commit message</span>
                <input value={subject} onChange={(e) => setSubject(e.target.value)} />
              </label>
              <label className="field col">
                <span>Extended description <span className="muted small">optional</span></span>
                <textarea rows={4} value={body} onChange={(e) => setBody(e.target.value)} />
              </label>
            </>
          )}
          {info.deleteBranchOnMerge ? (
            <p className="muted small">This repo deletes the head branch automatically after merge.</p>
          ) : (
            <label className="check"><input type="checkbox" checked={deleteBranch} onChange={(e) => setDeleteBranch(e.target.checked)} /> Delete <code>{pr.headRef}</code> after merge</label>
          )}
          {canBypass(info) && (
            <label className="check override">
              <input type="checkbox" checked={admin} onChange={(e) => { setAdmin(e.target.checked); if (e.target.checked) setAuto(false); }} />
              <span><b>Merge without waiting for requirements to be met</b> (bypass branch rules as admin)</span>
            </label>
          )}
          {!st.canMerge && canAuto(info) && (
            <label className="check">
              <input type="checkbox" checked={auto} disabled={admin} onChange={(e) => setAuto(e.target.checked)} />
              Enable auto-merge: GitHub merges it once requirements are met
            </label>
          )}
          <p className="muted small">
            Merges head <code>{info.pullRequest.headRefOid.slice(0, 7)}</code> only. If someone pushes in the meantime, GitHub refuses the merge.
          </p>
          {err && <div className="banner error">{err}</div>}
        </div>
        <footer className="row end modal-foot">
          <button onClick={onClose} disabled={busy}>Cancel</button>
          <button className={admin ? "danger-fill" : "merge-btn"} disabled={!ready || busy} onClick={go}>
            {busy ? "Merging…" : admin ? `${verb} (override)` : verb}
          </button>
        </footer>
      </div>
    </div>
  );
}
