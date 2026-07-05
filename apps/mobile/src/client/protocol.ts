// ============================================================================
// M6 DEBT - thin duplicated protocol mirror.
// This is a minimal, phone-scoped copy of apps/desktop/src/protocol.ts. The M7
// spec (mobile.md 2.2) extracts packages/client-proto + client-core so all three
// shells share one codebase; M6 ships STANDALONE with this duplicated slice and
// carries the drift risk as declared debt. Keep field names byte-identical to the
// desktop mirror and the dflow-proto crate so nothing diverges silently.
// The phone is a read-only-plus-approve client: it never sends PTY input, so the
// input/resize frame encoders are intentionally absent.
// ============================================================================

export const PROTOCOL_VERSION = 1;

export interface Envelope {
  v: number;
  id?: string;
  type: string;
  payload: unknown;
}

export interface ProtocolError {
  code: string;
  message: string;
  retryable: boolean;
  detail?: string;
}

export interface AuthWelcome {
  proto_version: number;
  scope: string;
  daemon_version: string;
}

// --- Styled screen capture (session.attach) --------------------------------
// The read-only terminal peek renders this, NOT an xterm instance (mobile.md 1:
// "styled screen snapshot ... look, do not type"). Per security.md the daemon
// scrubs known secret values out of any capture that leaves a session before it
// reaches a remote client, so the peek is a scrubbed screen image, never a raw
// secret-bearing stream.

export interface CursorPos {
  col: number;
  row: number;
  visible: boolean;
}

export interface StyledRun {
  text: string;
  fg?: string;
  bg?: string;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
}

export interface StyledSnapshot {
  cols: number;
  rows: number;
  lines: StyledRun[][];
}

export interface SessionAttached {
  session_id: string;
  cols: number;
  rows: number;
  cursor: CursorPos;
  snapshot: StyledSnapshot;
  replay_base64: string;
}

// session.peek response (dflow-proto SessionPeeked): a read-only, bounded, scrubbed
// plain-text screen capture. `lines` is the COUNT of lines returned; `text` is the
// visible-screen tail as plain text with secret values redacted. This is the phone's
// read-only peek - session.attach is forbidden in the phone scope.
export interface SessionPeeked {
  session_id: string;
  lines: number;
  text: string;
}

// Binary frame kinds ([u8 kind][16-byte session id][bytes...]). The phone decodes
// output frames only to drop them: the peek re-reads a fresh snapshot on a poll,
// it never streams the live PTY, and it never encodes input or resize.
export enum FrameKind {
  Output = 0,
  Input = 1,
  Resize = 2,
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}
