// TypeScript mirror of the Phase 0 subset of dflow-proto. Control messages use the
// JSON envelope; PTY I/O uses binary frames.

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

export interface SessionInfo {
  session_id: string;
  harness: string;
  cols: number;
  rows: number;
  alive: boolean;
  attached: number;
  created_at_ms: number;
}

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

// Binary frame kinds ([u8 kind][16-byte session id][bytes...]).
export enum FrameKind {
  Output = 0,
  Input = 1,
  Resize = 2,
}

export interface Frame {
  kind: FrameKind;
  sid: Uint8Array; // 16 bytes
  data: Uint8Array;
}

const SID_LEN = 16;

export function encodeFrame(kind: FrameKind, sid: Uint8Array, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(1 + SID_LEN + data.length);
  out[0] = kind;
  out.set(sid, 1);
  out.set(data, 1 + SID_LEN);
  return out;
}

export function decodeFrame(bytes: Uint8Array): Frame | null {
  if (bytes.length < 1 + SID_LEN) return null;
  return {
    kind: bytes[0] as FrameKind,
    sid: bytes.slice(1, 1 + SID_LEN),
    data: bytes.slice(1 + SID_LEN),
  };
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}
