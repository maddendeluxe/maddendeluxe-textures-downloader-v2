import { invoke } from "@tauri-apps/api/core";

// Links go through our own command rather than the opener plugin: inside an AppImage
// the plugin runs the AppImage's own bundled xdg-open, with the AppImage's search paths
// still set, and on SteamOS that opened nothing while reporting success. The whole
// story is in src-tauri/src/commands/open_external.rs.
//
// A failure here used to be dropped on the floor, so a dead link looked like nothing
// happening at all. It now reaches ~/textures-downloader-debug.log, via the console
// hook in debugLog.ts, along with everything the opener tried.
export async function openExternal(url: string): Promise<void> {
  try {
    await invoke("open_external", { url });
  } catch (err) {
    console.error(`Could not open ${url}:`, err);
  }
}
