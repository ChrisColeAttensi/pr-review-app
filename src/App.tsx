import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import { JobsProvider } from "./jobs";
import { orderedPrs, Sidebar } from "./components/Sidebar";
import { PrView, type Tab } from "./components/PrView";
import { SettingsDialog } from "./components/SettingsDialog";
import type { Snapshot } from "./types";
import "react-diff-view/style/index.css";
import "./App.css";

const TAB_ORDER: Tab[] = ["overview", "walkthrough", "files", "review", "notes"];

export default function App() {
  const [snap, setSnap] = useState<Snapshot>({ me: "", prs: [], errors: [], fetchedAt: 0, loading: true });
  const [selected, setSelected] = useState<string>();
  const [tab, setTab] = useState<Tab>("overview");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    api.snapshot().then(setSnap);
    const uns = [
      listen<Snapshot>("prs-updated", (e) => setSnap(e.payload)),
      listen<string>("open-pr", (e) => { setSelected(e.payload); setTab("overview"); }),
      listen("open-settings", () => setSettingsOpen(true)),
    ];
    return () => uns.forEach((u) => u.then((f) => f()));
  }, []);

  const ordered = useMemo(() => orderedPrs(snap.prs, filter), [snap.prs, filter]);
  const pr = snap.prs.find((p) => p.key === selected);

  useEffect(() => {
    if (!selected && ordered.length) setSelected(ordered[0].key);
  }, [ordered, selected]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement).closest("input, textarea, select");
      if (e.metaKey && e.key === ",") { e.preventDefault(); setSettingsOpen(true); return; }
      if (e.metaKey && e.key === "r") { e.preventDefault(); api.refresh(); return; }
      if (e.metaKey && /^[1-5]$/.test(e.key)) { e.preventDefault(); setTab(TAB_ORDER[Number(e.key) - 1]); return; }
      if (typing) return;
      if (e.key === "/") { e.preventDefault(); document.getElementById("pr-filter")?.focus(); }
      if (e.key === "j" || e.key === "k") {
        const i = ordered.findIndex((p) => p.key === selected);
        const next = ordered[Math.min(ordered.length - 1, Math.max(0, i + (e.key === "j" ? 1 : -1)))];
        if (next) setSelected(next.key);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ordered, selected]);

  return (
    <JobsProvider>
      <div className="app">
        <Sidebar
          snap={snap}
          selected={selected}
          onSelect={(k) => { setSelected(k); }}
          onSettings={() => setSettingsOpen(true)}
          onRefresh={() => api.refresh()}
          filter={filter}
          setFilter={setFilter}
        />
        <main className="main">
          {pr ? (
            <PrView key={pr.key} pr={pr} me={snap.me} tab={tab} setTab={setTab} />
          ) : (
            <div className="empty" data-tauri-drag-region>
              <p className="muted">{snap.loading ? "Fetching pull requests…" : "Select a pull request."}</p>
            </div>
          )}
        </main>
        {settingsOpen && <SettingsDialog onClose={() => setSettingsOpen(false)} />}
      </div>
    </JobsProvider>
  );
}
