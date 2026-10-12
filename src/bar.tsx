// PR Bar's settings window. PR Bar has no other UI: everything else lives in its menu.
import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { SettingsDialog } from "./components/SettingsDialog";
import { isMac } from "./platform";
import "./App.css";

document.documentElement.dataset.platform = isMac ? "mac" : "other";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsDialog app="bar" onClose={() => getCurrentWindow().close()} />
  </React.StrictMode>,
);
