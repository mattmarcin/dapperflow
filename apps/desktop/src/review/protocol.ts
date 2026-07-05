// Plan Studio wire contract (promoted from spike 5; see
// the design notes for the feasibility proof and the M3
// code map). This is the shared M3 review contract on the app side; the Rust
// `dflow-proto` artifact.* types mirror it.
//
// Two halves live here:
//   1. The postMessage schema between the app (parent, trusted) and the review SDK
//      running inside the sandboxed artifact iframe (child, UNTRUSTED - attacker
//      class 3 in security.md).
//   2. The artifact / feedback / recipe domain types the DataSource speaks
//      (artifact.get, artifact.feedback.submit, recipe.list per protocol.md).
//
// The postMessage half is deliberately allowlisted and versioned:
//
//   - Every message carries `v: PROTOCOL_VERSION`; a mismatch is dropped.
//   - `type` must be one of a fixed set; unknown types are dropped.
//   - The app side re-validates the full shape of every inbound message with the
//     runtime guards below (never trust the child's TypeScript). This is the
//     "allowlisted, versioned message schema validated on the app side" that
//     security.md / Artifact sandbox architecture requires.
//
// The child validates inbound app->SDK messages with the same guards, so a stray
// message from some other frame on the page cannot drive the SDK either.

export const PROTOCOL_VERSION = "dflow.plan.v1";

// ---- Shared value shapes ---------------------------------------------------

export type ReviewMode = "explore" | "annotate";

/** A text-range anchor as specced in plan-studio.md (Feedback payload). */
export interface TextAnchor {
  /** CSS selector for the nearest enclosing element that owns the offsets. */
  selector: string;
  /** Character offset of the selection start within the element's textContent. */
  start: number;
  /** Character offset of the selection end within the element's textContent. */
  end: number;
  /** The exact quoted text, so drift can be detected and re-anchored. */
  quote: string;
}

export type LayoutWarningKind =
  | "horizontal_overflow"
  | "element_overflow"
  | "clipped_text"
  | "overlapping_text"
  | "external_reference"; // CSP posture: an artifact referencing a CDN is an error

export type Severity = "error" | "warning";

/** The structured layout finding shape from plan-studio.md (Layout audit gate). */
export interface LayoutWarning {
  selector: string;
  kind: LayoutWarningKind;
  overflow_px: number;
  viewport_width: number;
  severity: Severity;
  /** Human-readable note for the review chrome; not part of the wire minimum. */
  detail?: string;
}

export type NativeControlType =
  | "radio"
  | "checkbox"
  | "select"
  | "text"
  | "textarea"
  | "contenteditable";

// ---- Child (SDK) -> app messages -------------------------------------------

export interface ReadyMsg {
  v: typeof PROTOCOL_VERSION;
  type: "ready";
  artifactId: string;
  sdkVersion: string;
}

export interface LayoutAuditMsg {
  v: typeof PROTOCOL_VERSION;
  type: "layout_audit";
  warnings: LayoutWarning[];
  viewportWidth: number;
  masked: boolean; // true when error-severity findings masked the artifact
}

export interface ModeChangedMsg {
  v: typeof PROTOCOL_VERSION;
  type: "mode_changed";
  mode: ReviewMode;
}

export interface AnnotationMsg {
  v: typeof PROTOCOL_VERSION;
  type: "annotation";
  id: string; // SDK-minted client id, so the app can dedupe/replace
  anchor: TextAnchor;
  /** Re-anchoring status for this annotation against the current render. */
  status: "anchored" | "drifted" | "reanchored" | "unanchored";
}

export interface ControlMsg {
  v: typeof PROTOCOL_VERSION;
  type: "control";
  question_key: string;
  value: string | string[] | boolean;
  control_type: NativeControlType;
  /** Best-effort human label of the control for the review queue. */
  label?: string;
}

export interface DiagramNodeMsg {
  v: typeof PROTOCOL_VERSION;
  type: "diagram_node";
  diagram: string;
  node: string;
  label: string;
}

export interface ActionMsg {
  v: typeof PROTOCOL_VERSION;
  type: "action";
  action: string;
  data?: Record<string, string>;
}

export interface ResizeMsg {
  v: typeof PROTOCOL_VERSION;
  type: "resize";
  height: number;
}

/** SDK forwards its own runtime errors so the app can prove zero-error happy paths. */
export interface SdkErrorMsg {
  v: typeof PROTOCOL_VERSION;
  type: "sdk_error";
  message: string;
  where: string;
}

export type FromSdkMessage =
  | ReadyMsg
  | LayoutAuditMsg
  | ModeChangedMsg
  | AnnotationMsg
  | ControlMsg
  | DiagramNodeMsg
  | ActionMsg
  | ResizeMsg
  | SdkErrorMsg;

// ---- App -> child (SDK) messages -------------------------------------------

export interface SetModeMsg {
  v: typeof PROTOCOL_VERSION;
  type: "set_mode";
  mode: ReviewMode;
}

export interface FocusAnnotationMsg {
  v: typeof PROTOCOL_VERSION;
  type: "focus_annotation";
  anchor: TextAnchor;
}

export interface ClearAnnotationMsg {
  v: typeof PROTOCOL_VERSION;
  type: "clear_annotation";
  id: string;
}

export interface RevealMaskedMsg {
  v: typeof PROTOCOL_VERSION;
  type: "reveal_masked"; // "Show anyway" from the layout gate banner
}

/** Ask the SDK to re-audit and re-anchor after the agent revised the artifact. */
export interface ReanchorMsg {
  v: typeof PROTOCOL_VERSION;
  type: "reanchor";
  anchors: { id: string; anchor: TextAnchor }[];
}

/**
 * App-initiated annotation by quote: the SDK finds each quote in its own DOM and
 * emits a proper annotation (exact selector + offsets). Doubles as a deterministic
 * way to seed anchors for the re-anchoring demo without a mouse.
 */
export interface AnnotateQuoteMsg {
  v: typeof PROTOCOL_VERSION;
  type: "annotate_quote";
  quotes: { id: string; quote: string }[];
}

export type ToSdkMessage =
  | SetModeMsg
  | FocusAnnotationMsg
  | ClearAnnotationMsg
  | RevealMaskedMsg
  | ReanchorMsg
  | AnnotateQuoteMsg;

// ---- Runtime validation (the trust boundary) -------------------------------

function isObj(x: unknown): x is Record<string, unknown> {
  return typeof x === "object" && x !== null;
}
function isStr(x: unknown): x is string {
  return typeof x === "string";
}
function isNum(x: unknown): x is number {
  return typeof x === "number" && Number.isFinite(x);
}

const FROM_SDK_TYPES = new Set([
  "ready",
  "layout_audit",
  "mode_changed",
  "annotation",
  "control",
  "diagram_node",
  "action",
  "resize",
  "sdk_error",
]);

const LAYOUT_KINDS = new Set<LayoutWarningKind>([
  "horizontal_overflow",
  "element_overflow",
  "clipped_text",
  "overlapping_text",
  "external_reference",
]);

const CONTROL_TYPES = new Set<NativeControlType>([
  "radio",
  "checkbox",
  "select",
  "text",
  "textarea",
  "contenteditable",
]);

function validAnchor(x: unknown): x is TextAnchor {
  return (
    isObj(x) &&
    isStr(x.selector) &&
    isNum(x.start) &&
    isNum(x.end) &&
    isStr(x.quote) &&
    x.start >= 0 &&
    x.end >= x.start &&
    // A hostile artifact could send a megabyte quote to blow up the queue.
    x.quote.length <= 4000 &&
    x.selector.length <= 1000
  );
}

function validLayoutWarning(x: unknown): x is LayoutWarning {
  return (
    isObj(x) &&
    isStr(x.selector) &&
    LAYOUT_KINDS.has(x.kind as LayoutWarningKind) &&
    isNum(x.overflow_px) &&
    isNum(x.viewport_width) &&
    (x.severity === "error" || x.severity === "warning")
  );
}

/**
 * The single inbound guard for app-side validation. Returns the message typed if
 * it is a well-formed, version-matched, allowlisted SDK message, else null.
 * Bounds-checks string lengths and array sizes so a hostile artifact cannot use
 * the channel as a memory-amplification vector.
 */
export function parseFromSdk(raw: unknown): FromSdkMessage | null {
  if (!isObj(raw)) return null;
  if (raw.v !== PROTOCOL_VERSION) return null;
  if (!isStr(raw.type) || !FROM_SDK_TYPES.has(raw.type)) return null;

  switch (raw.type) {
    case "ready":
      return isStr(raw.artifactId) && isStr(raw.sdkVersion) ? (raw as unknown as ReadyMsg) : null;
    case "layout_audit": {
      if (!Array.isArray(raw.warnings) || raw.warnings.length > 500) return null;
      if (!raw.warnings.every(validLayoutWarning)) return null;
      if (!isNum(raw.viewportWidth) || typeof raw.masked !== "boolean") return null;
      return raw as unknown as LayoutAuditMsg;
    }
    case "mode_changed":
      return raw.mode === "explore" || raw.mode === "annotate" ? (raw as unknown as ModeChangedMsg) : null;
    case "annotation": {
      if (!isStr(raw.id) || raw.id.length > 200) return null;
      if (!validAnchor(raw.anchor)) return null;
      const ok = ["anchored", "drifted", "reanchored", "unanchored"].includes(raw.status as string);
      return ok ? (raw as unknown as AnnotationMsg) : null;
    }
    case "control": {
      if (!isStr(raw.question_key) || raw.question_key.length > 200) return null;
      if (!CONTROL_TYPES.has(raw.control_type as NativeControlType)) return null;
      const v = raw.value;
      const okVal =
        isStr(v) ||
        typeof v === "boolean" ||
        (Array.isArray(v) && v.length <= 100 && v.every(isStr));
      if (!okVal) return null;
      if (isStr(v) && v.length > 8000) return null;
      return raw as unknown as ControlMsg;
    }
    case "diagram_node":
      return isStr(raw.diagram) && isStr(raw.node) && isStr(raw.label)
        ? (raw as unknown as DiagramNodeMsg)
        : null;
    case "action": {
      if (!isStr(raw.action) || raw.action.length > 200) return null;
      if (raw.data !== undefined) {
        if (!isObj(raw.data)) return null;
        const entries = Object.entries(raw.data);
        if (entries.length > 50 || !entries.every(([, val]) => isStr(val))) return null;
      }
      return raw as unknown as ActionMsg;
    }
    case "resize":
      return isNum(raw.height) ? (raw as unknown as ResizeMsg) : null;
    case "sdk_error":
      return isStr(raw.message) && isStr(raw.where) ? (raw as unknown as SdkErrorMsg) : null;
    default:
      return null;
  }
}

// ============================================================================
// Artifact + feedback domain contract (protocol.md artifact.*, plan-studio.md)
// ============================================================================

/** Re-anchoring lifecycle of a text-range annotation (plan-studio.md). */
export type AnchorStatus = "anchored" | "drifted" | "reanchored" | "unanchored";

/**
 * A single feedback item, matching the plan-studio.md poll-response shape EXACTLY.
 * The `text_range` anchor's `quote` is the load-bearing anchor: an `unanchored`
 * annotation still delivers `{ anchor: { quote, ... }, body }` so feedback is never
 * lost. A fired `data-action` clickable arrives as an `action` item (the spike's
 * placeholder `actions` array is folded in here). Approve is a first-class `action`.
 */
export type FeedbackItem =
  | { kind: "text_range"; anchor: TextAnchor; status: AnchorStatus; body: string }
  | { kind: "control"; question_key: string; value: string | string[] | boolean }
  | { kind: "diagram_node"; diagram: string; node: string; body: string }
  | { kind: "action"; action: string; body: string | null }
  | { kind: "chat"; body: string };

/** artifact.feedback.submit { artifact_id, round, items } (from the review chrome). */
export interface FeedbackSubmit {
  artifact_id: string;
  round: number;
  items: FeedbackItem[];
  layout_warnings: LayoutWarning[];
}

/**
 * The daemon's response to a submitted round. In fixture mode the agent "revises in
 * place", so the result carries the doc to reload and re-anchor against.
 */
export interface FeedbackSubmitResult {
  ok: boolean;
  round: number; // the next round number
  revised_doc_id?: string | null; // fixture: the revised artifact to reload
  next_step: string;
}

export type ArtifactStatus = "awaiting_feedback" | "revising" | "approved" | "ended" | (string & {});

/**
 * artifact.get { artifact_id } metadata (HTML is served separately over the signed
 * URL, never through this object). `doc_id` is the serving identity the artifact
 * HTTP endpoint knows; `revised_doc_id` is a fixture affordance to demo re-anchoring.
 */
export interface ArtifactMeta {
  id: string;
  card_id: string;
  kind: "plan" | "diagram" | "mockup" | (string & {});
  title: string;
  doc_id: string;
  revised_doc_id?: string | null;
  round: number;
  status: ArtifactStatus;
  created_at: number;
  updated_at: number;
}
