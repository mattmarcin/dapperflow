import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { AppStoreProvider } from "./state/app-store";
import { registerServiceWorker } from "./sw-register";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("root element missing");

createRoot(root).render(
  <React.StrictMode>
    <AppStoreProvider>
      <App />
    </AppStoreProvider>
  </React.StrictMode>,
);

// Guarded, best-effort. Skips entirely on a non-secure (LAN-HTTP) origin; the app is
// fully functional without it. See sw-register.ts for the honest posture.
registerServiceWorker();
