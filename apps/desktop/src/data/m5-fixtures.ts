// ============================================================================
// DEV ONLY - M5/M6 fixture seed data (GitHub issue import, verification gate,
// remote/device pairing). Kept beside recipe-fixtures.ts so fixtures.ts stays
// legible. None of this ships in a real daemon build; the live source degrades
// honestly until the M5 gate engine, gh transport, and M6 LAN listener land.
// ============================================================================

import {
  CiCheck,
  GateCheck,
  GateFinding,
  GithubAuthStatus,
  GithubImportConfig,
  GithubIssue,
  PrState,
  RemoteCapabilityProfile,
  RemoteListenerState,
} from "../model";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

// --- GitHub auth (github.auth.status) ---------------------------------------
// gh present and authenticated, so PR mode is available (gate.md). Flip authenticated
// to false to demo the clean degrade-to-local-only path in the GitHub settings.

export const GITHUB_AUTH: GithubAuthStatus = {
  gh_present: true,
  authenticated: true,
  user: "mattmarcin",
  host: "github.com",
  scopes: ["repo", "read:org", "gist"],
  setup_hint: null,
};

export const DEFAULT_IMPORT_CONFIG: GithubImportConfig = {
  assignees: [],
  labels: [],
  milestone: null,
  state: "open",
};

// --- GitHub issue pool (github.issues.preview / .import) --------------------
// A realistic backlog on the ledger-svc repo. Issue #241 backs the seed github_issue
// card ("Webhook signature check rejects valid Stripe events"); the rest populate the
// import preview, including two already-imported issues for the dedupe demo.

const REPO = "dappertoast/ledger-svc";
const issueUrl = (n: number) => `https://github.com/${REPO}/issues/${n}`;
const now = Date.now();

export const GITHUB_ISSUES: GithubIssue[] = [
  {
    number: 241,
    title: "Webhook signature check rejects valid Stripe events",
    body:
      "Since bumping the Stripe SDK, a subset of **valid** webhook events fail signature verification and 400 back to Stripe, which then retries them into oblivion.\n\n" +
      "## Steps to reproduce\n" +
      "1. Send a live `payment_intent.succeeded` event with the current signing secret.\n" +
      "2. Observe `SignatureVerificationError: no signatures found matching the expected signature`.\n\n" +
      "## Notes\n" +
      "- Only events whose raw body contains a multibyte character fail, which points at a body-encoding mismatch before the HMAC.\n" +
      "- We almost certainly re-serialize the JSON body before verifying instead of using the raw bytes.\n\n" +
      "Acceptance: valid events verify; the failure path has a regression test.",
    state: "open",
    author: "priya-ops",
    assignees: ["mattmarcin"],
    labels: [
      { name: "bug", color: "d73a4a" },
      { name: "webhooks", color: "5319e7" },
      { name: "priority:high", color: "b60205" },
    ],
    milestone: "v1.4 hardening",
    comments: [
      {
        author: "mattmarcin",
        body: "Confirmed - we JSON.parse then re-stringify before the HMAC. Need the raw body buffer. Will pick this up.",
        created_at: now - 3 * HOUR,
      },
      {
        author: "priya-ops",
        body: "Stripe retried this event 14 times overnight. Bumping to priority:high.",
        created_at: now - 2 * HOUR,
      },
    ],
    url: issueUrl(241),
    repo: REPO,
    updated_at: now - 2 * HOUR,
    suggested_type: "bug",
  },
  {
    number: 247,
    title: "Add idempotency keys to the invoice create endpoint",
    body:
      "Double-submits from the client create duplicate invoices. We should accept an `Idempotency-Key` header and de-dupe within a 24h window.\n\n" +
      "- Store key -> first response for 24h.\n" +
      "- Return the original response on replay, not a fresh 201.",
    state: "open",
    author: "dana-eng",
    assignees: [],
    labels: [
      { name: "feature", color: "0e8a16" },
      { name: "api", color: "1d76db" },
    ],
    milestone: "v1.4 hardening",
    comments: [],
    url: issueUrl(247),
    repo: REPO,
    updated_at: now - 6 * HOUR,
    suggested_type: "feature",
  },
  {
    number: 244,
    title: "Flaky test: settlement rounding off by one cent",
    body:
      "`settlement_test.go` fails ~1 run in 20 with a one-cent difference. Suspect banker's rounding applied inconsistently between the aggregate and the line items.",
    state: "open",
    author: "priya-ops",
    assignees: ["mattmarcin"],
    labels: [
      { name: "bug", color: "d73a4a" },
      { name: "flaky-test", color: "fbca04" },
    ],
    milestone: null,
    comments: [
      {
        author: "dana-eng",
        body: "Reproduced under `-count=200`. It's the aggregate rounding before summing.",
        created_at: now - 1 * DAY,
      },
    ],
    url: issueUrl(244),
    repo: REPO,
    updated_at: now - 20 * HOUR,
    suggested_type: "test",
  },
  {
    number: 240,
    title: "Document the webhook retry/backoff envelope",
    body: "Integrators keep asking what our retry schedule is. Publish the backoff envelope and the dead-letter behavior in the API docs.",
    state: "open",
    author: "dana-eng",
    assignees: [],
    labels: [
      { name: "docs", color: "0075ca" },
      { name: "good first issue", color: "7057ff" },
    ],
    milestone: null,
    comments: [],
    url: issueUrl(240),
    repo: REPO,
    updated_at: now - 2 * DAY,
    suggested_type: "chore",
  },
  {
    number: 238,
    title: "Rate-limit the public API by API key",
    body:
      "We have no per-key rate limiting; one noisy integration can starve the rest.\n\nProposal: token bucket, 100 req/min default, `429` with `Retry-After`.",
    state: "open",
    author: "mattmarcin",
    assignees: ["mattmarcin"],
    labels: [
      { name: "feature", color: "0e8a16" },
      { name: "reliability", color: "c5def5" },
    ],
    milestone: "v1.4 hardening",
    comments: [
      {
        author: "priya-ops",
        body: "Already scoped this into a card - see the board.",
        created_at: now - 3 * DAY,
      },
    ],
    url: issueUrl(238),
    repo: REPO,
    updated_at: now - 3 * DAY,
    suggested_type: "feature",
  },
  {
    number: 232,
    title: "Add /health and /ready probe endpoints",
    body: "K8s needs liveness/readiness probes. `/health` returns 200 always; `/ready` checks the DB pool.",
    state: "open",
    author: "dana-eng",
    assignees: [],
    labels: [{ name: "chore", color: "cfd3d7" }],
    milestone: null,
    comments: [],
    url: issueUrl(232),
    repo: REPO,
    updated_at: now - 5 * DAY,
    suggested_type: "chore",
  },
];

// --- Verification gate (gate.md) --------------------------------------------
// STABLE finding ids: the gate-findings.html artifact keys its radio controls by
// `finding.<id>`, and the Verify tab maps a captured control's question_key back to a
// finding, so these must match the artifact verbatim (like the stable artifact ids).

export const NPLUS1_FINDINGS: GateFinding[] = [
  {
    id: "nplus1-index",
    severity: "major",
    title: "Batched load still lacks a supporting index",
    scenario:
      "The N+1 is gone, but `invoice_line_items` has no index on `invoice_id`, so the single batched query does a sequential scan. Under the 40k-invoice fixture the endpoint p95 is 2.1s. Add an index on (invoice_id, position).",
    rule: null,
    file: "migrations/0042_invoice_lines.sql",
    line: 1,
    klass: "intent",
    resolution: null,
  },
  {
    id: "nplus1-ordering",
    severity: "major",
    title: "Batching changes line-item ordering",
    scenario:
      "The old per-invoice query returned line items in insertion order; the batched `WHERE invoice_id IN (...)` returns them grouped by a plan the DB chooses. The public API documents insertion order. Preserve an explicit ORDER BY, or version the endpoint.",
    rule: "api-contract: invoice.line_items are insertion-ordered",
    file: "src/invoices/list.ts",
    line: 88,
    klass: "intent",
    resolution: null,
  },
  {
    id: "nplus1-import",
    severity: "minor",
    title: "Unused import left after the refactor",
    scenario: "`groupBy` from lodash is no longer referenced once the manual loop is gone.",
    rule: "lint: no-unused-vars",
    file: "src/invoices/list.ts",
    line: 3,
    klass: "mechanical",
    resolution: "fix",
    auto_applied: true,
  },
];

export const NPLUS1_CHECKS: GateCheck[] = [
  {
    name: "build",
    cmd: "go build ./...",
    status: "passed",
    exit_code: 0,
    duration_ms: 8400,
    output: "go build ./...\n(ok, 0 diagnostics)",
  },
  {
    name: "test",
    cmd: "go test ./... -run Invoice",
    status: "passed",
    exit_code: 0,
    duration_ms: 21300,
    output:
      "=== RUN   TestInvoiceList_BatchedLoad\n--- PASS: TestInvoiceList_BatchedLoad (0.41s)\n=== RUN   TestInvoiceList_Ordering\n--- PASS: TestInvoiceList_Ordering (0.12s)\nok  \tledger/invoices\t2.317s",
  },
  {
    name: "lint",
    cmd: "golangci-lint run",
    status: "passed",
    exit_code: 0,
    duration_ms: 6100,
    output: "golangci-lint run\n1 issue auto-fixed (goimports): src/invoices/list.ts\n0 issues remaining",
  },
  {
    name: "typecheck",
    cmd: "tsc --noEmit",
    status: "passed",
    exit_code: 0,
    duration_ms: 4200,
    output: "tsc --noEmit\n(no errors)",
  },
];

// A gate mid-flight for the "Keyboard navigation" card: checks green, review running,
// no findings surfaced yet (adversarial reviewer still reading the diff).
export const KEYNAV_CHECKS: GateCheck[] = [
  { name: "build", cmd: "pnpm build", status: "passed", exit_code: 0, duration_ms: 12800, output: "vite build\n✓ built in 12.4s" },
  {
    name: "test",
    cmd: "pnpm test palette",
    status: "passed",
    exit_code: 0,
    duration_ms: 9400,
    output: "PASS  src/palette/nav.test.ts (18 tests)",
  },
  { name: "lint", cmd: "pnpm lint", status: "passed", exit_code: 0, duration_ms: 3300, output: "eslint .\n0 problems" },
  { name: "typecheck", cmd: "pnpm typecheck", status: "running", exit_code: null, duration_ms: null, output: "tsc --noEmit\n…" },
];

// A finished gate for the "Add /health and /ready" card (PR lane): all green, PR open,
// CI passed, mergeable - the disabled-until-green Merge action is ENABLED here.
export const HEALTH_CHECKS: GateCheck[] = [
  { name: "build", cmd: "go build ./...", status: "passed", exit_code: 0, duration_ms: 7100, output: "(ok)" },
  { name: "test", cmd: "go test ./...", status: "passed", exit_code: 0, duration_ms: 18200, output: "ok  \tledger/health\t0.204s" },
  { name: "lint", cmd: "golangci-lint run", status: "passed", exit_code: 0, duration_ms: 5600, output: "0 issues" },
];

export const HEALTH_PR: PrState = {
  status: "ci_passed",
  number: 318,
  url: "https://github.com/dappertoast/ledger-svc/pull/318",
  branch: "feat/health-probes",
  ci: [
    { name: "build", status: "success" },
    { name: "test", status: "success" },
    { name: "lint", status: "success" },
  ],
  mergeable: true,
  merge_method: "squash",
  fixes_issue: `${REPO}#232`,
};

// A PR still waiting on CI for the "Warm the parse cache" card: Merge stays DISABLED.
export const CACHE_PR: PrState = {
  status: "ci_running",
  number: 77,
  url: "https://github.com/dappertoast/orchard-cli/pull/77",
  branch: "perf/warm-parse-cache",
  ci: [
    { name: "build", status: "success" },
    { name: "test", status: "running" },
    { name: "bench", status: "queued" },
  ],
  mergeable: false,
  merge_method: "squash",
  fixes_issue: null,
};

export function ciAllGreen(ci: CiCheck[]): boolean {
  return ci.length > 0 && ci.every((c) => c.status === "success");
}

// --- Remote access / device pairing (M6; security.md) -----------------------

export const PHONE_PROFILE: RemoteCapabilityProfile = {
  needs_you: true,
  approvals: true,
  steering: true,
  terminals_read_only: true,
  vault_access: false,
  recipe_install: false,
};

// base64url of a compact JSON blob, no padding (the encoding apps/mobile settled on).
// Browser-only fixture code, so btoa is always present; encode UTF-8 first so any
// multibyte character in the payload survives.
export function base64url(input: string): string {
  const bytes = new TextEncoder().encode(input);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// Build the pairing payload apps/mobile settled on:
//   http://<lan-ip>:<port>/m#pair=<base64url{url,token}>
export function pairingPayload(lanIp: string, port: number, token: string): { url: string; payload: string } {
  const url = `http://${lanIp}:${port}/m`;
  const frag = base64url(JSON.stringify({ url, token }));
  return { url, payload: `${url}#pair=${frag}` };
}

// A phone-scoped capability token, base64url, stable-shaped for screenshots. In the
// real daemon this is minted by the LAN listener and scoped to PHONE_PROFILE.
export function mintRemoteToken(seed = "dev"): string {
  const raw = `${seed}.${Date.now().toString(36)}.${Math.random().toString(36).slice(2, 12)}`;
  return base64url(`dflow-lan-${raw}`).slice(0, 43);
}

export const REMOTE_LAN_IP = "192.168.1.42";
export const REMOTE_PORT = 8787;

// Initial state: the listener is OFF (opt-in per security.md), with one previously
// paired device retained so the revoke control and the paired-devices list are
// demonstrable. Enabling mints a fresh URL + token.
export function initialRemoteState(): RemoteListenerState {
  return {
    enabled: false,
    lan_ip: REMOTE_LAN_IP,
    port: REMOTE_PORT,
    url: null,
    pairing_payload: null,
    token: null,
    minted_at: null,
    profile: { ...PHONE_PROFILE },
    devices: [
      {
        id: "dev-iphone-01",
        name: "Matt's iPhone",
        profile: "phone",
        paired_at: now - 4 * DAY,
        last_seen: now - 40 * MIN,
        capabilities: { ...PHONE_PROFILE },
      },
    ],
  };
}
