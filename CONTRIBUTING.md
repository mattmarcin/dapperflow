# Contributing to DapperFlow

## Building and checking locally

The CI pipeline (`.github/workflows/ci.yml`, designed in `docs/spec/cicd.md`) runs exactly these commands.
Run them before opening a PR to get the same result locally.

Prerequisites: a stable Rust toolchain (the pipeline pins the lint gate to 1.91; any recent stable builds and tests), Node 22, and pnpm 10 (`corepack enable` or install pnpm directly). git is required at runtime by the app itself.

### Rust workspace (the six crates)

```bash
# Formatting (see the note on the advisory fmt gate in docs/spec/cicd.md).
cargo fmt --all --check

# Lint. CI pins this to Rust 1.91 for deterministic results.
cargo clippy --workspace --all-targets --locked -- -D warnings

# Build every target on any platform.
cargo build --workspace --all-targets --locked

# Tests. Windows is the validated platform. Run unit tests in parallel, then the
# PTY/daemon integration tests serialized so ConPTY spawn timing cannot flake them.
cargo test --workspace --lib --bins --locked
cargo test --workspace --test '*' --locked -- --test-threads=1
```

Set `DFLOW_DATA_DIR` to a throwaway directory when running daemon/integration tests so they never touch your real data dir.
Live and evidence tests are `#[ignore]`d and need real agent CLIs; run them explicitly with `--ignored` only when you have those CLIs installed.

### Frontend (two pnpm apps)

For each of `apps/desktop` and `apps/mobile`:

```bash
cd apps/<app>
pnpm install --frozen-lockfile
pnpm exec tsc          # strict typecheck
pnpm exec vite build   # production bundle
# (pnpm build runs both of the above)
```

### Desktop Tauri app

```bash
cd apps/desktop && pnpm install --frozen-lockfile && pnpm build   # produces dist/
cd apps/desktop/src-tauri && cargo check --locked
```

On Linux this needs the Tauri 2 system libraries (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `libayatana-appindicator3-dev`, `libsoup-3.0-dev`, `libxdo-dev`); see the `desktop-tauri` job in `ci.yml` for the exact `apt` list.

## Running the daemon in development

In development the desktop app does NOT spawn `dflowd`.
It runs in dev-external mode (`DFLOW_DEV_EXTERNAL_DAEMON=1`, the default for a debug build) and connects to a daemon you run yourself, so app rebuilds never fight an exe lock and a rebuild never orphans anything.
If no daemon is live, the app's connection UI tells you to start one instead of spawning `target/debug`.

Run the dev daemon and control it with the repo tasks (a `justfile`, or the equivalent standalone scripts under `scripts/`):

```bash
just daemon-dev       # run dflowd under cargo-watch: auto-rebuild + restart on any crate change
just daemon-status    # one line: running, pid, port, live session count
just daemon-stop      # graceful --stop: reaps the tree, marks sessions resumable
just daemon-restart   # stop -> rebuild -> start detached
```

If you do not have `just`, call the scripts directly - they contain the same logic and need no extra tooling:

```powershell
# Windows (PowerShell)
powershell -File scripts/daemon-dev.ps1
powershell -File scripts/daemon-stop.ps1
powershell -File scripts/daemon-status.ps1
powershell -File scripts/daemon-restart.ps1
```

```bash
# macOS / Linux / git-bash
bash scripts/daemon-dev.sh
bash scripts/daemon-stop.sh
bash scripts/daemon-status.sh
bash scripts/daemon-restart.sh
```

`daemon-dev` uses `cargo-watch` when it is installed (`cargo install cargo-watch`) and falls back to a single `cargo run -p dflowd` with a note when it is not.
These tasks replace the ad-hoc `taskkill /F` + rebuild loop, which was the source of the orphaned-ConPTY-host leaks: every stop here is a graceful `--stop` that reaps the whole tree through the daemon's Job Object.
To run the app against a bundled, app-managed daemon instead (production behavior) set `DFLOW_DEV_EXTERNAL_DAEMON=0`.

## Commit conventions

Conventional Commits (`feat:`, `fix:`, `chore:`, `docs(scope):`, ...).
Do not add agent co-author trailers.
Do not hand-edit generated files (`CHANGELOG.md`, lockfiles beyond what the package manager writes).
