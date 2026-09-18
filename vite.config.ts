import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// Which loader to build. Every game-specific string lives in games/<id>/game.json,
// the same file src-tauri/build.rs reads, so the frontend and the Rust side can never
// disagree about which mod they are downloading. `tools/games.py list` shows the ids.
// @ts-expect-error process is a nodejs global
const gameId = process.env.GAME || "madden09";
const gameConfig = JSON.parse(
  readFileSync(fileURLToPath(new URL(`./games/${gameId}/game.json`, import.meta.url)), "utf8"),
);

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // Baked in at build time and read by frontend/config.ts.
  define: {
    __GAME_CONFIG__: JSON.stringify(gameConfig),
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
