// Chooses the board's data source. The daemon serves the full Phase 1 protocol
// (card.*/project.*/dispatch.*/event.*/fleet.status), so live is the default;
// set VITE_DFLOW_FIXTURES=1 for the DEV-ONLY fixture board (offline demos,
// UI work without a daemon).

import { DflowClient } from "../client";
import { DataSource } from "./source";
import { FixtureDataSource } from "./fixtures";
import { LiveDataSource } from "./live";

export type { DataSource } from "./source";

export function usingFixtures(): boolean {
  const flag = (import.meta as { env?: Record<string, string> }).env?.VITE_DFLOW_FIXTURES;
  // Default OFF since Phase 1 integration: the live daemon is the real source.
  if (flag === undefined) return false;
  return flag === "1" || flag === "true";
}

export function createDataSource(client: DflowClient | null): DataSource {
  if (!usingFixtures() && client) return new LiveDataSource(client);
  return new FixtureDataSource();
}
