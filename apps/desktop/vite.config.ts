import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// @tauri-apps/cli sets TAURI_DEV_HOST when running `tauri dev` on a device.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Prevent Vite from obscuring Rust errors and tune for the Tauri dev flow.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Don't watch the Rust source tree from the Vite dev server.
      ignored: ["**/src-tauri/**"],
    },
  },

  // Produce assets that the Tauri webview can load from disk.
  build: {
    target: "safari15",
    // Vite 8 minifies with Oxc by default; `true` uses it. (Pinning "esbuild"
    // here would force the deprecated path that needs esbuild installed
    // separately.) Skip minification under TAURI_DEBUG for readable dev builds.
    minify: !process.env.TAURI_DEBUG,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
}));
