import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type SlackUser } from "../api";
import type { Pr, Settings } from "../types";

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
  const [phase, setPhase] = useState<"finding" | "found" | "missing" | "sending" | "sent" | "error">("finding");
  const [err, setErr] = useState<string>();
  const [message, setMessage] = useState(fill(template, pr));
  const [edited, setEdited] = useState(false);
  const [link, setLink] = useState<string>();

  const find = () => {
    setPhase("finding");
    setErr(undefined);
    api.slackLookup(pr.repo, pr.number, pr.author)
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
  useEffect(find, []);

  const send = async () => {
    if (!user) return;
    setPhase("sending");
    setErr(undefined);
    try {
      setLink(await api.slackSend(pr.author, user, message));
      setPhase("sent");
      setTimeout(onClose, 2500);
    } catch (e) {
      setErr(String(e));
      setPhase("found");
    }
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
            {phase === "finding" && <><span className="spinner" /> Finding <b>{pr.author}</b> on Slack…</>}
            {user && phase !== "finding" && (
              <>
                <b>{user.name}</b>
                {user.title && <span className="muted"> · {user.title}</span>}
                {user.email && <span className="muted small"> · {user.email}</span>}
                {cached && (
                  <button className="link" onClick={() => { api.slackForget(pr.author); setUser(undefined); find(); }}>Not them?</button>
                )}
              </>
            )}
            {phase === "missing" && <span className="muted">Couldn't match {pr.author} to a Slack user. You can still copy the message.</span>}
          </div>
          <textarea rows={4} value={message} disabled={phase === "sending" || phase === "sent"}
            onChange={(e) => { setMessage(e.target.value); setEdited(true); }}
            onKeyDown={(e) => { if (e.key === "Enter" && e.metaKey && phase === "found") send(); }} />
          <p className="muted small">Sent as you, through your Claude Slack connector.</p>
          {phase === "sent" && (
            <div className="banner ok">Sent ✓ {link && <button className="link" onClick={() => openUrl(link)}>Open in Slack</button>}</div>
          )}
          {err && <div className="banner error">{err}</div>}
        </div>
        <footer className="row end modal-foot">
          <button onClick={onClose}>{phase === "sent" ? "Close" : "Skip"}</button>
          <button onClick={copyAndOpen} disabled={phase === "sending"}>Copy & open Slack</button>
          <button className="primary" disabled={phase !== "found" || !message.trim()} onClick={send}>
            {phase === "sending" ? "Sending…" : "Send"}
          </button>
        </footer>
      </div>
    </div>
  );
}
