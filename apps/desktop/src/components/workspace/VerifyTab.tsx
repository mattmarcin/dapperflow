// The Verify tab: the verification gate on the card workspace (gate.md; product.md
// gate run -> Verifying lane). It shows the gate run's progress - the check commands
// with pass/fail and captured output, the adversarial review, and the ship step - and
// renders escalated findings in the Plan Studio review chrome (reuse src/review) as
// approve / fix / skip per finding. The PR state (open / CI / merged) carries a Merge
// action that stays disabled until the PR is green and every finding resolved.
//
// Fixture-tolerant: a realistic gate-run fixture drives every state; live mode shows the
// empty state until the daemon's gate engine serves gate runs.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../../state/store";
import {
  Card,
  FindingResolution,
  GateCheck,
  GateFinding,
  GateRun,
  PrState,
  Session,
} from "../../model";
import { ArtifactFrame, ArtifactFrameHandle } from "../../review/ArtifactFrame";
import { FromSdkMessage, PROTOCOL_VERSION, ReviewMode } from "../../review/protocol";
import { HarnessGlyph, harnessLabel } from "../../lib/glyphs";

interface Props {
  card: Card;
  sessions: Session[];
}

const RESOLUTION_LABEL: Record<FindingResolution, string> = {
  approve: "approve as-is",
  fix: "send back to fix",
  skip: "skip for now",
};

const SEVERITY_ORDER: Record<GateFinding["severity"], number> = { blocker: 0, major: 1, minor: 2 };

export function VerifyTab({ card, sessions }: Props) {
  const store = useStore();
  const [gate, setGate] = useState<GateRun | null | "loading">("loading");
  const [starting, setStarting] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setGate("loading");
    store
      .getGateRun(card.id)
      .then((g) => !cancelled && setGate(g))
      .catch(() => !cancelled && setGate(null));
    return () => {
      cancelled = true;
    };
  }, [store, card.id]);

  const runVerification = useCallback(async () => {
    setStarting(true);
    try {
      const res = await store.startGate(card.id);
      if (!res.ok) {
        store.flash(res.error ?? "Could not start verification.", { tone: "danger" });
        return;
      }
      const g = await store.getGateRun(card.id);
      setGate(g);
      store.flash("Verification started. Checks are running in a gate worktree.");
    } finally {
      setStarting(false);
    }
  }, [store, card.id]);

  if (gate === "loading") return <div className="vf-loading">Loading gate run…</div>;

  if (!gate) {
    const live = sessions.some((s) => s.state === "working" || s.state === "starting");
    return (
      <div className="vf-none">
        <div className="vf-none-inner">
          <svg width="46" height="46" viewBox="0 0 48 48" fill="none" aria-hidden className="vf-none-mark">
            <path d="M24 5l15 5v10c0 9-6.3 15.8-15 18.7C15.3 35.8 9 29 9 20V10z" stroke="#7bd0a8" strokeWidth="1.7" strokeLinejoin="round" />
            <path d="M17 23.5l4.8 4.8L32 18" stroke="#7bd0a8" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          <h3 className="vf-none-title">Not verified yet</h3>
          <p className="vf-none-body">
            Nothing becomes a PR because an agent says it is done. Run the gate to send this branch through the
            registered checks and an adversarial review on a different harness. Findings that touch intent come
            back here for your approve / fix / skip.
          </p>
          <button className="btn-primary vf-none-go" onClick={runVerification} disabled={starting || live}>
            {starting ? "Starting…" : "Run verification"}
          </button>
          {live ? <span className="vf-none-hint">This card still has an agent working. Verify once it reports done.</span> : null}
        </div>
      </div>
    );
  }

  return <GatePanel key={gate.id} card={card} gate={gate} setGate={setGate} onRerun={runVerification} rerunning={starting} />;
}

// ---- Gate status vocabulary -------------------------------------------------

function gateStatusMeta(status: GateRun["status"]): { label: string; tone: string } {
  switch (status) {
    case "pending":
      return { label: "Queued", tone: "quiet" };
    case "checks_running":
      return { label: "Running checks", tone: "working" };
    case "review":
      return { label: "Adversarial review", tone: "review" };
    case "awaiting_human":
      return { label: "Findings need you", tone: "attention" };
    case "autofixing":
      return { label: "Autofixing", tone: "working" };
    case "passed":
      return { label: "Gate passed", tone: "done" };
    case "failed":
      return { label: "Gate failed", tone: "trouble" };
    default:
      return { label: String(status), tone: "quiet" };
  }
}

function checksVerdict(checks: GateCheck[]): "passed" | "failed" | "running" | "pending" {
  if (checks.length === 0) return "pending";
  if (checks.some((c) => c.status === "failed")) return "failed";
  if (checks.some((c) => c.status === "running" || c.status === "pending")) return "running";
  if (checks.every((c) => c.status === "passed" || c.status === "skipped")) return "passed";
  return "pending";
}

// ---- The gate panel proper --------------------------------------------------

function GatePanel({
  card,
  gate,
  setGate,
  onRerun,
  rerunning,
}: {
  card: Card;
  gate: GateRun;
  setGate: (g: GateRun) => void;
  onRerun: () => void;
  rerunning: boolean;
}) {
  const store = useStore();
  const frameRef = useRef<ArtifactFrameHandle | null>(null);
  const [mode, setMode] = useState<ReviewMode>("explore");
  const [ready, setReady] = useState(false);
  const [rejected, setRejected] = useState(0);
  const [applying, setApplying] = useState(false);
  const [note, setNote] = useState("");
  // Pending (unsent) resolutions the human picked in the review chrome, keyed by finding id.
  const [picks, setPicks] = useState<Map<string, FindingResolution>>(new Map());

  const intentFindings = useMemo(
    () =>
      gate.findings
        .filter((f) => f.klass === "intent")
        .sort((a, b) => SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity]),
    [gate.findings],
  );
  const autoFindings = useMemo(() => gate.findings.filter((f) => f.klass !== "intent"), [gate.findings]);
  const awaitingHuman = gate.status === "awaiting_human" && intentFindings.some((f) => !f.resolution);
  const showFindingsReview = awaitingHuman && !!gate.findings_doc_id;

  // Merge existing resolutions (already-applied) with pending picks for the queue view.
  const effectiveResolution = useCallback(
    (f: GateFinding): FindingResolution | null => picks.get(f.id) ?? f.resolution ?? null,
    [picks],
  );
  const allDecided = intentFindings.every((f) => effectiveResolution(f) !== null);

  // Poll while the run is in flight so running checks and the CI-green transition land live.
  useEffect(() => {
    const inFlight =
      gate.status === "checks_running" ||
      gate.status === "review" ||
      gate.status === "autofixing" ||
      gate.pr.status === "pushing" ||
      gate.pr.status === "ci_running";
    if (!inFlight) return;
    const t = window.setInterval(() => {
      store.getGateRun(card.id).then((g) => g && setGate(g)).catch(() => undefined);
    }, 2500);
    return () => window.clearInterval(t);
  }, [gate.status, gate.pr.status, card.id, store, setGate]);

  const onMessage = useCallback((msg: FromSdkMessage) => {
    switch (msg.type) {
      case "ready":
        setReady(true);
        break;
      case "mode_changed":
        setMode(msg.mode);
        break;
      case "control": {
        if (typeof msg.value !== "string") break;
        if (msg.question_key === "gate.note") {
          setNote(msg.value);
          break;
        }
        const m = /^finding\.(.+)$/.exec(msg.question_key);
        if (m && (msg.value === "approve" || msg.value === "fix" || msg.value === "skip")) {
          const findingId = m[1];
          const resolution = msg.value;
          setPicks((prev) => {
            const next = new Map(prev);
            next.set(findingId, resolution);
            return next;
          });
        }
        break;
      }
      default:
        break;
    }
  }, []);

  const onRejected = useCallback(() => setRejected((n) => n + 1), []);

  const applyMode = useCallback((next: ReviewMode) => {
    setMode(next);
    frameRef.current?.send({ v: PROTOCOL_VERSION, type: "set_mode", mode: next });
  }, []);

  const applyResolutions = useCallback(async () => {
    if (!allDecided || applying) return;
    setApplying(true);
    try {
      let latest = gate;
      for (const f of intentFindings) {
        const r = effectiveResolution(f);
        if (r && f.resolution !== r) {
          latest = await store.resolveGateFinding(card.id, f.id, r);
        }
      }
      setGate(latest);
      setPicks(new Map());
      const advanced = latest.status === "passed";
      store.flash(
        advanced
          ? "All findings resolved. The gate passed and opened a PR."
          : "Resolutions applied.",
      );
    } catch (e) {
      store.flash(String(e instanceof Error ? e.message : e), { tone: "danger" });
    } finally {
      setApplying(false);
    }
  }, [allDecided, applying, gate, intentFindings, effectiveResolution, store, card.id, setGate]);

  const meta = gateStatusMeta(gate.status);
  const verdict = checksVerdict(gate.checks);

  return (
    <div className="vf-root">
      <div className="vf-bar">
        <div className="vf-bar-left">
          <span className={`vf-status tone-${meta.tone}`}>
            <span className="vf-status-dot" aria-hidden />
            {meta.label}
          </span>
          <span className="vf-bar-meta">
            {gate.mode === "checks_only" ? "checks-only gate" : "full gate"}
            {gate.reviewer_harness ? (
              <>
                {" · reviewer "}
                <span className="vf-reviewer">
                  <HarnessGlyph harness={gate.reviewer_harness} />
                  {harnessLabel(gate.reviewer_harness)}
                </span>
              </>
            ) : null}
          </span>
        </div>
        <div className="vf-bar-right">
          {showFindingsReview ? (
            <div className="pf-modeswitch" role="group" aria-label="Review mode">
              <button className={`pf-mode${mode === "explore" ? " is-active" : ""}`} onClick={() => mode !== "explore" && applyMode("explore")}>
                Explore
              </button>
              <button className={`pf-mode${mode === "annotate" ? " is-active" : ""}`} onClick={() => mode !== "annotate" && applyMode("annotate")}>
                Annotate
              </button>
              <span className="pf-kbd">A</span>
            </div>
          ) : null}
          <button className="btn-ghost btn-sm" onClick={onRerun} disabled={rerunning}>
            {rerunning ? "Starting…" : "Re-run gate"}
          </button>
        </div>
      </div>

      <div className="vf-body">
        <div className="vf-main">
          {/* The pipeline: Checks -> Review -> Ship (a real sequence, not decoration). */}
          <div className="vf-rail">
            <RailStep
              n={1}
              title="Checks"
              status={verdict === "passed" ? "done" : verdict === "failed" ? "failed" : verdict === "running" ? "running" : "pending"}
              note={`${gate.checks.filter((c) => c.status === "passed").length}/${gate.checks.length} passed`}
            />
            <RailStep
              n={2}
              title="Review"
              status={
                gate.status === "review"
                  ? "running"
                  : awaitingHuman
                    ? "attention"
                    : gate.status === "passed" || gate.pr.status !== "none"
                      ? "done"
                      : verdict === "passed"
                        ? "running"
                        : "pending"
              }
              note={
                awaitingHuman
                  ? `${intentFindings.filter((f) => !effectiveResolution(f)).length} finding${intentFindings.length === 1 ? "" : "s"} need you`
                  : gate.reviewer_harness
                    ? `adversarial · ${harnessLabel(gate.reviewer_harness)}`
                    : "adversarial"
              }
            />
            <RailStep
              n={3}
              title="Ship"
              status={gate.pr.status === "merged" ? "done" : gate.pr.status !== "none" ? "running" : gate.status === "passed" ? "ready" : "pending"}
              note={gate.pr.number ? `PR #${gate.pr.number}` : card.project_id ? "opens on pass" : "local merge"}
              last
            />
          </div>

          {/* Checks with captured output (gate.md step 1: output captured as evidence). */}
          <section className="vf-checks">
            <div className="vf-section-head">
              <h3 className="vf-section-title">Checks</h3>
              <span className={`pf-pill ${verdict === "passed" ? "ok" : verdict === "failed" ? "err" : "warn"}`}>
                {verdict === "passed" ? "all green" : verdict === "failed" ? "failing" : verdict === "running" ? "running" : "queued"}
              </span>
            </div>
            <div className="vf-check-list">
              {gate.checks.map((c) => (
                <CheckRow key={c.name} check={c} />
              ))}
            </div>
          </section>

          {/* The finding review, in the Plan Studio chrome (reuse src/review). */}
          {showFindingsReview ? (
            <section className="vf-review">
              <div className="vf-section-head">
                <h3 className="vf-section-title">Findings review</h3>
                <span className="pf-channel" title={`postMessage channel ${PROTOCOL_VERSION}`}>
                  <span className={`pf-dot${ready ? " is-on" : ""}`} aria-hidden />
                  {ready ? "sdk ready" : "loading"}
                  {rejected > 0 ? <span className="pf-rejected">· {rejected} rejected</span> : null}
                </span>
              </div>
              <div className="vf-frame">
                <ArtifactFrame
                  ref={frameRef}
                  initialDocId={gate.findings_doc_id!}
                  signUrl={store.signArtifactUrl}
                  onMessage={onMessage}
                  onRejected={onRejected}
                />
              </div>
            </section>
          ) : gate.status === "review" ? (
            <section className="vf-review-progress">
              <span className="vf-spinner" aria-hidden />
              <div>
                <strong>Adversarial review in progress.</strong>
                <p>
                  A {gate.reviewer_harness ? harnessLabel(gate.reviewer_harness) : "reviewer"} session on a
                  different harness than the author is reading the diff against the card's acceptance criteria.
                  Findings that touch intent will appear here for your judgment.
                </p>
              </div>
            </section>
          ) : gate.findings.length === 0 && gate.status === "passed" ? (
            <section className="vf-review-clean">
              <IconCheckCircle />
              <div>
                <strong>Review clean.</strong>
                <p>The adversarial reviewer raised nothing that needs your judgment. The branch earned its way out.</p>
              </div>
            </section>
          ) : null}
        </div>

        <aside className="vf-side">
          {showFindingsReview ? (
            <section className="pf-panel">
              <div className="pf-panel-head">
                <h3 className="pf-panel-title">Resolutions</h3>
                <span className="pf-count">
                  {intentFindings.filter((f) => effectiveResolution(f)).length}/{intentFindings.length} decided
                </span>
              </div>
              <div className="vf-res-list">
                {intentFindings.map((f) => {
                  const r = effectiveResolution(f);
                  return (
                    <div key={f.id} className={`vf-res${r ? ` is-${r}` : " is-undecided"}`}>
                      <span className={`vf-res-sev sev-${f.severity}`}>{f.severity}</span>
                      <span className="vf-res-title">{f.title}</span>
                      <span className="vf-res-verdict">{r ? RESOLUTION_LABEL[r] : "undecided"}</span>
                    </div>
                  );
                })}
              </div>
              {note.trim() ? (
                <div className="vf-res-note">
                  <span className="pf-group-label">Note to author</span>
                  <p>{note.trim()}</p>
                </div>
              ) : null}
              <button className="btn-primary vf-apply" onClick={() => void applyResolutions()} disabled={!allDecided || applying}>
                {applying ? "Applying…" : allDecided ? "Apply resolutions" : "Decide every finding first"}
              </button>
              <p className="vf-res-hint">
                Pick approve / fix / skip on each finding in the review. Applying advances the gate: fixes go back
                to the fixer, then the branch ships.
              </p>
            </section>
          ) : null}

          <PrCard card={card} gate={gate} setGate={setGate} />

          <section className="pf-panel vf-evidence">
            <div className="pf-panel-head">
              <h3 className="pf-panel-title">Gate evidence</h3>
            </div>
            <dl className="vf-evidence-grid">
              <div>
                <dt>worktree</dt>
                <dd className="mono">{gate.worktree_id.slice(-8)}</dd>
              </div>
              <div>
                <dt>lease class</dt>
                <dd>gate</dd>
              </div>
              <div>
                <dt>reviewer</dt>
                <dd>{gate.reviewer_harness ? harnessLabel(gate.reviewer_harness) : "-"}</dd>
              </div>
              <div>
                <dt>auto-fixed</dt>
                <dd>{autoFindings.length}</dd>
              </div>
            </dl>
            <p className="vf-evidence-note">
              Every check output and finding is a card event with an evidence pointer, so the timeline shows why
              the branch was allowed out.
            </p>
          </section>
        </aside>
      </div>
    </div>
  );
}

function RailStep({
  n,
  title,
  status,
  note,
  last,
}: {
  n: number;
  title: string;
  status: "pending" | "running" | "attention" | "ready" | "done" | "failed";
  note: string;
  last?: boolean;
}) {
  return (
    <div className={`vf-step is-${status}${last ? " is-last" : ""}`}>
      <span className="vf-step-node" aria-hidden>
        {status === "done" ? (
          <svg width="13" height="13" viewBox="0 0 16 16" fill="none"><path d="M3 8.5l3.2 3.2L13 4.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" /></svg>
        ) : status === "failed" ? (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" /></svg>
        ) : (
          <span className="vf-step-num">{n}</span>
        )}
      </span>
      <div className="vf-step-text">
        <span className="vf-step-title">{title}</span>
        <span className="vf-step-note">{note}</span>
      </div>
    </div>
  );
}

function CheckRow({ check }: { check: GateCheck }) {
  const [open, setOpen] = useState(check.status === "failed");
  const hasOutput = !!check.output;
  return (
    <div className={`vf-check status-${check.status}`}>
      <button className="vf-check-head" onClick={() => hasOutput && setOpen((o) => !o)} disabled={!hasOutput} aria-expanded={open}>
        <span className="vf-check-glyph" aria-hidden>
          {check.status === "passed" ? (
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none"><path d="M3 8.5l3.2 3.2L13 4.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" /></svg>
          ) : check.status === "failed" ? (
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" /></svg>
          ) : check.status === "running" ? (
            <span className="vf-check-spin" />
          ) : (
            <span className="vf-check-dot" />
          )}
        </span>
        <span className="vf-check-name">{check.name}</span>
        <code className="vf-check-cmd">{check.cmd}</code>
        <span className="vf-check-timing">
          {check.duration_ms ? `${(check.duration_ms / 1000).toFixed(1)}s` : check.status === "running" ? "running" : ""}
          {typeof check.exit_code === "number" ? ` · exit ${check.exit_code}` : ""}
        </span>
        {hasOutput ? <span className={`vf-check-caret${open ? " is-open" : ""}`} aria-hidden>▾</span> : null}
      </button>
      {open && hasOutput ? <pre className="vf-check-output">{check.output}</pre> : null}
    </div>
  );
}

function PrCard({ card, gate, setGate }: { card: Card; gate: GateRun; setGate: (g: GateRun) => void }) {
  const store = useStore();
  const [merging, setMerging] = useState(false);
  const pr = gate.pr;

  const merge = useCallback(async () => {
    setMerging(true);
    try {
      const res = await store.mergePr(card.id);
      if (!res.ok) {
        store.flash(res.error ?? "Merge failed.", { tone: "danger" });
        return;
      }
      const g = await store.getGateRun(card.id);
      if (g) setGate(g);
      store.flash(`Merged PR #${pr.number} (squash). Worktree returned.`);
    } finally {
      setMerging(false);
    }
  }, [store, card.id, pr.number, setGate]);

  const openPr = () => pr.url && window.open(pr.url, "_blank", "noopener,noreferrer");

  if (pr.status === "none") {
    return (
      <section className="pf-panel vf-pr">
        <div className="pf-panel-head">
          <h3 className="pf-panel-title">Pull request</h3>
        </div>
        <p className="vf-pr-empty">
          No PR yet. The gate pushes and opens one once checks are green and every finding is resolved. Branch:{" "}
          <code>{pr.branch}</code>.
        </p>
      </section>
    );
  }

  const merged = pr.status === "merged";
  const green = ciVerdict(pr);

  return (
    <section className={`pf-panel vf-pr${merged ? " is-merged" : ""}`}>
      <div className="pf-panel-head">
        <h3 className="pf-panel-title">Pull request</h3>
        <span className={`vf-pr-state is-${green}`}>{prStatusLabel(pr)}</span>
      </div>

      <button className="vf-pr-link" onClick={openPr} disabled={!pr.url}>
        <IconBranch />
        <span className="vf-pr-num">#{pr.number}</span>
        <code className="vf-pr-branch">{pr.branch}</code>
      </button>

      {pr.ci.length > 0 ? (
        <div className="vf-ci">
          {pr.ci.map((c) => (
            <span key={c.name} className={`vf-ci-check is-${c.status}`}>
              <span className="vf-ci-dot" aria-hidden />
              {c.name}
            </span>
          ))}
        </div>
      ) : null}

      {pr.fixes_issue ? (
        <p className="vf-pr-fixes">
          Closes <code>{pr.fixes_issue}</code> on merge via the PR body's Fixes line.
        </p>
      ) : null}

      {merged ? (
        <div className="vf-pr-merged">
          <IconCheckCircle />
          Merged with squash. Worktree returned; head is contained.
        </div>
      ) : (
        <>
          <button className="btn-primary vf-merge" onClick={() => void merge()} disabled={!pr.mergeable || merging}>
            {merging ? "Merging…" : pr.mergeable ? "Merge (squash)" : "Merge"}
          </button>
          {!pr.mergeable ? (
            <p className="vf-merge-hint">
              {green === "failure"
                ? "CI is red. Merge stays disabled until it is green."
                : "Waiting on CI. Merge unlocks when every check is green and findings are resolved."}
            </p>
          ) : (
            <p className="vf-merge-hint is-ready">Green across the board. Squash-merge closes this out.</p>
          )}
        </>
      )}
    </section>
  );
}

function ciVerdict(pr: PrState): "success" | "failure" | "running" {
  if (pr.ci.some((c) => c.status === "failure")) return "failure";
  if (pr.ci.some((c) => c.status === "running" || c.status === "queued")) return "running";
  if (pr.ci.length > 0 && pr.ci.every((c) => c.status === "success")) return "success";
  return "running";
}

function prStatusLabel(pr: PrState): string {
  switch (pr.status) {
    case "open":
      return "open";
    case "ci_running":
      return "CI running";
    case "ci_passed":
      return "CI green";
    case "ci_failed":
      return "CI red";
    case "merged":
      return "merged";
    case "pushing":
      return "pushing";
    default:
      return String(pr.status);
  }
}

function IconBranch() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M5 3v10M5 3a1.6 1.6 0 100 3.2M5 6.2v0M11 5a1.6 1.6 0 100 3.2M11 8.2c0 2-2 2.5-4 2.8M5 13a1.6 1.6 0 100-3.2" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function IconCheckCircle() {
  return (
    <svg className="vf-check-circle" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="6.4" stroke="currentColor" strokeWidth="1.3" />
      <path d="M5 8.2l2 2L11 6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
