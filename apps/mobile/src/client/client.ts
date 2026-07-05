// ============================================================================
// M6 DEBT - thin duplicated transport.
// A phone-scoped copy of apps/desktop/src/client.ts, reduced to the read-only-plus-
// approve surface of the phone capability profile (security.md). Differences from
// the desktop client, all deliberate:
//   - connects to an ARBITRARY ws:// URL from the pairing payload, not a hardcoded
//     loopback port (the LAN listener lives at whatever host:port the QR encoded);
//   - authenticates as `client: mobile`, so the daemon grants the phone scope;
//   - sends NO PTY input or resize frames, and registers no output handler - binary
//     frames are decoded only to be dropped. The terminal peek re-reads a fresh
//     styled snapshot on a poll instead of streaming the live PTY.
// M7 (mobile.md 2.2) replaces this with the shared packages/client-core.
// ============================================================================

import { AuthWelcome, Envelope, PROTOCOL_VERSION, ProtocolError, SessionAttached } from "./protocol";

export type ConnectionStatus = "connecting" | "connected" | "disconnected";

interface Pending {
  resolve: (payload: unknown) => void;
  reject: (err: ProtocolError) => void;
}

export class DflowMobileClient {
  private ws?: WebSocket;
  private pending = new Map<string, Pending>();
  private seq = 0;
  private shouldRun = true;
  private reconnectTimer?: ReturnType<typeof setTimeout>;

  status: ConnectionStatus = "connecting";
  daemonVersion = "";
  grantedScope = "";

  onStatus?: (status: ConnectionStatus) => void;
  onReconnect?: () => void;
  // Server-initiated events (event.*, no id) fan out here; the data layer subscribes.
  onEvent?: (env: Envelope) => void;

  /**
   * @param url   the LAN WebSocket endpoint from the pairing payload, e.g.
   *              `ws://192.168.1.20:8787/ws`.
   * @param token the phone-scoped capability token from the pairing payload.
   */
  constructor(private readonly url: string, private readonly token: string) {}

  /** Open the socket and complete the `auth.hello { client: mobile }` handshake. */
  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.setStatus("connecting");
      let ws: WebSocket;
      try {
        ws = new WebSocket(this.url);
      } catch (e) {
        reject(asError(e));
        return;
      }
      ws.binaryType = "arraybuffer";
      this.ws = ws;
      let authed = false;

      ws.onopen = () => {
        ws.send(
          JSON.stringify({
            v: PROTOCOL_VERSION,
            id: "auth",
            type: "auth.hello",
            payload: { token: this.token, client: "mobile", proto_versions: [PROTOCOL_VERSION] },
          }),
        );
      };

      ws.onmessage = (ev) => {
        if (typeof ev.data === "string") {
          let env: Envelope;
          try {
            env = JSON.parse(ev.data) as Envelope;
          } catch {
            return;
          }
          if (!authed) {
            if (env.type === "auth.welcome") {
              authed = true;
              const w = env.payload as AuthWelcome;
              this.daemonVersion = w.daemon_version ?? "";
              this.grantedScope = w.scope ?? "";
              this.setStatus("connected");
              resolve();
            } else {
              reject({ code: "auth", message: "authentication rejected", retryable: false });
              ws.close();
            }
            return;
          }
          this.handleControl(env);
        }
        // Binary frames (live PTY output) are dropped: the phone is read-only and
        // polls fresh snapshots instead of streaming.
      };

      ws.onerror = () => {
        if (!authed) reject({ code: "network", message: "websocket error", retryable: true });
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
    this.reconnectTimer = setTimeout(async () => {
      this.reconnectTimer = undefined;
      if (!this.shouldRun) return;
      try {
        await this.connect();
        this.onReconnect?.();
      } catch {
        this.scheduleReconnect();
      }
    }, 1800);
  }

  private setStatus(status: ConnectionStatus): void {
    this.status = status;
    this.onStatus?.(status);
  }

  private handleControl(env: Envelope): void {
    if (!env.id) {
      if (env.type.startsWith("event.")) this.onEvent?.(env);
      return;
    }
    const pending = this.pending.get(env.id);
    if (!pending) return;
    this.pending.delete(env.id);
    if (env.type === "error") pending.reject(env.payload as ProtocolError);
    else pending.resolve(env.payload);
  }

  /** Generic typed request for the read/approve verbs the phone profile allows. */
  call<T>(type: string, payload: unknown): Promise<T> {
    const id = `m${(this.seq++).toString(36)}-${Date.now().toString(36)}`;
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
   * session.attach { session_id, cols, rows } -> the styled screen snapshot for the
   * read-only peek. The phone reads the snapshot, then immediately detaches: it never
   * holds a live PTY attachment, never registers an output handler, never types.
   */
  async peek(sessionId: string, cols = 80, rows = 24): Promise<SessionAttached> {
    const attached = await this.call<SessionAttached>("session.attach", {
      session_id: sessionId,
      cols,
      rows,
    });
    // Release the attachment right away - read-only, no streaming (protocol.md
    // backpressure: a slow client gets a fresh snapshot on catch-up, which is exactly
    // the poll model here).
    this.call("session.detach", { session_id: sessionId }).catch(() => undefined);
    return attached;
  }

  get connected(): boolean {
    return this.status === "connected";
  }

  close(): void {
    this.shouldRun = false;
    if (this.reconnectTimer !== undefined) clearTimeout(this.reconnectTimer);
    this.ws?.close();
  }
}

function asError(e: unknown): ProtocolError {
  const message = e instanceof Error ? e.message : String(e);
  return { code: "network", message, retryable: true };
}
