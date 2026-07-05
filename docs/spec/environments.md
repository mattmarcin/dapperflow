# Environment Management Specification

One of the quietly hardest problems in parallel agent development: worktrees inherit tracked files but not the environment that makes a project actually run.
`.env` files are gitignored, secrets live in one checkout, local services (a Cloudflare D1 sqlite, a dev database, a docker compose stack) assume a single working copy and fixed ports.
Spin up four worktrees of the same app and everything collides or is missing.

DapperFlow treats environment as a first-class, per-project resource that follows work into every worktree automatically.

## Env Vault

- Per-project encrypted store (`env_entries` table; values encrypted at rest with a key held in the OS credential store: DPAPI on Windows, Keychain on macOS, Secret Service on Linux).
- Entry kinds:
  - `var`: plain environment variable injected into session environments.
  - `secret`: same injection, but write-only through the API (the UI can set/rotate, never display); the full redaction boundary (scrollback scrubbing, gate evidence, event payloads, artifacts, drift-guard diffs) is owned by `security.md / Secret handling policy`.
  - `file`: a materialized file (e.g. `.env.local`, `.dev.vars`, a service-account json) with a relative target path template inside the worktree.
- Import assist: point the vault at an existing checkout's `.env*` files once; it parses, classifies, and ingests them.

## Worktree materialization lifecycle

- On lease: inject `var`/`secret` entries into the session PTY environment and write `file` entries to their target paths.
- On return: shred materialized secret files before the worktree re-enters the pool; plain caches stay warm.
- Drift guard: if an agent edits a materialized file (e.g. adds a var it needed), teardown diffs it against the vault and raises a Needs You item offering to absorb the change, so environment knowledge accretes instead of evaporating with the worktree.

## Local services and the port broker

Declared per project in `services` (see data-model.md), optionally referenced by recipes.

- `scope: per_worktree`: the service runs once per leased worktree (e.g. `wrangler dev`, `npm run dev`); its state dirs live inside the worktree so instances never share storage.
- `scope: shared`: a singleton the daemon starts on first demand and refcounts (e.g. a heavyweight docker compose stack); worktrees connect to it.
- Port broker: services declare named ports; the daemon allocates real free ports per instance and injects them as env vars (`DFLOW_PORT_<NAME>`, plus template substitution into service commands).
  Parallel dev servers stop fighting over 3000; the card workspace shows each session's live URLs.
- Service health is part of session context: a dispatch whose declared services failed to start parks the card in Needs You rather than letting an agent flail against a dead backend.

## Cross-device future (deferred, designed-for)

- Vault entries ride the same event-log sync layer planned for M6+, but as client-side-encrypted payloads: encryption keys never leave devices, pairing exchanges keys directly (E2E), the sync relay stores ciphertext only.
- The `env_entries` schema (ciphertext blob, key id, per-entry versioning) is chosen now so E2E sync is an addition, not a migration.

## Milestone placement

- M2: vault (vars, secrets, files), lease-time materialization, teardown shredding, import assist.
- M3: drift guard; per-worktree services + port broker.
- M4+: shared services with refcounting.
- M6+: E2E-encrypted cross-device sync.
