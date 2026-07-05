# Protocol Specification

The wire protocol between `dflowd` and every client: the desktop app, the `dflow` agent CLI, and future mobile/web clients.
Defined in the `dflow-proto` crate; all types serde-serializable; protocol version negotiated at handshake.

## Transport

- WebSocket over loopback by default (`ws://127.0.0.1:<port>`); the port is written to the daemon's runtime file.
- Remote access (M6) adds a TLS listener with device pairing; the message layer is unchanged.
- Control messages: JSON text frames with envelope `{ "v": 1, "id": "<ulid>", "type": "<family.verb>", "payload": { ... } }`.
- Responses echo `id`; server-initiated events use `type` under `event.*` with no `id`.
- PTY I/O: binary frames, layout `[u8 kind][16-byte session ulid][bytes...]` where kind = output, input, or resize; keeps the hot path off the JSON serializer.

## Authentication

- First daemon run mints a root bearer token, stored with restricted ACL in the app data dir.
- Clients send `auth.hello { token, client: desktop|agent|mobile, proto_versions: [..] }` as the first frame; the daemon replies with granted scope and chosen version, or closes.
- Agent-CLI tokens are per-task scoped: minted at dispatch, injected as `DFLOW_TOKEN` + `DFLOW_CARD` env vars, valid only for that card's endpoints, revoked at teardown.

## Message families

### session.*

- `session.create { card_id, harness, model?, effort?, worktree_id, env }` -> `{ session_id }`
- `session.attach { session_id, cols, rows }` -> screen state replay (styled snapshot + recent scrollback) then live binary output frames
- `session.detach { session_id }`
- `session.input` / `session.resize`: binary frames (above)
- `session.send_verified { session_id, text, submit: true }` -> `{ submitted: bool, attempts }` (verified submit; see adapters.md)
- `session.signal { session_id, action: interrupt|exit|kill }`
- `session.resume { session_id }` -> relaunches the harness in the same worktree with its resume flag; response `{ session_id, resumed_from, resume_ref? }` where `session_id` is the NEW session and `resumed_from` records the predecessor's lineage (see architecture.md, session resume)
- `session.rename { session_id, title }`
- `agents.list {}` / `agents.detect {}` (PATH scan for known CLIs) / `agents.add { name, adapter, command, extra_args, extra_env }` / `agents.update` / `agents.remove`
- `session.list {}` -> compact fleet table, including `interrupted` sessions with resumability info

### project.*

- `project.add { path }` -> validation (git repo, default branch detection) -> `{ project_id }`
- `project.update { project_id, mode?, check_cmds?, default_recipe?, rounds_schedule?, gardener_schedule? }` (absent fields unchanged; the schedule fields carry the round/gardener schedule json, `""` clears)
- `project.list {}`

### card.*

- `card.create { title, type, project_id?, dial_recipe?, brief? }`
- `card.update`, `card.move { card_id, column }`, `card.query { filter }`
- `card.get { card_id }` -> card + sessions + artifacts + latest events

### dispatch.*

- `dispatch.start { card_id, recipe?, harness?, model?, effort? }` -> resolves recipe, leases worktree, materializes environment, composes brief, creates session
- `dispatch.cancel { card_id }`

### artifact.*

- `artifact.register { card_id, path, kind }` (from `dflow plan open`)
- `artifact.get { artifact_id }` -> HTML + audit status (desktop renders it)
- `artifact.feedback.submit { artifact_id, items: [...] }` (from the review chrome; see plan-studio.md payloads)
- `artifact.feedback.poll { artifact_id, wait: true }` -> long-poll used by the agent CLI; returns queued feedback, layout warnings, or `{ ended, next_step }`

### env.*

- `env.set { project_id, key, value, kind: secret|var|file, target? }` (values write-only from clients; reads are materialization-only)
- `env.list { project_id }` -> names and kinds, never values
- `env.materialize { worktree_id }` / `env.cleanup { worktree_id }` (daemon-internal, exposed for diagnostics)

### recipe.*

- `recipe.list {}` -> bundled + user + project recipes with source
- `recipe.get { name }` / `recipe.validate { content }` -> parsed structure + errors
- `recipe.install { source: path|url, scope: user|project }`

### github.*

- `github.auth.status {}` -> reports the local `gh` CLI presence and auth: `{ present, authenticated, account?, host?, repo? }` (gh-first; DapperFlow shells out to `gh`, so there is no in-daemon OAuth device flow)
- `github.issues.preview { project_id, filter }` -> `{ repo, issues: [{ number, title, labels, state, url, dedupe, existing_card_id? }] }`: the filtered issue list with a per-issue dedupe status, without importing
- `github.issues.import { project_id, filter: { numbers?, assignee?, labels?, milestone?, state? }, dial_recipe? }` -> `{ repo, results: [{ number, title, card_id, outcome: created|refreshed|suppressed }] }`: creates/refreshes origin cards (dedupe on origin_ref). The curated selection nests under `filter.numbers`; an empty filter imports the configured filter set

### fleet.* and event.*

- `fleet.status {}` -> one snapshot: `{ sessions, needs_you }` (the session table with lifecycle state, and the open Needs You queue highest score first)
- `event.subscribe { cursor? }` -> stream of `card_events` from cursor; the timeline UI, Needs You updates, and future sync all consume this
- `event.ack { cursor }` for client bookmark persistence

## Errors, reconnection, and flow control

- Every request either succeeds or returns a structured error: `{ id, type: "error", payload: { code, message, retryable, detail? } }` with stable machine-readable codes; auth failures close the socket with a distinct close code so clients distinguish "reconnect" from "re-pair".
- In-flight requests are cancellable: `cancel { id }`; the daemon responds to the original id with `code: cancelled`.
- Event subscriptions are resumable: `event.subscribe { cursor }` replays from the cursor (ULID of the last seen event); clients persist their cursor, so a dropped connection never loses timeline entries. Unknown event kinds must be preserved and displayed generically, never dropped.
- PTY output frames carry per-session sequence numbers; the client acks periodically and the daemon coalesces frames under backpressure (a slow client gets a fresh styled snapshot on catch-up instead of an unbounded buffer).
- Large payloads (artifacts, gate evidence) transfer chunked with size headers; the desktop fetches artifact documents over the daemon's HTTP endpoint (see security.md) rather than through WS frames.

## Webview token handoff

The Tauri shell obtains the root token via one Tauri IPC call at startup and passes it to the webview in memory; it never appears in URLs or persisted frontend storage (see security.md / Token architecture).

## Versioning rules

- `v` bumps only on breaking envelope changes; family payloads evolve additively with serde defaults.
- The daemon serves the newest version both sides support; clients older than the daemon's minimum get a structured `upgrade_required` close reason.
