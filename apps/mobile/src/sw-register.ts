// Service-worker registration, GUARDED for secure contexts only, with the honest
// LAN-HTTP posture baked in.
//
// The M6 PWA is served over plain HTTP on a LAN IP (security.md: LAN v1 ships without
// TLS). A plain-HTTP LAN origin is NOT a secure context (mobile.md 0, the secure-context
// wall), so `navigator.serviceWorker` is either absent or will reject registration. The
// app is fully functional without a service worker - it holds no offline promise beyond
// the live/stale-marked connection - so registration is best-effort and skipped loudly
// (in the console, and surfaced in Settings) rather than pretended.
//
// Where the origin IS secure (localhost during dev, or a future TLS listener), the worker
// registers and provides a minimal app-shell cache for a faster cold load.

export interface SwStatus {
  label: string;
  detail: string;
}

export function registerServiceWorker(): void {
  if (!("serviceWorker" in navigator)) {
    console.info("[dapperflow] service worker unsupported by this browser; running without one.");
    return;
  }
  if (!window.isSecureContext) {
    console.info(
      "[dapperflow] non-secure origin (LAN HTTP): skipping service worker by design. The app runs fully without one.",
    );
    return;
  }
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("./sw.js", { scope: "./" }).then(
      (reg) => console.info("[dapperflow] service worker registered:", reg.scope),
      (err) => console.warn("[dapperflow] service worker registration failed (non-fatal):", err),
    );
  });
}

/** The status shown in Settings, computed the same way the guard decides. */
export function serviceWorkerStatus(): SwStatus {
  if (!("serviceWorker" in navigator)) {
    return { label: "unsupported", detail: "This browser does not offer service workers. The app runs fully without one." };
  }
  if (!window.isSecureContext) {
    return {
      label: "off (LAN HTTP)",
      detail:
        "This origin is plain HTTP on the LAN, which is not a secure context, so no service worker and no install prompt - by design. Everything works over the live connection; disconnected views are stale-marked, never faked.",
    };
  }
  return {
    label: "active",
    detail: "This origin is a secure context, so the app-shell cache is active for faster cold loads.",
  };
}
