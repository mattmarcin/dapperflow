// The Plan tab: Plan Studio's review chrome, live in the card workspace
// (plan-studio.md "The loop", promoted from the spike 5 chrome).
//
// The artifact renders in the sandboxed ArtifactFrame; everything the human does
// here (text-range annotations with the four anchor states, native-control
// decisions keyed by question keys, diagram-node comments, data-action clicks, an
// overall note) accumulates VISIBLY in the feedback queue and sends as ONE batch
// (Enter), which is the poll payload the planning agent receives. Approve is a
// first-class action recorded as plan_approved; it resolves the card's plan_round
// Needs You item and the loop ends.
//
// Feedback is never lost (plan-studio.md): the queued draft persists per artifact
// across GUI reloads, and an annotation whose anchor is lost still delivers its
// quote + body.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../../state/store";
import { Card } from "../../model";
import { ArtifactFrame, ArtifactFrameHandle } from "../../review/ArtifactFrame";
import {
  AnchorStatus,
  ArtifactMeta,
  FeedbackItem,
  FeedbackSubmit,
  FromSdkMessage,
  LayoutWarning,
  NativeControlType,
  PROTOCOL_VERSION,
  ReviewMode,
  TextAnchor,
} from "../../review/protocol";

interface AnnotationItem {
  id: string;
  anchor: TextAnchor;
  status: AnchorStatus;
  body: string;
}
interface ControlItem {
  question_key: string;
  value: string | string[] | boolean;
  control_type: NativeControlType;
  label?: string;
}
interface DiagramItem {
  key: string;
  diagram: string;
  node: string;
  label: string;
  body: string;
}
interface ActionItem {
  id: string;
  action: string;
  data?: Record<string, string>;
}

// The queued draft persisted per artifact (plan-studio.md: feedback is never lost).
interface PlanDraft {
  artifactId: string;
  annotations: AnnotationItem[];
  controls: [string, ControlItem][];
  diagramNodes: DiagramItem[];
  actions: ActionItem[];
  chat: string;
}

const draftKey = (artifactId: string) => `dflow.plan-draft.${artifactId}`;

function loadDraft(artifactId: string): PlanDraft | null {
  try {
    const raw = window.localStorage.getItem(draftKey(artifactId));
    if (!raw) return null;
    const d = JSON.parse(raw) as PlanDraft;
    return d && d.artifactId === artifactId ? d : null;
  } catch {
    return null;
  }
}

function saveDraft(d: PlanDraft): void {
  try {
    const empty =
      d.annotations.length === 0 &&
      d.controls.length === 0 &&
      d.diagramNodes.length === 0 &&
      d.actions.length === 0 &&
      !d.chat.trim();
    if (empty) window.localStorage.removeItem(draftKey(d.artifactId));
    else window.localStorage.setItem(draftKey(d.artifactId), JSON.stringify(d));
  } catch {
    /* storage unavailable: the queue lives for the session only */
  }
}

function clearDraft(artifactId: string): void {
  try {
    window.localStorage.removeItem(draftKey(artifactId));
  } catch {
    /* no-op */
  }
}

const STATUS_LABEL: Record<AnchorStatus, string> = {
  anchored: "anchored",
  drifted: "drifted",
  reanchored: "re-anchored",
  unanchored: "anchor lost",
};

export function PlanTab({ card }: { card: Card }) {
  const store = useStore();
  const [meta, setMeta] = useState<ArtifactMeta | null | "loading">("loading");

  useEffect(() => {
    let cancelled = false;
    setMeta("loading");
    store
      .getPlanArtifact(card.id)
      .then((m) => {
        if (!cancelled) setMeta(m);
      })
      .catch(() => {
        if (!cancelled) setMeta(null);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [card.id]);

  if (meta === "loading") {
    return <div className="pf-loading">Loading plan…</div>;
  }
  if (!meta) {
    return (
      <div className="pf-none">
        <div className="pf-none-inner">
          <svg width="44" height="44" viewBox="0 0 44 44" fill="none" aria-hidden className="pf-none-mark">
            <rect x="8" y="6" width="28" height="32" rx="3" stroke="#c98bdb" strokeWidth="1.6" />
            <path d="M14 14h16M14 20h16M14 26h9" stroke="#c98bdb" strokeWidth="1.6" strokeLinecap="round" opacity="0.55" />
          </svg>
          <h3 className="pf-none-title">No plan yet</h3>
          <p className="pf-none-body">
            When this card's agent opens a plan round (dflow plan open), the artifact renders here
            for review: annotate, answer its questions, and send one batch of feedback.
          </p>
        </div>
      </div>
    );
  }
  return <PlanReview key={meta.id} card={card} meta={meta} />;
}

// The review chrome proper. Keyed by artifact id so a different artifact remounts
// with a clean slate.
function PlanReview({ card, meta }: { card: Card; meta: ArtifactMeta }) {
  const store = useStore();
  const frameRef = useRef<ArtifactFrameHandle | null>(null);

  const [mode, setMode] = useState<ReviewMode>("explore");
  const [ready, setReady] = useState(false);
  const [round, setRound] = useState(meta.round);
  const [approved, setApproved] = useState(meta.status === "approved");
  const [approving, setApproving] = useState(false);
  const [sending, setSending] = useState(false);

  const [annotations, setAnnotations] = useState<AnnotationItem[]>([]);
  const [controls, setControls] = useState<Map<string, ControlItem>>(new Map());
  const [diagramNodes, setDiagramNodes] = useState<DiagramItem[]>([]);
  const [actions, setActions] = useState<ActionItem[]>([]);
  const [chat, setChat] = useState("");

  const [warnings, setWarnings] = useState<LayoutWarning[]>([]);
  const [masked, setMasked] = useState(false);
  const [revealed, setRevealed] = useState(false);

  // Where last round's notes landed after the agent's revision (the four states).
  const [lastRound, setLastRound] = useState<AnnotationItem[]>([]);
  const lastRoundIdsRef = useRef<Set<string>>(new Set());
  const [lastPayload, setLastPayload] = useState<FeedbackSubmit | null>(null);

  const [rejected, setRejected] = useState(0);
  const [sdkErrors, setSdkErrors] = useState<string[]>([]);

  // Anchors to re-anchor on the NEXT ready: restored draft annotations after a
  // reload, or the just-sent round's after a revision swap.
  const pendingReanchorRef = useRef<{ id: string; anchor: TextAnchor }[] | null>(null);

  // Restore the persisted queue draft once (feedback is never lost).
  const restoredRef = useRef(false);
  useEffect(() => {
    if (restoredRef.current) return;
    restoredRef.current = true;
    const draft = loadDraft(meta.id);
    if (!draft) return;
    setAnnotations(draft.annotations);
    setControls(new Map(draft.controls));
    setDiagramNodes(draft.diagramNodes);
    setActions(draft.actions);
    setChat(draft.chat);
    if (draft.annotations.length > 0) {
      pendingReanchorRef.current = draft.annotations.map((a) => ({ id: a.id, anchor: a.anchor }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meta.id]);

  // Persist the queue draft as it changes.
  useEffect(() => {
    saveDraft({
      artifactId: meta.id,
      annotations,
      controls: [...controls.entries()],
      diagramNodes,
      actions,
      chat,
    });
  }, [meta.id, annotations, controls, diagramNodes, actions, chat]);

  // ---- Inbound SDK messages (identity-checked + schema-validated upstream) ----
  const onMessage = useCallback((msg: FromSdkMessage) => {
    switch (msg.type) {
      case "ready": {
        setReady(true);
        const pending = pendingReanchorRef.current;
        if (pending && pending.length > 0) {
          pendingReanchorRef.current = null;
          frameRef.current?.send({ v: PROTOCOL_VERSION, type: "reanchor", anchors: pending });
        }
        break;
      }
      case "layout_audit":
        setWarnings(msg.warnings);
        setMasked(msg.masked);
        setRevealed(false);
        break;
      case "mode_changed":
        setMode(msg.mode);
        break;
      case "annotation": {
        if (lastRoundIdsRef.current.has(msg.id)) {
          // A sent annotation re-anchored against the revision: update its fate.
          setLastRound((prev) =>
            prev.map((a) => (a.id === msg.id ? { ...a, anchor: msg.anchor, status: msg.status } : a)),
          );
          break;
        }
        setAnnotations((prev) => {
          const idx = prev.findIndex((a) => a.id === msg.id);
          if (idx >= 0) {
            const next = prev.slice();
            next[idx] = { ...next[idx], anchor: msg.anchor, status: msg.status };
            return next;
          }
          return [...prev, { id: msg.id, anchor: msg.anchor, status: msg.status, body: "" }];
        });
        break;
      }
      case "control":
        setControls((prev) => {
          const next = new Map(prev);
          next.set(msg.question_key, {
            question_key: msg.question_key,
            value: msg.value,
            control_type: msg.control_type,
            label: msg.label,
          });
          return next;
        });
        break;
      case "diagram_node":
        setDiagramNodes((prev) => {
          const key = `${msg.diagram}::${msg.node}`;
          if (prev.some((d) => d.key === key)) return prev;
          return [...prev, { key, diagram: msg.diagram, node: msg.node, label: msg.label, body: "" }];
        });
        break;
      case "action":
        setActions((prev) => [...prev, { id: `act-${Date.now()}-${prev.length}`, action: msg.action, data: msg.data }]);
        break;
      case "resize":
        break; // the frame scrolls internally
      case "sdk_error":
        setSdkErrors((prev) => [...prev, `${msg.where}: ${msg.message}`]);
        break;
      default:
        break;
    }
  }, []);

  const onRejected = useCallback(() => setRejected((n) => n + 1), []);

  // ---- Mode toggle (two-way: buttons here, the "a" key inside the iframe) -----
  const applyMode = useCallback((next: ReviewMode) => {
    setMode(next);
    frameRef.current?.send({ v: PROTOCOL_VERSION, type: "set_mode", mode: next });
  }, []);

  const queueCount =
    annotations.length + controls.size + diagramNodes.length + actions.length + (chat.trim() ? 1 : 0);

  // ---- Batch send (Enter): one payload, the specced shape ---------------------
  const sendRound = useCallback(async () => {
    if (queueCount === 0 || sending || approved) return;
    setSending(true);
    const items: FeedbackItem[] = [
      // An unanchored annotation still ships { quote, body } (nothing is lost).
      ...annotations
        .filter((a) => a.body.trim())
        .map((a) => ({
          kind: "text_range" as const,
          anchor: a.anchor,
          status: a.status,
          body: a.body.trim(),
        })),
      ...Array.from(controls.values()).map((c) => ({
        kind: "control" as const,
        question_key: c.question_key,
        value: c.value,
      })),
      ...diagramNodes
        .filter((d) => d.body.trim())
        .map((d) => ({ kind: "diagram_node" as const, diagram: d.diagram, node: d.node, body: d.body.trim() })),
      ...actions.map((a) => ({
        kind: "action" as const,
        action: a.action,
        body: a.data ? JSON.stringify(a.data) : null,
      })),
      ...(chat.trim() ? [{ kind: "chat" as const, body: chat.trim() }] : []),
    ];
    const payload: FeedbackSubmit = {
      artifact_id: meta.id,
      round,
      items,
      layout_warnings: warnings,
    };
    try {
      const res = await store.submitFeedback(payload);
      setLastPayload(payload);
      // Remember where this round's annotations were, to show their fate after the
      // agent's revision.
      const sent = annotations.filter((a) => a.body.trim());
      lastRoundIdsRef.current = new Set(sent.map((a) => a.id));
      setLastRound(sent);
      // Consume the batch; the ball is in the agent's court.
      setAnnotations([]);
      setControls(new Map());
      setDiagramNodes([]);
      setActions([]);
      setChat("");
      clearDraft(meta.id);
      setRound(res.round);
      if (res.revised_doc_id) {
        // The agent revised in place: reload and re-anchor the sent notes against
        // the new render (anchored / drifted / re-anchored / unanchored).
        pendingReanchorRef.current = sent.map((a) => ({ id: a.id, anchor: a.anchor }));
        setReady(false);
        frameRef.current?.loadArtifact(res.revised_doc_id);
        store.flash(`Round ${round} sent. The agent revised the plan - round ${res.round} is up.`);
      } else {
        store.flash(`Round ${round} sent (${items.length} item${items.length === 1 ? "" : "s"}).`);
      }
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setSending(false);
    }
  }, [queueCount, sending, approved, annotations, controls, diagramNodes, actions, chat, meta.id, round, warnings, store]);

  // ---- Approve (first-class; recorded as plan_approved) -----------------------
  const approve = useCallback(async () => {
    if (approving || approved) return;
    setApproving(true);
    try {
      await store.approvePlan(meta.id, card.id);
      setApproved(true);
      clearDraft(meta.id);
      store.flash("Plan approved. The agent proceeds to implementation.");
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setApproving(false);
    }
  }, [approving, approved, store, meta.id, card.id]);

  // Global keys while the Plan tab is mounted: Enter sends, "a" toggles mode.
  // Suppressed while typing in the queue's own fields.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const editing = t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
      if (editing) return;
      if (e.key === "Enter" && queueCount > 0) {
        e.preventDefault();
        void sendRound();
      } else if ((e.key === "a" || e.key === "A") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        applyMode(mode === "annotate" ? "explore" : "annotate");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [queueCount, sendRound, applyMode, mode]);

  const focusAnnotation = useCallback((anchor: TextAnchor) => {
    frameRef.current?.send({ v: PROTOCOL_VERSION, type: "focus_annotation", anchor });
  }, []);

  const removeAnnotation = useCallback((id: string) => {
    setAnnotations((prev) => prev.filter((a) => a.id !== id));
    frameRef.current?.send({ v: PROTOCOL_VERSION, type: "clear_annotation", id });
  }, []);

  const revealMasked = useCallback(() => {
    setRevealed(true);
    frameRef.current?.send({ v: PROTOCOL_VERSION, type: "reveal_masked" });
  }, []);

  const errorCount = useMemo(() => warnings.filter((w) => w.severity === "error").length, [warnings]);
  const warnCount = warnings.length - errorCount;

  return (
    <div className="pf-root">
      <div className="pf-bar">
        <div className="pf-modeswitch" role="group" aria-label="Review mode">
          <button
            className={`pf-mode${mode === "explore" ? " is-active" : ""}`}
            onClick={() => mode !== "explore" && applyMode("explore")}
          >
            Explore
          </button>
          <button
            className={`pf-mode${mode === "annotate" ? " is-active" : ""}`}
            onClick={() => mode !== "annotate" && applyMode("annotate")}
          >
            Annotate
          </button>
          <span className="pf-kbd">A</span>
        </div>

        <div className="pf-bar-mid">
          <span className="pf-round">round {round}</span>
          {approved ? (
            <span className="pf-state is-approved">plan approved</span>
          ) : (
            <span className="pf-state">awaiting your review</span>
          )}
        </div>

        <div className="pf-bar-right">
          <span className="pf-channel" title={`postMessage channel ${PROTOCOL_VERSION}`}>
            <span className={`pf-dot${ready ? " is-on" : ""}`} aria-hidden />
            {ready ? "sdk ready" : "loading"}
            {rejected > 0 ? <span className="pf-rejected">· {rejected} rejected</span> : null}
          </span>
        </div>
      </div>

      {masked && !revealed ? (
        <div className="pf-gate" role="alert">
          <span className="pf-gate-dot" aria-hidden />
          <span className="pf-gate-text">
            <strong>
              {errorCount} rendering error{errorCount === 1 ? "" : "s"}
            </strong>{" "}
            masked this artifact. The findings return to the agent with your next batch, so it can
            fix its own rendering before you spend a round.
          </span>
          <button className="pf-gate-btn" onClick={revealMasked}>
            Show anyway
          </button>
        </div>
      ) : null}

      <div className="pf-body">
        <div className="pf-stage">
          <ArtifactFrame
            ref={frameRef}
            initialDocId={meta.doc_id}
            signUrl={store.signArtifactUrl}
            onMessage={onMessage}
            onRejected={onRejected}
          />
        </div>

        <aside className="pf-side">
          <section className="pf-panel">
            <div className="pf-panel-head">
              <h3 className="pf-panel-title">Feedback queue</h3>
              <span className="pf-count">{queueCount} queued</span>
            </div>

            {annotations.length > 0 ? (
              <div className="pf-group">
                <div className="pf-group-label">Annotations</div>
                {annotations.map((a) => (
                  <div key={a.id} className={`pf-anno status-${a.status}`}>
                    <div className="pf-anno-top">
                      <button
                        className="pf-quote"
                        onClick={() => focusAnnotation(a.anchor)}
                        title="Scroll to this phrase in the plan"
                      >
                        &ldquo;{a.anchor.quote}&rdquo;
                      </button>
                      <span className={`pf-status status-${a.status}`}>{STATUS_LABEL[a.status]}</span>
                      <button className="pf-x" onClick={() => removeAnnotation(a.id)} aria-label="Remove annotation">
                        ×
                      </button>
                    </div>
                    <input
                      className="pf-body-input"
                      placeholder="Your note on this phrase…"
                      value={a.body}
                      onChange={(e) =>
                        setAnnotations((prev) =>
                          prev.map((x) => (x.id === a.id ? { ...x, body: e.target.value } : x)),
                        )
                      }
                    />
                  </div>
                ))}
              </div>
            ) : null}

            {controls.size > 0 ? (
              <div className="pf-group">
                <div className="pf-group-label">Decisions</div>
                {Array.from(controls.values()).map((c) => (
                  <div key={c.question_key} className="pf-control">
                    <span className="pf-ctrl-key">{c.label ?? c.question_key}</span>
                    <span className="pf-ctrl-val">{renderValue(c.value)}</span>
                  </div>
                ))}
              </div>
            ) : null}

            {diagramNodes.length > 0 ? (
              <div className="pf-group">
                <div className="pf-group-label">Diagram nodes</div>
                {diagramNodes.map((d) => (
                  <div key={d.key} className="pf-anno">
                    <div className="pf-anno-top">
                      <span className="pf-quote is-node">
                        {d.node} <span className="pf-muted">({d.label})</span>
                      </span>
                      <button
                        className="pf-x"
                        onClick={() => setDiagramNodes((prev) => prev.filter((x) => x.key !== d.key))}
                        aria-label="Remove node comment"
                      >
                        ×
                      </button>
                    </div>
                    <input
                      className="pf-body-input"
                      placeholder="Your note on this node…"
                      value={d.body}
                      onChange={(e) =>
                        setDiagramNodes((prev) =>
                          prev.map((x) => (x.key === d.key ? { ...x, body: e.target.value } : x)),
                        )
                      }
                    />
                  </div>
                ))}
              </div>
            ) : null}

            {actions.length > 0 ? (
              <div className="pf-group">
                <div className="pf-group-label">Actions</div>
                {actions.map((a) => (
                  <div key={a.id} className="pf-control">
                    <span className="pf-ctrl-key">{a.action}</span>
                    <span className="pf-ctrl-val">{a.data ? JSON.stringify(a.data) : ""}</span>
                  </div>
                ))}
              </div>
            ) : null}

            <div className="pf-group">
              <div className="pf-group-label">Overall</div>
              <textarea
                className="pf-chat"
                placeholder="Direction feedback for the whole plan…"
                value={chat}
                onChange={(e) => setChat(e.target.value)}
              />
            </div>

            {queueCount === 0 ? (
              <p className="pf-empty-line">
                Nothing queued. Switch to Annotate and select a phrase, answer a control in the
                plan, or click a diagram node.
              </p>
            ) : null}

            <div className="pf-send-row">
              <button
                className="btn-primary pf-send"
                onClick={() => void sendRound()}
                disabled={queueCount === 0 || sending || approved}
              >
                {sending ? "Sending…" : `Send round ${round}`}
                {!sending && queueCount > 0 ? <span className="pf-kbd inline">Enter</span> : null}
              </button>
              <button className="pf-approve" onClick={() => void approve()} disabled={approving || approved}>
                {approved ? "Approved" : approving ? "Approving…" : "Approve plan"}
              </button>
            </div>
          </section>

          {lastRound.length > 0 ? (
            <section className="pf-panel">
              <div className="pf-panel-head">
                <h3 className="pf-panel-title">Last round's notes</h3>
                <span className="pf-count">after revision</span>
              </div>
              {lastRound.map((a) => (
                <div key={a.id} className={`pf-fate status-${a.status}`}>
                  <span className={`pf-status status-${a.status}`}>{STATUS_LABEL[a.status]}</span>
                  <button className="pf-quote is-small" onClick={() => focusAnnotation(a.anchor)}>
                    &ldquo;{a.anchor.quote}&rdquo;
                  </button>
                </div>
              ))}
              <p className="pf-hint">
                How your sent notes fared in the agent's revision. A lost anchor still delivered
                its quote and note.
              </p>
            </section>
          ) : null}

          <section className="pf-panel">
            <div className="pf-panel-head">
              <h3 className="pf-panel-title">Layout audit</h3>
              <span className="pf-count">
                {errorCount > 0 ? <span className="pf-pill err">{errorCount} error</span> : null}
                {warnCount > 0 ? <span className="pf-pill warn">{warnCount} warn</span> : null}
                {warnings.length === 0 ? <span className="pf-pill ok">clean</span> : null}
              </span>
            </div>
            {warnings.length === 0 ? (
              <p className="pf-empty-line">No findings. The artifact renders cleanly at this width.</p>
            ) : (
              <ul className="pf-warnlist">
                {warnings.map((w, i) => (
                  <li key={i} className={`pf-warn sev-${w.severity}`}>
                    <span className={`pf-warn-kind sev-${w.severity}`}>{w.kind}</span>
                    <code className="pf-selector">{w.selector}</code>
                    <span className="pf-warn-px">
                      {w.overflow_px}px · vw {w.viewport_width}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {lastPayload ? (
            <details className="pf-payload">
              <summary>Last sent payload</summary>
              <pre className="pf-json">{JSON.stringify(lastPayload, null, 2)}</pre>
            </details>
          ) : null}

          {sdkErrors.length > 0 ? (
            <section className="pf-panel is-bad">
              <div className="pf-panel-head">
                <h3 className="pf-panel-title">SDK errors</h3>
                <span className="pf-count">{sdkErrors.length}</span>
              </div>
              <pre className="pf-json bad">{sdkErrors.join("\n")}</pre>
            </section>
          ) : null}
        </aside>
      </div>
    </div>
  );
}

function renderValue(v: string | string[] | boolean): string {
  if (typeof v === "boolean") return v ? "yes" : "no";
  if (Array.isArray(v)) return v.join(", ");
  return v || "(empty)";
}
