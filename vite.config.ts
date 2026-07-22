import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and does not clear the screen so Rust errors
// stay visible. See https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      // Don't watch the Rust source tree from the web dev server.
      ignored: ["**/src-tauri/**"],
    },
  },
  // Produce a build the Tauri bundler can serve from disk.
  build: {
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
  },
});
