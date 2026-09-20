import { invoke } from "@tauri-apps/api/core";

// Everything the webview would have printed to a console nobody can see goes to the
// same file the backend writes: ~/textures-downloader-debug.log. A bug report from a
// machine we cannot reach (a Steam Deck, say) then arrives with its own evidence.

function describe(value: unknown): string {
  if (value instanceof Error) return `${value.name}: ${value.message}\n${value.stack ?? ""}`;
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function debugLog(level: string, ...parts: unknown[]): void {
  // Never let logging throw, and never log a logging failure (that would loop).
  try {
    void invoke("frontend_log", { level, message: parts.map(describe).join(" ") }).catch(() => {});
  } catch {
    /* not running inside Tauri */
  }
}

export function installDebugLog(): void {
  for (const level of ["log", "info", "warn", "error"] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      original(...args);
      debugLog(level, ...args);
    };
  }
  window.addEventListener("error", (e) => {
    debugLog("uncaught", e.message, `at ${e.filename}:${e.lineno}:${e.colno}`, e.error);
  });
  window.addEventListener("unhandledrejection", (e) => {
    debugLog("unhandledrejection", e.reason);
  });
  // Every click on a link, before any handler runs: proves whether a click that
  // "did nothing" ever reached the page at all.
  document.addEventListener(
    "click",
    (e) => {
      const link = (e.target as HTMLElement | null)?.closest?.("a");
      if (link) debugLog("click", `link "${link.textContent?.trim()}" href=${link.getAttribute("href")}`);
    },
    true
  );
  debugLog("start", `frontend loaded; userAgent=${navigator.userAgent}`);
}
