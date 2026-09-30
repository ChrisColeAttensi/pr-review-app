import { memo, useMemo, useState, type ReactNode } from "react";
import { Diff, Hunk, getChangeKey, tokenize, type ChangeData, type FileData, type HunkData } from "react-diff-view";
import { refractor } from "refractor";
import { Markdown } from "./Markdown";
import type { Annotation, AnnotationKind, Draft } from "../types";

export const filePath = (f: FileData) => (f.type === "delete" ? f.oldPath : f.newPath);

const LANG: Record<string, string> = {
  ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx", mjs: "javascript", cjs: "javascript",
  cs: "csharp", py: "python", rs: "rust", go: "go", json: "json", css: "css", scss: "scss",
  html: "markup", xml: "markup", svg: "markup", vue: "markup", csproj: "markup", md: "markdown",
  yml: "yaml", yaml: "yaml", sh: "bash", zsh: "bash", swift: "swift", kt: "kotlin", java: "java",
  sql: "sql", rb: "ruby", php: "php", toml: "toml", dockerfile: "docker", graphql: "graphql",
};

function languageOf(path: string): string | undefined {
  const base = path.split("/").pop()!.toLowerCase();
  const ext = base === "dockerfile" ? "dockerfile" : base.split(".").pop() ?? "";
  const lang = LANG[ext];
  return lang && refractor.registered(lang) ? lang : undefined;
}

const highlighter = {
  highlight: (text: string, lang: string) => refractor.highlight(text, lang).children as any,
};

function changeLine(c: ChangeData): { line: number; side: "new" | "old" } {
  if (c.type === "insert") return { line: c.lineNumber, side: "new" };
  if (c.type === "delete") return { line: c.lineNumber, side: "old" };
  return { line: c.newLineNumber, side: "new" };
}

/** Nearest change to a line on a side; notes the model anchored slightly off still land. */
export function anchorKey(hunks: HunkData[], line: number, side: "new" | "old"): string | null {
  let best: ChangeData | null = null;
  let bestDist = Infinity;
  for (const h of hunks) {
    for (const c of h.changes) {
      let n: number | null = null;
      if (side === "new") n = c.type === "insert" ? c.lineNumber : c.type === "normal" ? c.newLineNumber : null;
      else n = c.type === "delete" ? c.lineNumber : c.type === "normal" ? c.oldLineNumber : null;
      if (n == null) continue;
      const d = Math.abs(n - line);
      if (d < bestDist) {
        bestDist = d;
        best = c;
      }
    }
  }
  return best && bestDist <= 15 ? getChangeKey(best) : null;
}

export const KIND_LABEL: Record<AnnotationKind, string> = {
  explain: "What", why: "Why", concern: "Concern", question: "Question", suggestion: "Suggestion", nit: "Nit",
};

export function AnnotationCard({ a, onReply }: { a: Annotation; onReply?: () => void }) {
  const [open, setOpen] = useState(a.kind === "concern" || a.kind === "question" || a.kind === "suggestion" || a.kind === "explain" || a.kind === "why");
  return (
    <div className={`note note-${a.kind}`}>
      <div className="note-head" onClick={() => setOpen(!open)}>
        <span className={`kind kind-${a.kind}`}>{KIND_LABEL[a.kind]}</span>
        <span className="note-title">{a.title}</span>
        <span className="note-src">Claude · private</span>
        {onReply && (
          <button className="link" onClick={(e) => { e.stopPropagation(); onReply(); }}>
            + note
          </button>
        )}
      </div>
      {open && <Markdown>{a.body}</Markdown>}
    </div>
  );
}

function Composer({ initial = "", onSave, onCancel }: { initial?: string; onSave: (b: string) => void; onCancel: () => void }) {
  const [body, setBody] = useState(initial);
  return (
    <div className="composer">
      <textarea
        autoFocus
        value={body}
        placeholder="Private review note (never posted). Markdown ok. ⌘↩ to save"
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && e.metaKey && body.trim()) onSave(body.trim());
          if (e.key === "Escape") onCancel();
        }}
      />
      <div className="row end">
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" disabled={!body.trim()} onClick={() => onSave(body.trim())}>
          Save note
        </button>
      </div>
    </div>
  );
}

export function DraftCard({ d, onDelete, onEdit }: { d: Draft; onDelete: () => void; onEdit: (b: string) => void }) {
  const [editing, setEditing] = useState(false);
  if (editing)
    return <Composer initial={d.body} onCancel={() => setEditing(false)} onSave={(b) => { onEdit(b); setEditing(false); }} />;
  return (
    <div className="note note-draft">
      <div className="note-head">
        <span className="kind kind-draft">My note</span>
        <span className="note-title" />
        <button className="link" onClick={() => navigator.clipboard.writeText(d.body)}>Copy</button>
        <button className="link" onClick={() => setEditing(true)}>Edit</button>
        <button className="link danger" onClick={onDelete}>Delete</button>
      </div>
      <Markdown>{d.body}</Markdown>
    </div>
  );
}

interface Props {
  file: FileData;
  viewType: "unified" | "split";
  annotations: Annotation[];
  drafts: Draft[];
  summary?: string;
  onAddDraft: (d: Omit<Draft, "id" | "createdAt">) => void;
  onEditDraft: (id: string, body: string) => void;
  onDeleteDraft: (id: string) => void;
  defaultCollapsed?: boolean;
  id?: string;
}

function DiffFileInner({ file, viewType, annotations, drafts, summary, onAddDraft, onEditDraft, onDeleteDraft, defaultCollapsed, id }: Props) {
  const path = filePath(file);
  const [collapsed, setCollapsed] = useState(!!defaultCollapsed);
  const [composing, setComposing] = useState<string | null>(null);
  const [showNotes, setShowNotes] = useState(true);

  const adds = file.hunks.reduce((n, h) => n + h.changes.filter((c) => c.type === "insert").length, 0);
  const dels = file.hunks.reduce((n, h) => n + h.changes.filter((c) => c.type === "delete").length, 0);
  const size = adds + dels;

  const tokens = useMemo(() => {
    const language = languageOf(path);
    if (!language || collapsed || size > 4000) return undefined;
    try {
      return tokenize(file.hunks, { highlight: true, refractor: highlighter as any, language });
    } catch {
      return undefined;
    }
  }, [file.hunks, path, collapsed, size]);

  // Resolve every note to a change key once.
  const { byKey, orphans, changeByKey } = useMemo(() => {
    const changeByKey = new Map<string, ChangeData>();
    for (const h of file.hunks) for (const c of h.changes) changeByKey.set(getChangeKey(c), c);
    const byKey = new Map<string, { notes: Annotation[]; drafts: Draft[] }>();
    const orphans: Annotation[] = [];
    const slot = (k: string) => {
      if (!byKey.has(k)) byKey.set(k, { notes: [], drafts: [] });
      return byKey.get(k)!;
    };
    for (const a of annotations) {
      const k = anchorKey(file.hunks, a.line, a.side);
      if (k) slot(k).notes.push(a);
      else orphans.push(a);
    }
    for (const d of drafts) {
      const k = anchorKey(file.hunks, d.line, d.side);
      if (k) slot(k).drafts.push(d);
    }
    return { byKey, orphans, changeByKey };
  }, [annotations, drafts, file.hunks]);

  const widgets = useMemo(() => {
    const w: Record<string, ReactNode> = {};
    const keys = new Set([...(showNotes ? byKey.keys() : []), ...(composing ? [composing] : [])]);
    for (const k of keys) {
      const slot = byKey.get(k);
      w[k] = (
        <div className="widget">
          {showNotes && slot?.notes.map((a, i) => <AnnotationCard key={i} a={a} onReply={() => setComposing(k)} />)}
          {showNotes && slot?.drafts.map((d) => (
            <DraftCard key={d.id} d={d} onDelete={() => onDeleteDraft(d.id)} onEdit={(b) => onEditDraft(d.id, b)} />
          ))}
          {composing === k && (
            <Composer
              onCancel={() => setComposing(null)}
              onSave={(body) => {
                const c = changeByKey.get(k)!;
                onAddDraft({ path, body, ...changeLine(c) });
                setComposing(null);
              }}
            />
          )}
        </div>
      );
    }
    return w;
  }, [byKey, composing, showNotes, changeByKey, onAddDraft, onDeleteDraft, onEditDraft, path]);

  const noteCount = annotations.length;
  const concernCount = annotations.filter((a) => a.kind === "concern").length;

  return (
    <section className="diff-file" id={id}>
      <header className="diff-file-head" onClick={() => setCollapsed(!collapsed)}>
        <span className="chev">{collapsed ? "▸" : "▾"}</span>
        <span className={`ftype ftype-${file.type}`}>{file.type === "add" ? "A" : file.type === "delete" ? "D" : file.type === "rename" ? "R" : "M"}</span>
        <span className="fpath">
          {file.type === "rename" && <span className="muted">{file.oldPath} → </span>}
          {path}
        </span>
        <span className="add">+{adds}</span>
        <span className="del">−{dels}</span>
        {noteCount > 0 && (
          <button
            className={`chip ${showNotes ? "on" : ""}`}
            title="Toggle Claude's notes"
            onClick={(e) => { e.stopPropagation(); setShowNotes(!showNotes); }}
          >
            ✦ {noteCount}{concernCount > 0 && <span className="chip-warn"> · {concernCount} ⚠</span>}
          </button>
        )}
        {drafts.length > 0 && <span className="chip on">✎ {drafts.length}</span>}
      </header>
      {!collapsed && (
        <>
          {summary && <div className="file-summary"><Markdown>{summary}</Markdown></div>}
          {showNotes && orphans.length > 0 && (
            <div className="widget orphan">
              {orphans.map((a, i) => (
                <AnnotationCard key={i} a={{ ...a, title: `${a.title} (line ${a.line})` }} />
              ))}
            </div>
          )}
          {file.hunks.length === 0 ? (
            <div className="muted pad">Binary or empty change.</div>
          ) : (
            <Diff
              viewType={viewType}
              diffType={file.type}
              hunks={file.hunks}
              widgets={widgets}
              tokens={tokens}
              gutterEvents={{ onClick: ({ change }: { change: ChangeData | null }) => change && setComposing(getChangeKey(change)) }}
            >
              {(hunks) => hunks.map((h) => <Hunk key={h.content} hunk={h} />)}
            </Diff>
          )}
        </>
      )}
    </section>
  );
}

export const DiffFile = memo(DiffFileInner);
