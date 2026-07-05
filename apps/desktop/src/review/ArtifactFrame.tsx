// The sandboxed artifact host + postMessage bridge. Promoted from spike 5
// (the design notes, mechanics 1 and 2).
//
// The iframe is sandboxed exactly as security.md specs: `allow-scripts allow-forms`
// and crucially NO `allow-same-origin`, so the artifact runs in an OPAQUE origin.
// Two consequences this component handles:
//   1. `event.origin` is the string "null" and cannot be trusted, so inbound
//      messages are gated by identity (`event.source === iframe.contentWindow`)
//      and then re-validated field-by-field with parseFromSdk (the allowlist).
//   2. Outbound postMessage cannot target a real origin, so it targets "*". That
//      is safe because the sandboxed child cannot read the parent, and the child
//      only accepts messages whose source === parent.
//
// The serving URL is resolved through `signUrl` (the DataSource seam): today the
// dev artifact service mints it; once the daemon's loopback artifact endpoint
// lands, `artifact.get` mints the real short-lived capability URL and nothing here
// changes. The iframe never holds a bearer token, only the signed URL.

import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { FromSdkMessage, parseFromSdk, ToSdkMessage } from "./protocol";

export interface ArtifactFrameHandle {
  send: (msg: ToSdkMessage) => void;
  loadArtifact: (docId: string) => void;
  currentArtifact: () => string;
}

interface Props {
  initialDocId: string;
  /** Mint a short-lived signed serving URL for a doc id (DataSource.signArtifactUrl). */
  signUrl: (docId: string) => Promise<string>;
  onMessage: (msg: FromSdkMessage) => void;
  /** Called when an inbound message fails identity or schema validation. */
  onRejected: (reason: string) => void;
  /** Fired when a (re)load begins, so the host can reset its per-render state. */
  onNavigate?: (docId: string) => void;
}

export const ArtifactFrame = forwardRef<ArtifactFrameHandle, Props>(function ArtifactFrame(
  { initialDocId, signUrl, onMessage, onRejected, onNavigate },
  ref,
) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const docRef = useRef<string>(initialDocId);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const [errorText, setErrorText] = useState<string | null>(null);

  // Keep the latest callbacks without re-subscribing the window listener.
  const onMessageRef = useRef(onMessage);
  onMessageRef.current = onMessage;
  const onRejectedRef = useRef(onRejected);
  onRejectedRef.current = onRejected;
  const signRef = useRef(signUrl);
  signRef.current = signUrl;

  async function loadArtifact(docId: string) {
    setStatus("loading");
    setErrorText(null);
    docRef.current = docId;
    onNavigate?.(docId);
    try {
      // Ask the endpoint to mint a short-lived signed URL, then point the iframe at
      // it. The iframe holds a capability, never a token (security.md).
      const url = await signRef.current(docId);
      const frame = iframeRef.current;
      if (frame) frame.src = url;
    } catch (err) {
      setStatus("error");
      setErrorText(
        `Could not load the plan artifact. The artifact service is unavailable. (${String(err)})`,
      );
    }
  }

  useEffect(() => {
    const onWinMessage = (e: MessageEvent) => {
      const frame = iframeRef.current;
      // Identity gate: opaque origin means we cannot check e.origin, so we require
      // the message to come from THIS iframe's live contentWindow.
      if (!frame || e.source !== frame.contentWindow) return;
      const msg = parseFromSdk(e.data);
      if (!msg) {
        onRejectedRef.current(`dropped malformed message: ${briefly(e.data)}`);
        return;
      }
      if (msg.type === "ready") setStatus("ready");
      onMessageRef.current(msg);
    };
    window.addEventListener("message", onWinMessage);
    return () => window.removeEventListener("message", onWinMessage);
  }, []);

  useImperativeHandle(ref, () => ({
    send: (msg: ToSdkMessage) => {
      const win = iframeRef.current?.contentWindow;
      if (win) win.postMessage(msg, "*"); // opaque child: "*" is the only option
    },
    loadArtifact,
    currentArtifact: () => docRef.current,
  }));

  // Initial load once the iframe element exists.
  useEffect(() => {
    void loadArtifact(initialDocId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="pf-frame-wrap">
      {status === "loading" ? <div className="pf-frame-status">Loading plan…</div> : null}
      {status === "error" ? <div className="pf-frame-status is-error">{errorText}</div> : null}
      <iframe
        ref={iframeRef}
        className="pf-frame"
        title="Plan artifact"
        // The specced posture. No allow-same-origin: opaque origin, no parent DOM
        // access, no token theft. allow-forms so native controls submit; the SDK
        // still routes everything through postMessage.
        sandbox="allow-scripts allow-forms"
      />
    </div>
  );
});

function briefly(x: unknown): string {
  try {
    const s = typeof x === "string" ? x : JSON.stringify(x);
    return s.length > 80 ? `${s.slice(0, 80)}…` : s;
  } catch {
    return String(x);
  }
}
