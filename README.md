# DapperFlow

[![CI](https://github.com/mattmarcin/dapperflow/actions/workflows/ci.yml/badge.svg)](https://github.com/mattmarcin/dapperflow/actions/workflows/ci.yml)

A native, cross-platform cockpit for conducting AI coding agents across many projects at once.

You are the conductor.
DapperFlow gives you one window with a kanban board spanning every project, embedded live terminals running whichever agent CLI you prefer (Claude Code, Codex, OpenCode, Pi), an orchestrator agent that knows all your projects, git-worktree isolation for conflict-free parallel work, interactive HTML plan review, and a verification gate before anything becomes a PR.

## Status

Early access: the desktop app and daemon are built and self-hostable; the cloud features are in development.

- The full specification lives in [`docs/spec/`](docs/spec/).

## License

DapperFlow is source-available under the [Functional Source License (FSL-1.1-ALv2)](LICENSE.md).
You may use, modify, and self-host it freely for any purpose except a Competing Use (reselling it as a substitute for DapperFlow or its commercial offerings).
Each released version automatically converts to the permissive Apache License 2.0 two years after its release.
The cloud features (network punch-through, end-to-end-encrypted cross-device secret sync, and the hosted backend the mobile app uses) are separate commercial offerings.

## Design pillars

1. Zero forced dependencies: users need git and the agent CLIs they already use, nothing else (no tmux, no external multiplexer).
2. Real terminals are the conversation surface with agents; we never scrape output into a fake chat UI.
3. Plans are living HTML artifacts reviewed with annotations, never markdown in a terminal.
4. Your attention is the scarce resource: one ranked cross-project "Needs You" queue instead of twelve terminal tabs.
5. Local-first, sync-ready, mobile-ready: a headless daemon with an authenticated network protocol from day one.
