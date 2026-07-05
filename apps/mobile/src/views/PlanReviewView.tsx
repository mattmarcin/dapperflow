import { useEffect, useState } from "react";
import { useStore } from "../state/app-store";
import { PlanArtifact } from "../client/model";
import { Sheet } from "../components/Sheet";

// A sample plan for the fallback path: live daemons without an artifact endpoint yet
// (they do not exist at M6) still render a complete, honest review surface. Labeled so
// it is never mistaken for a real plan.
const SAMPLE_PLAN: PlanArtifact = {
  id: "sample-plan",
  card_id: "sample",
  card_title: "Sample plan (no artifact endpoint yet)",
  project_name: null,
  round: 1,
  status: "awaiting_review",
  summary: "This daemon has not served a plan artifact for this card. Showing a sample so the review surface is complete.",
  layoutWarnings: [],
  sections: [
    { heading: "Why you are seeing this", body: ["The artifact endpoints (artifact.get) land with the Plan Studio milestone. Until then the phone renders a sample so the approve flow is demonstrable end to end."] },
  ],
};

// Plan-round review, read-only-plus-approve. v1 supports exactly two actions: approve,
// and one overall chat-feedback note. No inline annotation, no quote anchors - that is
// the touch-tuned artifact chrome of M7 (mobile.md 2.4). Plan approve is a destructive-
// forward action, so it takes an explicit confirm (the app-enforced guard that stands in
// for the native biometric gate the PWA cannot raise; security.md 5.3).
export function PlanReviewView({ cardId }: { cardId: string }) {
  const store = useStore();
  const [plan, setPlan] = useState<PlanArtifact | null>(null);
  const [loading, setLoading] = useState(true);
  const [feedback, setFeedback] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [approved, setApproved] = useState(false);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    store.source
      .getPlan(cardId)
      .then((p) => {
        if (alive) setPlan(p ?? SAMPLE_PLAN);
      })
      .catch(() => {
        if (alive) setPlan(SAMPLE_PLAN);
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [cardId, store.source]);

  const approve = async () => {
    if (!plan) return;
    setBusy(true);
    const res = await store.approvePlan(plan.id, feedback);
    setBusy(false);
    if (res.ok) setApproved(true);
    setConfirming(false);
  };

  return (
    <Sheet title="Plan review" onClose={store.closeOverlay}>
      {loading || !plan ? (
        <div className="plan-loading">Loading plan…</div>
      ) : (
        <div className="plan">
          <div className="plan-meta">
            <span className="plan-round">Round {plan.round}</span>
            {plan.project_name ? <span className="plan-proj">{plan.project_name}</span> : null}
            <span className={`plan-status status-${plan.status}`}>
              {plan.status === "approved" || approved ? "Approved" : "Awaiting review"}
            </span>
          </div>
          <h3 className="plan-title">{plan.card_title}</h3>

          {plan.layoutWarnings.length > 0 ? (
            <div className="plan-warn" role="note">
              Layout audit at 390px flagged: {plan.layoutWarnings.join("; ")}
            </div>
          ) : null}

          {plan.html ? (
            // A served artifact document renders read-only in a sandboxed frame (opaque
            // origin, no same-origin, no forms). Structured sections are the fallback.
            <iframe className="plan-frame" sandbox="allow-scripts" srcDoc={plan.html} title="Plan artifact" />
          ) : (
            <div className="plan-doc">
              <p className="plan-summary">{plan.summary}</p>
              {plan.sections.map((sec, i) => (
                <section className="plan-section" key={i}>
                  <h4 className="plan-h">{sec.heading}</h4>
                  {sec.body.map((line, j) => (
                    <p className="plan-p" key={j}>{line}</p>
                  ))}
                </section>
              ))}
            </div>
          )}

          {approved || plan.status === "approved" ? (
            <div className="plan-approved">
              <CheckGlyph />
              <div>
                <strong>Plan approved.</strong>
                <span> Implementation continues on the desktop.</span>
              </div>
              <button className="btn-primary btn-sm" onClick={store.closeOverlay}>Done</button>
            </div>
          ) : (
            <div className="plan-actions">
              <label className="field-label" htmlFor="plan-feedback">Overall feedback (optional)</label>
              <textarea
                id="plan-feedback"
                className="plan-feedback"
                placeholder="One note to the agent before you approve…"
                value={feedback}
                onChange={(e) => setFeedback(e.target.value)}
                rows={3}
              />
              {confirming ? (
                <div className="confirm-row">
                  <span className="confirm-text">Approve this plan?</span>
                  <button className="btn-ghost btn-sm" onClick={() => setConfirming(false)} disabled={busy}>Cancel</button>
                  <button className="btn-primary btn-sm" onClick={approve} disabled={busy}>
                    {busy ? "Approving…" : "Confirm approve"}
                  </button>
                </div>
              ) : (
                <button className="btn-primary btn-block" onClick={() => setConfirming(true)}>
                  Approve plan
                </button>
              )}
              <p className="plan-guard-note">
                Approve takes a confirm here; the native app gates it behind biometrics (M7).
              </p>
            </div>
          )}
        </div>
      )}
    </Sheet>
  );
}

function CheckGlyph() {
  return (
    <svg width="20" height="20" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M3 8.4l3 3L13 4.5" />
    </svg>
  );
}
