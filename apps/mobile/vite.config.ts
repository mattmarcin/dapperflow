import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The M6 PWA is served two ways:
//   dev  - `pnpm dev` binds 0.0.0.0 so a real phone on the LAN can load it against
//          fixtures (see README.md for the command + Windows firewall note);
//   prod - the daemon's opt-in LAN listener serves the built `dist/` assets under a
//          path (the QR flow uses `/m`). `base: "./"` keeps every asset reference
//          relative, so the bundle drops under whatever path the daemon mounts it at
//          without a rebuild. That path mount is the merge-time integration seam.
export default defineConfig({
  base: "./",
  plugins: [react()],
  clearScreen: false,
  server: {
    host: true, // 0.0.0.0 - reachable from a phone on the same LAN
    port: 5273, // distinct from the desktop dev server (5173)
    strictPort: true,
  },
  preview: {
    host: true,
    port: 5273,
    strictPort: true,
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
  },
});
