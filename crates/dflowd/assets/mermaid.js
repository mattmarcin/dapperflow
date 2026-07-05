/*
 * Bundled Mermaid placeholder (`plan-studio.md` / Mermaid; `spike5` / Mermaid finding).
 *
 * The real artifact serves the bundled Mermaid build (`mermaid.min.js`, a single ~3.5MB
 * esbuild IIFE with zero dynamic imports) same-origin under `script-src 'self'` with no
 * `unsafe-eval` - the spike proved Mermaid 11.16 does not need eval, so the strict CSP
 * needs no loosening for diagrams. That build exposes its API at
 * `window.__esbuild_esm_mermaid_nm.mermaid` (default export), NOT a bare `window.mermaid`;
 * the injector and SDK normalize this defensively.
 *
 * PACKAGING SWAP POINT: this placeholder is committed instead of the 3.5MB binary (no
 * CDN is permitted under the CSP, and a multi-megabyte vendored blob does not belong in
 * the source tree). A packaged build drops the real `mermaid.min.js` here (or the
 * injector reads it from the app bundle), served gzipped and lazy-injected only for
 * artifacts that contain a `.mermaid` block. The placeholder keeps the served document
 * valid and the SDK's defensive resolution a no-op when no real build is present.
 */
(function () {
  "use strict";
  if (!window.__esbuild_esm_mermaid_nm) {
    window.__esbuild_esm_mermaid_nm = {
      mermaid: {
        __placeholder: true,
        initialize: function () {},
        run: function () { return Promise.resolve(); },
        render: function (id, text, cb) { if (cb) cb("<svg data-mermaid-placeholder=\"1\"></svg>"); return Promise.resolve({ svg: "" }); },
      },
    };
  }
})();
