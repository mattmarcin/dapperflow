// DapperFlow M6 PWA - minimal app-shell service worker.
//
// This only ever runs where the origin is a secure context (see src/sw-register.ts);
// on the plain-HTTP LAN origin it is never registered. Its sole job is a faster cold
// load of the static shell. It deliberately does NOT cache protocol data: the client
// state model is live-or-stale-marked (mobile.md 3.3), never a silent offline snapshot,
// and it never touches the WebSocket connection.

const CACHE = "dapperflow-shell-v1";
const SHELL = ["./", "./index.html", "./manifest.webmanifest", "./favicon.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting())
      .catch(() => undefined),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  // Only same-origin GETs for the static shell; never intercept WS or cross-origin.
  if (req.method !== "GET" || new URL(req.url).origin !== self.location.origin) return;

  event.respondWith(
    caches.match(req).then((hit) => {
      if (hit) return hit;
      return fetch(req)
        .then((res) => {
          // Cache successful navigations and static assets opportunistically.
          if (res.ok && (req.mode === "navigate" || /\.(js|css|svg|png|webmanifest)$/.test(req.url))) {
            const copy = res.clone();
            caches.open(CACHE).then((c) => c.put(req, copy)).catch(() => undefined);
          }
          return res;
        })
        .catch(() => caches.match("./index.html").then((idx) => idx ?? Response.error()));
    }),
  );
});
