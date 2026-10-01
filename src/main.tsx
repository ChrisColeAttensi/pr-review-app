import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { isMac } from "./platform";

// The sidebar leaves room for the macOS traffic lights; other platforms have a native title bar.
document.documentElement.dataset.platform = isMac ? "mac" : "other";

async function boot() {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    (await import("./dev/mock")).installMock();
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
boot();
