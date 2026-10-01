import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type SlackUser } from "../api";
import type { Pr, Settings } from "../types";
import { useToasts } from "../toasts";
import { mod } from "../platform";

/** Send without blocking: a toast tracks it, with Retry and Copy if it fails. */
function sendInBackground(toasts: ReturnType<typeof useToasts>, login: string, user: SlackUser, message: string, replace?: number) {
  const first = user.name.split(/\s+/)[0];
  const id = toasts.show({ text: `Sending Slack message to ${first}…`, tone: "info", busy: true }, replace);
  api.slackSend(login, user, message).then(
    (link) =>
      toasts.show(
        { text: `Sent to ${first} on Slack ✓`, tone: "ok", actions: link ? [{ label: "Open in Slack", run: () => openUrl(link) }] : [] },
        id,
      ),
    (e) =>
      toasts.show(
        {
          text: `Couldn't message ${first}: ${String(e)}`,
          tone: "error",
          actions: [
            { label: "Retry", run: () => sendInBackground(toasts, login, user, message) },
            { label: "Copy message", run: () => navigator.clipboard.writeText(message) },
          ],
        },
        id,
      ),
  );
}

export type SlackReason = "approve" | "merge";

function fill(template: string, pr: Pr, user?: SlackUser) {
  const name = user?.name || pr.author;
  const vars: Record<string, string> = {
    first: name.split(/\s+/)[0], name, title: pr.title, url: pr.url, repo: pr.repo, number: String(pr.number),
  };
  return template.replace(/\{(\w+)\}/g, (m, k) => vars[k] ?? m);
}

/** Offered after approving or merging someone else's PR. Nothing is sent without a click. */
export function SlackPrompt({ pr, reason, settings, onClose }: {
  pr: Pr; reason: SlackReason; settings: Settings; onClose: () => void;
}) {
  const template = reason === "approve" ? settings.slackApproveTemplate : settings.slackMergeTemplate;
  const [user, setUser] = useState<SlackUser>();
  const [cached, setCached] = useState(false);
  const [phase, setPhase] = useState<"finding" | "found" | "missing" | "error">("finding");
  const [err, setErr] = useState<string>();
  const [message, setMessage] = useState(fill(template, pr));
  const [edited, setEdited] = useState(false);

  const find = (force = false) => {
    setPhase("finding");
    setErr(undefined);
    api.slackLookup(pr.repo, pr.number, pr.author, force)
      .then((r) => {
        setCached(r.cached);
        if (r.user) {
          setUser(r.user);
          if (!edited) setMessage(fill(template, pr, r.user));
          setPhase("found");
        } else setPhase("missing");
      })
      .catch((e) => { setErr(String(e)); setPhase("error"); });
  };
  useEffect(() => find(), []);

  const toasts = useToasts();
  const send = () => {
    if (!user) return;
    sendInBackground(toasts, pr.author, user, message);
    onClose();
  };

  const copyAndOpen = async () => {
    await navigator.clipboard.writeText(message);
    openUrl("slack://open");
    onClose();
  };

  return (
    <div className="modal-back" onClick={onClose}>
      <div className="modal merge-modal" onClick={(e) => e.stopPropagation()}>
        <header className="row">
          <h2>Message {user?.name ?? pr.author} on Slack?</h2>
        </header>
        <div className="modal-body">
          <div className="slack-who">
            {phase === "finding" && <><span className="spinner" /> Finding <b>{pr.author}</b> on Slack… <span className="muted small">only the first time; it's remembered after</span></>}
            {user && phase !== "finding" && (
              <>
                <b>{user.name}</b>
                {user.title && <span className="muted"> · {user.title}</span>}
                {user.email && <span className="muted small"> · {user.email}</span>}
                <button className="link" onClick={() => { api.slackForget(pr.author); setUser(undefined); find(true); }}>Not them?</button>
              </>
            )}
            {phase === "missing" && (
              <>
                <span className="muted">
                  Couldn't match {pr.author} to a Slack user{cached ? " (remembered from earlier today)" : ""}. You can still copy the message.
                </span>
                <button className="link" onClick={() => find(true)}>Search again</button>
              </>
            )}
          </div>
          <textarea rows={4} value={message} 
            onChange={(e) => { setMessage(e.target.value); setEdited(true); }}
            onKeyDown={(e) => { if (e.key === "Enter" && mod(e) && phase === "found") send(); }} />
          <p className="muted small">Sent as you, through your Claude Slack connector. It sends in the background; a toast tells you when it's done.</p>
          {err && <div className="banner error">{err}</div>}
        </div>
        <footer className="row end modal-foot">
          <button onClick={onClose}>Skip</button>
          <button onClick={copyAndOpen}>Copy & open Slack</button>
          <button className="primary" disabled={phase !== "found" || !message.trim()} onClick={send}>
            Send
          </button>
        </footer>
      </div>
    </div>
  );
}
