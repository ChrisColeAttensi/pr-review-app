import { useEffect, useState } from "react";
import { api } from "../api";
import type { Categories, Settings } from "../types";
import { TRAY, isMac } from "../platform";

const CATS: { id: keyof Categories; label: string; hint: string }[] = [
  { id: "reviewRequested", label: "Review requested from me", hint: "Someone asks you (or your team) to review" },
  { id: "reReview", label: "New commits on PRs I reviewed", hint: "The head moved past the commit you reviewed" },
  { id: "mine", label: "My own PRs", hint: "CI finishes, or someone reviews" },
  { id: "watched", label: "Watched repos", hint: "Any new PR in the repos listed below" },
];

const lines = (s: string) => s.split("\n").map((x) => x.trim()).filter(Boolean);

export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const [s, setS] = useState<Settings>();
  const [found, setFound] = useState<Record<string, string>>();
  const [overrideRepo, setOverrideRepo] = useState("");
  const [overridePath, setOverridePath] = useState("");
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string>();
  const [autostart, setAutostart] = useState<boolean>();
  const [usage, setUsage] = useState<{ count: number; bytes: number }>();
  const [cleaning, setCleaning] = useState(false);
  const [cleanMsg, setCleanMsg] = useState<string>();
  const [slackMsg, setSlackMsg] = useState<string>();
  const [notifyMsg, setNotifyMsg] = useState<string>();

  useEffect(() => {
    api.settings().then(setS);
    api.discoverRepos().then(setFound);
    api.worktreeUsage().then(setUsage).catch(() => {});
    api.getAutostart().then(setAutostart).catch(() => setAutostart(false));
  }, []);
  if (!s) return null;
  const set = (patch: Partial<Settings>) => setS({ ...s, ...patch });

  const clean = async (everything: boolean) => {
    setCleaning(true);
    setCleanMsg(undefined);
    try {
      const r = await api.cleanupWorktrees(everything);
      setCleanMsg(r.removed.length ? `Removed ${r.removed.length}: ${r.removed.join(", ")}` : "Nothing to remove.");
      setUsage(await api.worktreeUsage());
    } catch (e) {
      setCleanMsg(String(e));
    } finally {
      setCleaning(false);
    }
  };
  const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);

  const save = async () => {
    setSaving(true);
    try {
      await api.saveSettings(s);
      onClose();
    } catch (e) {
      setErr(String(e));
      setSaving(false);
    }
  };

  return (
    <div className="modal-back" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <header className="row">
          <h2>Settings</h2>
          <span className="spacer" />
          <button onClick={onClose}>Cancel</button>
          <button className="primary" onClick={save} disabled={saving}>Save</button>
        </header>
        {err && <div className="banner error">{err}</div>}
        <div className="modal-body">
          <section>
            <h3>General</h3>
            <label className="check">
              <input
                type="checkbox"
                disabled={autostart === undefined}
                checked={!!autostart}
                onChange={(e) =>
                  api.setAutostart(e.target.checked).then(setAutostart).catch((x) => setErr(String(x)))
                }
              />
              Launch at login <span className="muted small">starts quietly in the {TRAY}; applies immediately</span>
            </label>
          </section>

          <section>
            <h3>Monitoring</h3>
            <table className="cat-table">
              <thead>
                <tr><th /><th>Show</th><th>Notify</th></tr>
              </thead>
              <tbody>
                {CATS.map((c) => (
                  <tr key={c.id}>
                    <td><div>{c.label}</div><div className="muted small">{c.hint}</div></td>
                    <td><input type="checkbox" checked={s.categories[c.id]} onChange={(e) => set({ categories: { ...s.categories, [c.id]: e.target.checked } })} /></td>
                    <td><input type="checkbox" disabled={!s.categories[c.id] || !s.notifications} checked={s.notifyCategories[c.id]}
                      onChange={(e) => set({ notifyCategories: { ...s.notifyCategories, [c.id]: e.target.checked } })} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="row">
              <label className="check"><input type="checkbox" checked={s.notifications} onChange={(e) => set({ notifications: e.target.checked })} /> Desktop notifications</label>
              <span className="spacer" />
              <button onClick={() => { api.testNotification(); setNotifyMsg(isMac
                ? "Arrives in 5 seconds: switch to another app now. macOS only shows banners when PR Review is in the background. If it lands only in Notification Center, set PR Review to Banners or Alerts in System Settings → Notifications."
                : "Arrives in 5 seconds. If no banner appears, check that notifications for PR Review are on in Settings → System → Notifications, and that Do Not Disturb is off."); }}>Send test notification</button>
            </div>
            {notifyMsg && <p className="muted small">{notifyMsg}</p>}
            <label className="check"><input type="checkbox" checked={s.directRequestsOnly} onChange={(e) => set({ directRequestsOnly: e.target.checked })} /> Hide review requests that only come via a team (also in the sidebar as “Team requests”)</label>
            <label className="field">
              <span>Check every</span>
              <select value={s.pollIntervalSecs} onChange={(e) => set({ pollIntervalSecs: Number(e.target.value) })}>
                {[60, 120, 300, 600, 1800].map((n) => <option key={n} value={n}>{n < 120 ? `${n}s` : `${n / 60} min`}</option>)}
              </select>
            </label>
            <label className="field col">
              <span>Watched repos <span className="muted small">one <code>owner/repo</code> per line</span></span>
              <textarea rows={3} defaultValue={s.watchedRepos.join("\n")} onChange={(e) => set({ watchedRepos: lines(e.target.value) })} />
            </label>
          </section>

          <section>
            <h3>Repositories</h3>
            <p className="muted small">Reviews run in a throwaway git worktree of your local clone, at the PR head. Clones are found by scanning these folders two levels deep.</p>
            <label className="field col">
              <span>Folders to scan <span className="muted small">one per line</span></span>
              <textarea rows={2} defaultValue={s.repoRoots.join("\n")} onChange={(e) => set({ repoRoots: lines(e.target.value) })} />
            </label>
            <details>
              <summary>{found ? `${Object.keys(found).length} clones found` : "Scanning…"}</summary>
              <ul className="repo-list">
                {found && Object.entries(found).map(([r, p]) => <li key={r}><code>{r}</code> <span className="muted small">{p}</span></li>)}
              </ul>
            </details>
            <div className="field col">
              <span>Overrides</span>
              {Object.entries(s.repoPaths).map(([r, p]) => (
                <div key={r} className="row">
                  <code>{r}</code> → <span className="muted small">{p}</span>
                  <button className="link danger" onClick={() => { const rp = { ...s.repoPaths }; delete rp[r]; set({ repoPaths: rp }); }}>remove</button>
                </div>
              ))}
              <div className="row">
                <input placeholder="owner/repo" value={overrideRepo} onChange={(e) => setOverrideRepo(e.target.value)} />
                <input placeholder="~/path/to/clone" value={overridePath} onChange={(e) => setOverridePath(e.target.value)} />
                <button disabled={!overrideRepo || !overridePath}
                  onClick={() => { set({ repoPaths: { ...s.repoPaths, [overrideRepo.trim().toLowerCase()]: overridePath.trim() } }); setOverrideRepo(""); setOverridePath(""); }}>Add</button>
              </div>
            </div>
          </section>

          <section>
            <h3>Review worktrees</h3>
            <p className="muted small">
              Each review checks the PR out into its own worktree. These are removed automatically once the PR is merged or
              closed, or after they sit unused. Reports, walkthroughs and your notes are kept.
            </p>
            <label className="field">
              <span>Remove unused after</span>
              <select value={s.worktreeRetentionDays} onChange={(e) => set({ worktreeRetentionDays: Number(e.target.value) })}>
                {[1, 3, 7, 14, 30].map((d) => <option key={d} value={d}>{d} day{d > 1 ? "s" : ""}</option>)}
                <option value={0}>Never (only when the PR closes)</option>
              </select>
            </label>
            <div className="row">
              <span className="muted small">{usage ? `${usage.count} worktree${usage.count === 1 ? "" : "s"} · ${size(usage.bytes)}` : "Measuring…"}</span>
              <span className="spacer" />
              <button disabled={cleaning} onClick={() => clean(false)} title="Apply the rules above now (uses saved settings)">Clean up now</button>
              <button className="danger" disabled={cleaning || !usage?.count} onClick={() => clean(true)} title="Remove every worktree not in use by a running job">Remove all</button>
            </div>
            {cleanMsg && <p className="muted small">{cleanMsg}</p>}
          </section>

          <section>
            <h3>Slack</h3>
            <p className="muted small">
              After you approve or merge someone else's PR, offer to DM them. Messages go out as you through your
              Claude Slack connector; nothing is sent until you click Send.
            </p>
            <label className="check"><input type="checkbox" checked={s.slackOnApprove} onChange={(e) => set({ slackOnApprove: e.target.checked })} /> Offer after approving</label>
            <label className="check"><input type="checkbox" checked={s.slackOnMerge} onChange={(e) => set({ slackOnMerge: e.target.checked })} /> Offer after merging</label>
            <label className="field col">
              <span>Approve message</span>
              <input value={s.slackApproveTemplate} onChange={(e) => set({ slackApproveTemplate: e.target.value })} />
            </label>
            <label className="field col">
              <span>Merge message</span>
              <input value={s.slackMergeTemplate} onChange={(e) => set({ slackMergeTemplate: e.target.value })} />
            </label>
            <p className="muted small">
              <code>{"{first}"}</code> <code>{"{name}"}</code> <code>{"{title}"}</code> <code>{"{url}"}</code> <code>{"{repo}"}</code> <code>{"{number}"}</code> are filled in. Markdown links work.
            </p>
            <div className="row">
              <button disabled={slackMsg === "Sending…"} onClick={() => {
                setSlackMsg("Sending…");
                api.slackTest().then(() => setSlackMsg("Sent ✓ — check your Slack DMs with yourself.")).catch((e) => setSlackMsg(String(e)));
              }}>Send test message to myself</button>
              {slackMsg && <span className="muted small">{slackMsg}</span>}
            </div>
          </section>

          <section>
            <h3>Claude</h3>
            <label className="field">
              <span>Review command</span>
              <input value={s.reviewCommand} onChange={(e) => set({ reviewCommand: e.target.value })} />
            </label>
            <p className="muted small">Sent as the prompt to <code>claude -p</code>. <code>{"{url}"}</code>, <code>{"{number}"}</code> and <code>{"{repo}"}</code> are filled in. The app tells Claude to stop before posting, pushing or fixing.</p>
            <label className="field">
              <span>Permission mode</span>
              <select value={s.permissionMode} onChange={(e) => set({ permissionMode: e.target.value })}>
                <option value="auto">auto — classifier approves safe actions</option>
                <option value="acceptEdits">acceptEdits</option>
                <option value="dontAsk">dontAsk — deny anything not pre-allowed</option>
                <option value="bypassPermissions">bypassPermissions — no checks</option>
              </select>
            </label>
            <label className="field">
              <span>Review model</span>
              <input placeholder="default" value={s.reviewModel} onChange={(e) => set({ reviewModel: e.target.value })} />
            </label>
            <label className="field">
              <span>Explain model</span>
              <input placeholder="default (e.g. sonnet for speed)" value={s.insightsModel} onChange={(e) => set({ insightsModel: e.target.value })} />
            </label>
            <label className="field">
              <span>claude binary</span>
              <input placeholder="found on PATH" value={s.claudePath} onChange={(e) => set({ claudePath: e.target.value })} />
            </label>
            <label className="check"><input type="checkbox" checked={s.allowChromeVerify} onChange={(e) => set({ allowChromeVerify: e.target.checked })} /> Allow pr-review to verify in Chrome when needed: it decides per PR, and only opens Chrome for changes the diff and CI can't settle (needs the Claude in Chrome extension)</label>
            <label className="check"><input type="checkbox" checked={s.autoInsights} onChange={(e) => set({ autoInsights: e.target.checked })} /> Explain new review requests automatically, so the walkthrough is ready when you open them</label>
          </section>
        </div>
      </div>
    </div>
  );
}
