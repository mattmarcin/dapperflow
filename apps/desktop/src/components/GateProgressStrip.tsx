// A compact gate-progress readout shown inline on a board card while it sits in the
// Verifying lane (product.md deliverable: "the Verifying lane shows gate progress
// inline on the card"). One segment per check, colored by outcome, plus a one-line
// phase label - glanceable from the board without opening the card's Verify tab.
//
// It reads the card's gate run through the DataSource (fixture or live); a card with no
// gate run renders nothing, so non-gated cards look unchanged.

import { useEffect, useState } from "react";
import { GateRun } from "../model";
import { useStore } from "../state/store";

function phaseLabel(gate: GateRun): { text: string; tone: string } {
  const passed = gate.checks.filter((c) => c.status === "passed").length;
  const total = gate.checks.length;
  const failing = gate.checks.some((c) => c.status === "failed");
  const running = gate.checks.some((c) => c.status === "running" || c.status === "pending");
  const needFindings = gate.findings.filter((f) => f.klass === "intent" && !f.resolution).length;

  if (failing) return { text: "checks failing", tone: "trouble" };
  if (running) return { text: `checks ${passed}/${total}`, tone: "working" };
  if (gate.status === "awaiting_human" && needFindings > 0)
    return { text: `${needFindings} finding${needFindings === 1 ? "" : "s"} need you`, tone: "attention" };
  if (gate.status === "review") return { text: `review · ${passed}/${total} checks`, tone: "review" };
  if (gate.status === "passed") return { text: "gate passed", tone: "done" };
  return { text: `checks ${passed}/${total}`, tone: "working" };
}

function segTone(status: string): string {
  switch (status) {
    case "passed":
      return "done";
    case "failed":
      return "trouble";
    case "running":
      return "working";
    default:
      return "pending";
  }
}

export function GateProgressStrip({ cardId }: { cardId: string }) {
  const store = useStore();
  const [gate, setGate] = useState<GateRun | null>(null);

  useEffect(() => {
    let cancelled = false;
    store
      .getGateRun(cardId)
      .then((g) => !cancelled && setGate(g))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [store, cardId]);

  if (!gate || gate.checks.length === 0) return null;

  const phase = phaseLabel(gate);
  const needFindings = gate.findings.some((f) => f.klass === "intent" && !f.resolution);

  return (
    <div className="card-gate" title="Verification gate progress">
      <div className="card-gate-bar" aria-hidden>
        {gate.checks.map((c, i) => (
          <span key={i} className={`card-gate-seg tone-${segTone(c.status)}`} />
        ))}
        {/* The review step as one trailing segment. */}
        <span
          className={`card-gate-seg card-gate-review tone-${
            needFindings ? "attention" : gate.status === "passed" ? "done" : gate.status === "review" ? "review" : "pending"
          }`}
        />
      </div>
      <span className={`card-gate-label tone-${phase.tone}`}>{phase.text}</span>
    </div>
  );
}
