import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

export interface ToastAction {
  label: string;
  run: () => void;
}

interface Toast {
  id: number;
  text: string;
  tone: "info" | "ok" | "error";
  busy?: boolean;
  actions?: ToastAction[];
}

interface ToastsCtx {
  /** Returns the id, so a "working…" toast can be replaced by its outcome. */
  show: (t: Omit<Toast, "id">, replace?: number) => number;
  dismiss: (id: number) => void;
}

const Ctx = createContext<ToastsCtx>(null!);
export const useToasts = () => useContext(Ctx);

let nextId = 1;

export function ToastsProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const dismiss = useCallback((id: number) => setToasts((ts) => ts.filter((t) => t.id !== id)), []);
  const show = useCallback(
    (t: Omit<Toast, "id">, replace?: number) => {
      const id = replace ?? nextId++;
      setToasts((ts) => [...ts.filter((x) => x.id !== id), { ...t, id }]);
      // Successes fade on their own; errors stay until dealt with.
      if (t.tone === "ok") setTimeout(() => dismiss(id), 6000);
      return id;
    },
    [dismiss],
  );
  const value = useMemo(() => ({ show, dismiss }), [show, dismiss]);

  return (
    <Ctx.Provider value={value}>
      {children}
      <div className="toasts">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.tone}`}>
            {t.busy && <span className="spinner" />}
            <span className="toast-text">{t.text}</span>
            {t.actions?.map((a) => (
              <button key={a.label} className="link" onClick={() => { a.run(); dismiss(t.id); }}>{a.label}</button>
            ))}
            {!t.busy && <button className="link toast-x" onClick={() => dismiss(t.id)} title="Dismiss">✕</button>}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}
