import { defineConfig } from "vite";

// Tauri expects a fixed dev-server port and must not see its own Rust sources as frontend changes.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
  },
});
