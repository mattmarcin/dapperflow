// The onboarding-audit offer (product.md / Card sources: onboarding audit).
// Appears after a project registers, and on demand from the Projects tree for
// re-audits. Offer, never force: "not now" is a first-class answer, and the audit
// never auto-runs. Findings land in Inbox only, each with file:line evidence, and
// completion raises exactly one audit_digest Needs You item.

import { useState } from "react";
import { AuditDepth } from "../model";
import { useStore } from "../state/store";
import { Modal } from "./Modal";

export function AuditOfferModal() {
  const store = useStore();
  const project = store.auditOffer;
  const [busy, setBusy] = useState<AuditDepth | null>(null);
  if (!project) return null;

  const run = async (depth: AuditDepth) => {
    if (busy) return;
    setBusy(depth);
    try {
      const res = await store.startAudit(project.id, depth);
      if (res.ok) {
        store.flash(
          `Audit ${depth === "deep" ? "(deep) " : ""}running on ${project.name}. Findings land in Inbox.`,
        );
        store.dismissAuditOffer();
      } else {
        store.flash(res.error ?? "The audit could not start.", { tone: "danger" });
      }
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal
      title={`Audit ${project.name}?`}
      subtitle="One agent run that turns a cold repo into a warm project: a seeded backlog with file:line evidence, seeded notes, and a project profile."
      onClose={store.dismissAuditOffer}
      width={520}
      footer={
        <button className="btn-ghost" onClick={store.dismissAuditOffer}>
          Not now
        </button>
      }
    >
      <div className="audit-choices">
        <button
          type="button"
          className="audit-choice"
          onClick={() => void run("quick")}
          disabled={busy !== null}
        >
          <span className="audit-choice-head">
            <span className="audit-choice-name">{busy === "quick" ? "Scanning…" : "Quick scan"}</span>
            <span className="audit-choice-budget">up to 10 cards · 6 notes</span>
          </span>
          <span className="audit-choice-desc">
            A fast pass over structure, risks, and obvious gaps. Findings file into Inbox for
            your triage; nothing is fixed or shipped.
          </span>
        </button>
        <button
          type="button"
          className="audit-choice"
          onClick={() => void run("deep")}
          disabled={busy !== null}
        >
          <span className="audit-choice-head">
            <span className="audit-choice-name">{busy === "deep" ? "Scanning…" : "Deep audit"}</span>
            <span className="audit-choice-budget">up to 25 cards · 12 notes</span>
          </span>
          <span className="audit-choice-desc">
            A thorough read: dependencies, test gaps, duplicated logic, and a richer project
            profile. Same rules - Inbox only, evidence required.
          </span>
        </button>
      </div>
      <p className="audit-fineprint">
        The audit recipe has no ship stage and its session cannot advance the cards it files.
      </p>
    </Modal>
  );
}
