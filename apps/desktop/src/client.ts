// DflowClient: the webview's connection to dflowd. Owns the WebSocket, the auth
// handshake, request/response correlation, binary frame routing, and resilient
// reconnection so the status bar reflects reality.

import {
  decodeFrame,
  encodeFrame,
  Envelope,
  FrameKind,
  PROTOCOL_VERSION,
  ProtocolError,
  SessionAttached,
  SessionInfo,
} from "./protocol";
import { toHex, ulidToBytes } from "./ulid";

export type ConnectionStatus = "connecting" | "connected" | "disconnected";

interface Pending {
  resolve: (payload: unknown) => void;
  reject: (err: ProtocolError) => void;
}

export interface CreateSessionInput {
  harness: string;
  /** Configured launcher (name or id). The daemon resolves its command + extra env
   *  and records the adapter family as the session harness (protocol.md). */
  agent?: string;
  cols: number;
  rows: number;
  cwd?: string;
  env?: Record<string, string>;
}

export class DflowClient {
  private ws?: WebSocket;
  private pending = new Map<string, Pending>();
  private outputHandlers = new Map<string, (data: Uint8Array) => void>();
  private seq = 0;
  private shouldRun = true;
  private reconnectTimer?: number;

  status: ConnectionStatus = "connecting";
  daemonVersion = "";

  onStatus?: (status: ConnectionStatus) => void;
  onReconnect?: () => void;
  // Server-initiated events (event.*, no id) fan out here; the board/timeline
  // subscribe through the data layer (see data/live.ts).
  onEvent?: (env: Envelope) => void;

  constructor(private readonly port: number, private readonly token: string) {}

  /** Open the socket and complete the auth handshake. Resolves once welcomed. */
  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.setStatus("connecting");
      const ws = new WebSocket(`ws://127.0.0.1:${this.port}/ws`);
      ws.binaryType = "arraybuffer";
      this.ws = ws;
      let authed = false;

      ws.onopen = () => {
        ws.send(
          JSON.stringify({
            v: PROTOCOL_VERSION,
            id: "auth",
            type: "auth.hello",
            payload: { token: this.token, client: "desktop", proto_versions: [PROTOCOL_VERSION] },
          }),
        );
      };

      ws.onmessage = (ev) => {
        if (typeof ev.data === "string") {
          const env = JSON.parse(ev.data) as Envelope;
          if (!authed) {
            if (env.type === "auth.welcome") {
              authed = true;
              this.daemonVersion = (env.payload as { daemon_version: string }).daemon_version;
              this.setStatus("connected");
              resolve();
            } else {
              reject(new Error("authentication rejected") as unknown as ProtocolError);
              ws.close();
            }
            return;
          }
          this.handleControl(env);
        } else {
          this.handleBinary(new Uint8Array(ev.data as ArrayBuffer));
        }
      };

      ws.onerror = () => {
        if (!authed) reject(new Error("websocket error") as unknown as ProtocolError);
      };

      ws.onclose = () => {
        this.setStatus("disconnected");
        this.pending.forEach((p) =>
          p.reject({ code: "internal", message: "connection closed", retryable: true }),
        );
        this.pending.clear();
        if (this.shouldRun) this.scheduleReconnect();
      };
    });
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== undefined) return;
    this.reconnectTimer = window.setTimeout(async () => {
      this.reconnectTimer = undefined;
      if (!this.shouldRun) return;
      try {
        await this.connect();
        this.onReconnect?.();
      } catch {
        this.scheduleReconnect();
      }
    }, 1500);
  }

  private setStatus(status: ConnectionStatus): void {
    this.status = status;
    this.onStatus?.(status);
  }

  private handleControl(env: Envelope): void {
    if (!env.id) {
      // Server-initiated event (event.*). Fan out to the data layer.
      if (env.type.startsWith("event.")) this.onEvent?.(env);
      return;
    }
    const pending = this.pending.get(env.id);
    if (!pending) return;
    this.pending.delete(env.id);
    if (env.type === "error") {
      pending.reject(env.payload as ProtocolError);
    } else {
      pending.resolve(env.payload);
    }
  }

  private handleBinary(bytes: Uint8Array): void {
    const frame = decodeFrame(bytes);
    if (!frame || frame.kind !== FrameKind.Output) return;
    const handler = this.outputHandlers.get(toHex(frame.sid));
    handler?.(frame.data);
  }

  private request<T>(type: string, payload: unknown): Promise<T> {
    const id = `c${(this.seq++).toString(36)}-${Date.now().toString(36)}`;
    return new Promise<T>((resolve, reject) => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
        reject({ code: "internal", message: "not connected", retryable: true } as ProtocolError);
        return;
      }
      this.pending.set(id, { resolve: resolve as (p: unknown) => void, reject });
      this.ws.send(JSON.stringify({ v: PROTOCOL_VERSION, id, type, payload }));
    });
  }

  /**
   * Generic typed request for control verbs beyond the Phase 0 session set
   * (project.*, card.*, dispatch.*, event.*). The live data layer (data/live.ts)
   * calls through here against the protocol.md payload shapes.
   */
  call<T>(type: string, payload: unknown): Promise<T> {
    return this.request<T>(type, payload);
  }

  get connected(): boolean {
    return this.status === "connected";
  }

  // --- Session verbs -------------------------------------------------------

  createSession(input: CreateSessionInput): Promise<{ session_id: string }> {
    return this.request("session.create", {
      harness: input.harness,
      agent: input.agent,
      cols: input.cols,
      rows: input.rows,
      cwd: input.cwd,
      env: input.env ?? {},
    });
  }

  attach(sessionId: string, cols: number, rows: number): Promise<SessionAttached> {
    return this.request("session.attach", { session_id: sessionId, cols, rows });
  }

  detach(sessionId: string): Promise<{ ok: boolean }> {
    return this.request("session.detach", { session_id: sessionId });
  }

  kill(sessionId: string): Promise<{ ok: boolean }> {
    return this.request("session.kill", { session_id: sessionId });
  }

  /** Rename a session (protocol.md session.rename). Optimistic at the call site. */
  rename(sessionId: string, title: string): Promise<{ ok: boolean }> {
    return this.request("session.rename", { session_id: sessionId, title });
  }

  async list(): Promise<SessionInfo[]> {
    const res = await this.request<{ sessions: SessionInfo[] }>("session.list", {});
    return res.sessions;
  }

  /**
   * Ask the daemon to shut down gracefully (daemon.shutdown). The daemon exits before
   * it can reply, so the request usually rejects on socket close - callers treat a
   * thrown error as success. Live sessions reconcile as interrupted on next startup.
   */
  async shutdownDaemon(): Promise<void> {
    // Stop the auto-reconnect loop first: this is an intentional stop, not a drop.
    this.shouldRun = false;
    try {
      await this.request("daemon.shutdown", {});
    } finally {
      if (this.reconnectTimer !== undefined) window.clearTimeout(this.reconnectTimer);
    }
  }

  // --- PTY I/O -------------------------------------------------------------

  registerOutput(sessionId: string, handler: (data: Uint8Array) => void): void {
    this.outputHandlers.set(toHex(ulidToBytes(sessionId)), handler);
  }

  unregisterOutput(sessionId: string): void {
    this.outputHandlers.delete(toHex(ulidToBytes(sessionId)));
  }

  sendInput(sessionId: string, data: string | Uint8Array): void {
    const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
    this.sendFrame(encodeFrame(FrameKind.Input, ulidToBytes(sessionId), bytes));
  }

  sendResize(sessionId: string, cols: number, rows: number): void {
    const body = new Uint8Array(4);
    const view = new DataView(body.buffer);
    view.setUint16(0, cols); // big-endian, matching the daemon
    view.setUint16(2, rows);
    this.sendFrame(encodeFrame(FrameKind.Resize, ulidToBytes(sessionId), body));
  }

  private sendFrame(bytes: Uint8Array): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(bytes);
    }
  }

  close(): void {
    this.shouldRun = false;
    if (this.reconnectTimer !== undefined) window.clearTimeout(this.reconnectTimer);
    this.ws?.close();
  }
}
