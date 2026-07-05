// Chooses the board's data source. The daemon serves the full Phase 1 protocol
// (card.*/project.*/dispatch.*/event.*/fleet.status), so live is the default;
// set VITE_DFLOW_FIXTURES=1 for the DEV-ONLY fixture board (offline demos,
// UI work without a daemon).

import { DflowClient } from "../client";
import { DataSource } from "./source";
import { EmptyDataSource } from "./empty";
import { FixtureDataSource } from "./fixtures";
import { LiveDataSource } from "./live";

export type { DataSource } from "./source";

export function usingFixtures(): boolean {
  const flag = (import.meta as { env?: Record<string, string> }).env?.VITE_DFLOW_FIXTURES;
  // Default OFF since Phase 1 integration: the live daemon is the real source.
  if (flag === undefined) return false;
  return flag === "1" || flag === "true";
}

// Precedence, and the honesty rule that governs it:
//   1. VITE_DFLOW_FIXTURES=1 -> FixtureDataSource: the DEV-ONLY demo board, always clearly
//      labeled as fixtures by the UI (which keys its indicator off the source `.mode`).
//   2. a connected client -> LiveDataSource: the real daemon.
//   3. otherwise (no dev flag, daemon unreachable) -> EmptyDataSource: an HONEST empty,
//      disconnected board. It must NEVER fall back to fabricated fixtures here - a down
//      daemon on a real build silently rendering a fake fleet (fake cards, projects, paired
//      devices) as if live was a real honesty bug. The daemon-offline banner explains the
//      empty state instead.
export function createDataSource(client: DflowClient | null): DataSource {
  if (usingFixtures()) return new FixtureDataSource();
  if (client) return new LiveDataSource(client);
  return new EmptyDataSource();
}
