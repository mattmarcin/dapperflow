// Configured-launcher helpers for the Settings > Agents rack and the launcher
// pickers. Adapter families carry a human label; the caution reason is recomputed
// from the launcher's own args so the UI can NAME which flags weaken safety - the
// daemon only sends the `caution` boolean (dflow-core / agents danger list).

export const ADAPTER_LABEL: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
  cursor: "Cursor",
  pi: "Pi",
  custom: "Custom",
};

// One-line, plain-language note on what an adapter family is, shown under the family
// select so a custom launcher (cc-alt) is never a mystery.
export const ADAPTER_HINT: Record<string, string> = {
  claude: "Anthropic Claude Code.",
  codex: "OpenAI Codex CLI.",
  opencode: "OpenCode.",
  cursor: "Cursor Agent.",
  pi: "Pi.",
  custom: "A CLI with no built-in behavior profile yet.",
};

export function adapterLabel(adapter: string): string {
  return ADAPTER_LABEL[adapter] ?? adapter;
}

// The autonomy flags that weaken safety, mirrored from dflow_core::agents (the
// caution danger list, whose comment points back at product.md / Settings > Agents).
// Single tokens plus adjacent flag/value pairs and `flag=value` spellings. Used only
// to display WHICH args triggered the daemon's caution flag; the daemon's boolean is
// the source of truth for whether to warn.
const CAUTION_SINGLE = new Set(["--dangerously-skip-permissions", "--auto"]);
const CAUTION_PAIRS: [string, string][] = [
  ["--permission-mode", "bypassPermissions"],
  ["-a", "never"],
  ["--ask-for-approval", "never"],
];

/** The subset of a launcher's extra args that weaken safety, spelled for display. */
export function cautionArgs(extraArgs: string[]): string[] {
  const hits: string[] = [];
  for (let i = 0; i < extraArgs.length; i++) {
    const arg = extraArgs[i];
    if (CAUTION_SINGLE.has(arg)) {
      hits.push(arg);
      continue;
    }
    for (const [flag, value] of CAUTION_PAIRS) {
      if (arg === `${flag}=${value}`) hits.push(arg);
      else if (arg === flag && extraArgs[i + 1] === value) hits.push(`${flag} ${value}`);
    }
  }
  return hits;
}
