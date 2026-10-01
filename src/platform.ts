/** The macOS ⌘ shortcuts map to Ctrl everywhere else. */
export const isMac = /Mac/.test(navigator.platform || navigator.userAgent);

/** The shortcut modifier is held: ⌘ on macOS, Ctrl elsewhere. */
export const mod = (e: { metaKey: boolean; ctrlKey: boolean }) => (isMac ? e.metaKey : e.ctrlKey);

/** Label for a shortcut: `keys("R")` is `⌘R` on macOS, `Ctrl+R` elsewhere. */
export const keys = (k: string) => (isMac ? `⌘${k}` : `Ctrl+${k}`);

/** Label for mod+Enter. */
export const MOD_ENTER = isMac ? "⌘↩" : "Ctrl+Enter";

/** Where the tray icon lives. */
export const TRAY = isMac ? "menu bar" : "system tray";
