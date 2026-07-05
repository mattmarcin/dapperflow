// Dev-only Vite middleware that stands in for the daemon's loopback signed-URL
// artifact endpoint (plan-studio.md / security.md "Artifact sandbox architecture")
// until the daemon's artifact service lands. It is NOT part of the production
// design and is only mounted by the Vite dev server (apply: 'serve'); `pnpm build`
// never includes it. Promoted from the spike 5 artifact plugin.
//
// What it proves, faithfully:
//   - Artifacts served over HTTP with REAL response CSP headers (the exact posture
//     from security.md), not a meta-tag approximation.
//   - Short-lived SIGNED URLs: a capability (HMAC sig + expiry) lives in the URL,
//     no bearer token in the iframe. Tampered/expired URLs get 403.
//   - The review SDK and a bundled Mermaid build are injected SERVER-SIDE as
//     same-origin <script src> tags, so the agent HTML never references them and
//     the strict `script-src` (no CDN, no inline) still admits them.
//
// Production deltas (documented in the design notes):
//   - Real origin split: artifact-origin = daemon loopback, app-origin = webview;
//     here both collapse to the one Vite origin, so <artifact-origin>/<app-origin>
//     render as 'self'. The sandbox attribute forces an opaque origin regardless,
//     which is what actually enforces the no-same-origin constraint.
//   - Signing is an in-process HMAC stub; the daemon mints capability URLs.
//   - The daemon owns real artifact-dir file serving and the secret-scan-before-
//     register from security.md.

import { createHmac, randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
// Use Vite's re-exported esbuild so we do not depend on the transitive `esbuild`
// package directly (pnpm does not hoist it, which breaks a bare import).
import { transformWithEsbuild, type Plugin } from "vite";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SDK_SRC = path.join(HERE, "src/review/review-sdk.ts");
const ARTIFACT_DIR = path.join(HERE, "src/review/artifacts");
const MERMAID_SRC = path.join(HERE, "node_modules/mermaid/dist/mermaid.min.js");

// The dev artifact catalog. Allowlisted so a request cannot traverse out of the
// artifact directory; the daemon will serve per-card artifact directories instead.
const ARTIFACT_IDS = new Set(["plan-good", "plan-good-revised", "plan-broken", "gate-findings"]);

const SECRET = randomBytes(32); // rotates every dev-server start
const TTL_MS = 60_000; // short-lived; production would be shorter with re-signing

function sign(subject: string): { exp: number; sig: string } {
  const exp = Date.now() + TTL_MS;
  const sig = createHmac("sha256", SECRET).update(`${subject}.${exp}`).digest("hex").slice(0, 32);
  return { exp, sig };
}

function verify(subject: string, exp: number, sig: string): boolean {
  if (!Number.isFinite(exp) || exp < Date.now()) return false;
  const expected = createHmac("sha256", SECRET).update(`${subject}.${exp}`).digest("hex").slice(0, 32);
  // Constant-ish comparison; length is fixed so a simple equality is acceptable here.
  return sig.length === expected.length && sig === expected;
}

function signedQuery(subject: string): string {
  const { exp, sig } = sign(subject);
  return `exp=${exp}&sig=${sig}`;
}

async function transpiledSdk(): Promise<string> {
  const src = await readFile(SDK_SRC, "utf8");
  // format:'iife' emits a self-contained classic script (no import/export), so the
  // strict `script-src 'self'` (no inline, no CDN) admits it as a same-origin file.
  const out = await transformWithEsbuild(src, SDK_SRC, {
    loader: "ts",
    format: "iife",
    target: "es2020",
  });
  return out.code;
}

/** Compose the served document: agent HTML + server-injected SDK + Mermaid. */
function composeDocument(id: string, rawHtml: string): string {
  const sdkTag = `<script src="/__artifact/asset/sdk.js?${signedQuery("sdk")}"></script>`;
  const mermaidTag = `<script src="/__artifact/asset/mermaid.js?${signedQuery("mermaid")}"></script>`;
  let html = rawHtml;

  // Stamp the artifact id onto <html> so the SDK can report it.
  if (/<html[\s>]/i.test(html)) {
    html = html.replace(/<html/i, `<html data-artifact-id="${id}"`);
  } else {
    html = `<!doctype html><html data-artifact-id="${id}">${html}</html>`;
  }

  // Mermaid must load before the SDK (the SDK calls mermaid.run on boot).
  const inject = `${mermaidTag}\n${sdkTag}\n`;
  if (/<\/body>/i.test(html)) {
    html = html.replace(/<\/body>/i, `${inject}</body>`);
  } else {
    html = `${html}${inject}`;
  }
  return html;
}

function cspHeader(): string {
  // The exact posture from security.md. Dev collapses both origins to 'self';
  // the sandbox attribute (set by the app on the <iframe>) still forces an opaque
  // origin, which is what enforces "no same-origin". connect-src 'none' means the
  // iframe cannot exfiltrate - all app talk is postMessage.
  return [
    "default-src 'none'",
    "img-src 'self' data:",
    "style-src 'self' 'unsafe-inline'",
    "script-src 'self'",
    "frame-ancestors 'self'",
    "connect-src 'none'",
    "base-uri 'none'",
    "form-action 'self'",
  ].join("; ");
}

interface MiniRes {
  statusCode: number;
  setHeader(k: string, v: string): void;
  end(body?: string | Buffer): void;
}

function send(res: MiniRes, status: number, contentType: string, body: string | Buffer, csp?: boolean) {
  res.statusCode = status;
  res.setHeader("Content-Type", contentType);
  res.setHeader("Cache-Control", "no-store");
  if (csp) res.setHeader("Content-Security-Policy", cspHeader());
  res.end(body);
}

export function devArtifactPlugin(): Plugin {
  return {
    name: "dflow-dev-artifact-server",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        const rawUrl = req.url ?? "";
        if (!rawUrl.startsWith("/__artifact/")) return next();
        const url = new URL(rawUrl, "http://127.0.0.1");
        const p = url.pathname;
        const exp = Number(url.searchParams.get("exp"));
        const sig = url.searchParams.get("sig") ?? "";

        try {
          // 1. Mint a signed URL for an artifact (the app calls this; mirrors the
          //    daemon `artifact.get` returning a capability URL).
          if (p === "/__artifact/sign") {
            const id = url.searchParams.get("id") ?? "";
            if (!ARTIFACT_IDS.has(id)) return send(res, 404, "application/json", `{"error":"unknown artifact"}`);
            const signed = `/__artifact/doc/${id}?${signedQuery(`doc:${id}`)}`;
            return send(res, 200, "application/json", JSON.stringify({ url: signed, ttl_ms: TTL_MS }));
          }

          // 2. The injected SDK (signed, same-origin classic script).
          if (p === "/__artifact/asset/sdk.js") {
            if (!verify("sdk", exp, sig)) return send(res, 403, "text/plain", "bad signature");
            const code = await transpiledSdk();
            return send(res, 200, "text/javascript; charset=utf-8", code);
          }

          // 3. The bundled Mermaid build (signed, same-origin classic script).
          if (p === "/__artifact/asset/mermaid.js") {
            if (!verify("mermaid", exp, sig)) return send(res, 403, "text/plain", "bad signature");
            const code = await readFile(MERMAID_SRC);
            return send(res, 200, "text/javascript; charset=utf-8", code);
          }

          // 4. The artifact document (signed; served with the strict CSP header).
          const m = p.match(/^\/__artifact\/doc\/([a-z0-9-]+)$/i);
          if (m) {
            const id = m[1];
            if (!ARTIFACT_IDS.has(id)) return send(res, 404, "text/plain", "unknown artifact");
            if (!verify(`doc:${id}`, exp, sig)) {
              return send(res, 403, "text/html", "<h1>403 - artifact link expired or tampered</h1>", true);
            }
            const raw = await readFile(path.join(ARTIFACT_DIR, `${id}.html`), "utf8");
            return send(res, 200, "text/html; charset=utf-8", composeDocument(id, raw), true);
          }

          return next();
        } catch (err) {
          send(res, 500, "text/plain", `dev artifact server error: ${String(err)}`);
        }
      });
    },
  };
}
