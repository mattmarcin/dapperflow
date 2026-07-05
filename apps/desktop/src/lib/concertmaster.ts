// Concertmaster helpers: the demo transcript for offline screenshots and the scope
// steer line. The panel is a real terminal hosting a real harness (product.md: real
// terminals, never a scraped chat facsimile), so these only cover the two seams that
// cannot run a live PTY: the fixture demo, and the context line we type when the user
// sets a project scope.

import { Card, NeedsYouItem, Project, Session } from "../model";

/** A `[kind:ULID]` deep-link token, exactly as dflow-mcp emits it. */
export function deepLinkToken(kind: string, id: string): string {
  return `[${kind}:${id}]`;
}

// One short line describing the label a chip should carry for a card; keeps the demo
// transcript reading like a real Concertmaster answer.
function cardOneLiner(card: Card, project?: Project): string {
  const where = project ? ` (${project.name})` : "";
  return `${card.title}${where}`;
}

export interface DemoTranscriptInput {
  cards: Card[];
  sessions: Session[];
  projects: Project[];
  needsYou: NeedsYouItem[];
  launcherName: string;
}

/**
 * A believable "what needs me right now?" exchange, built from the LIVE store entities
 * so every `[kind:ULID]` token in it resolves to a real card/session/project and the
 * deep-link bar is fully clickable in demo mode. Pure string; the panel renders it in a
 * read-only terminal-styled surface when the daemon is absent (the design notes).
 */
export function buildDemoTranscript(input: DemoTranscriptInput): string {
  const { cards, sessions, projects, needsYou, launcherName } = input;
  const projectById = new Map(projects.map((p) => [p.id, p]));
  const cardById = new Map(cards.map((c) => [c.id, c]));

  // The two highest-scoring Needs You items with resolvable cards.
  const attention = needsYou
    .map((n) => ({ item: n, card: cardById.get(n.card_id) }))
    .filter((x): x is { item: NeedsYouItem; card: Card } => !!x.card)
    .slice(0, 2);

  const working = sessions.find((s) => s.state === "working");
  const workingCard = working?.card_id ? cardById.get(working.card_id) : undefined;
  const workingProject = workingCard?.project_id
    ? projectById.get(workingCard.project_id)
    : undefined;

  const lines: string[] = [];
  lines.push("~ concertmaster · dflow-mcp mounted · demo transcript");
  lines.push("");
  lines.push("> what needs me right now?");
  lines.push("");
  lines.push("● calling mcp__dflow__needs_you_list …");
  lines.push("");

  if (attention.length === 0) {
    lines.push("Nothing is waiting on you right now. The fleet is quiet.");
  } else {
    lines.push(`You have ${attention.length} thing${attention.length === 1 ? "" : "s"} waiting:`);
    lines.push("");
    attention.forEach(({ item, card }, i) => {
      const project = card.project_id ? projectById.get(card.project_id) : undefined;
      const token = deepLinkToken("card", card.id);
      const why =
        item.kind === "plan_round"
          ? "a plan round is waiting for your approval"
          : item.kind === "gate_finding"
            ? "the gate raised a finding that needs a call"
            : item.kind === "pr_ready"
              ? "a green PR is ready to merge"
              : "it is blocked on you";
      lines.push(`  ${i + 1}. ${token} ${cardOneLiner(card, project)}`);
      lines.push(`     ${why}.`);
    });
  }

  lines.push("");
  if (working && workingCard) {
    const st = deepLinkToken("session", working.id);
    const ct = deepLinkToken("card", workingCard.id);
    const note = working.status_note ? ` - ${working.status_note}` : "";
    lines.push(
      `Meanwhile ${st} is performing on ${ct} ${cardOneLiner(workingCard, workingProject)}${note}.`,
    );
  }
  if (projects[0]) {
    const pt = deepLinkToken("project", projects[0].id);
    lines.push(
      `Want me to focus on ${pt} ${projects[0].name}? Set the scope chip and I will keep to it.`,
    );
  }
  lines.push("");
  lines.push(`~ ${launcherName} · 6 turns · $0.03`);
  lines.push("> (type to continue - this is a demo transcript)");
  return lines.join("\n");
}

/**
 * The context line typed into the Concertmaster when the user sets a project scope
 * (product.md scoped sessions). We do NOT auto-submit: verified submit is a daemon
 * verb the panel cannot invoke yet (the design notes merge-time request 1),
 * so the honest behavior is to preload the composer and let the user send. The
 * `[project:ULID]` token means the Concertmaster echoes a one-click link back.
 */
export function scopeSteerLine(project: Project): string {
  return `From now on, scope your answers to the ${project.name} project (${deepLinkToken(
    "project",
    project.id,
  )}). Summarize what needs me there.`;
}
