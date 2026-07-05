import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { devArtifactPlugin } from "./dev-artifact-plugin";

// Vite config tuned for Tauri: fixed dev port, no screen clearing so Rust logs
// stay visible, and a modern build target the WebView2 runtime supports.
export default defineConfig({
  // devArtifactPlugin is dev-only (apply:'serve'); it stands in for the daemon's
  // loopback artifact endpoint, serving plan artifacts with the strict CSP + signed
  // URLs, and is absent from production builds (the daemon serves them in prod).
  plugins: [react(), devArtifactPlugin()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
  },
});
