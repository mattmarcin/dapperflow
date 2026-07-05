import { useMemo, useState } from "react";
import { useStore } from "../../state/store";
import { ConcertmasterSession as CmSession } from "../../model";
import { TerminalSlot } from "../../state/terminal-pool";
import { buildDemoTranscript, scopeSteerLine } from "../../lib/concertmaster";
import { useScrapedTokens } from "./mentions";
import { DeepLinkBar } from "./DeepLinkBar";

// The live Concertmaster: a real harness terminal (hosted by the shared pool), the
// deep-link bar above the composer, and a composer that types into the PTY. It is a
// real terminal, never a scraped chat facsimile (product.md principle 2); the composer
// is a convenience mouth, not a mediator.
export function ConcertmasterSession({ cm }: { cm: CmSession }) {
  const store = useStore();
  const client = store.client;
  const daemonReady = store.daemon === "connected";

  // Demo transcript is built from the live store entities so its [kind:ULID] tokens
  // resolve to real cards/sessions/projects and the link bar is fully clickable.
  const demoText = useMemo(
    () =>
      cm.demo
        ? buildDemoTranscript({
            cards: store.cards,
            sessions: store.sessions,
            projects: store.projects,
            needsYou: store.needsYou,
            launcherName: cm.agentName,
          })
        : "",
    [cm.demo, cm.agentName, store.cards, store.sessions, store.projects, store.needsYou],
  );

  const tokens = useScrapedTokens(cm, demoText);

  // First-prompt scope steer: if the Concertmaster was summoned with a focus project,
  // preload the scope line into the composer once (no auto-submit - verified submit is a
  // later daemon verb; the user reviews and sends).
  const initialInput = useMemo(() => {
    if (cm.demo || !cm.scopeProjectId) return undefined;
    const project = store.projects.find((p) => p.id === cm.scopeProjectId);
    return project ? scopeSteerLine(project) : undefined;
    // Only meaningful at first mount; scopeProjectId is fixed at creation for this id.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cm.sessionId]);

  return (
    <div className="cm-session">
      <div className="cm-stage">
        {cm.demo ? (
          <DemoTranscript text={demoText} />
        ) : client && daemonReady ? (
          <TerminalSlot
            sessionId={cm.sessionId}
            client={client}
            initialInput={initialInput}
            onKill={() => store.endConcertmaster()}
          />
        ) : (
          <DisconnectedNote onRestart={() => store.restartConcertmaster()} />
        )}
      </div>

      <DeepLinkBar tokens={tokens} />

      <Composer cm={cm} />
    </div>
  );
}

function Composer({ cm }: { cm: CmSession }) {
  const store = useStore();
  const [draft, setDraft] = useState("");
  const client = store.client;
  const live = !cm.demo && !!client && store.daemon === "connected";

  const send = () => {
    const text = draft.replace(/\s+$/, "");
    if (!text || !live || !client) return;
    // Type into the real PTY, then submit. Best-effort submit (a trailing CR); reliable
    // steering is verified submit, a later daemon verb (the design notes).
    client.sendInput(cm.sessionId, text);
    client.sendInput(cm.sessionId, "\r");
    setDraft("");
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="cm-composer">
      <textarea
        className="cm-composer-input"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={onKeyDown}
        rows={2}
        placeholder={
          cm.demo
            ? "Demo transcript - summon a live Concertmaster to chat."
            : live
              ? "Ask the Concertmaster… (Enter sends, Shift+Enter for a newline)"
              : "Reconnect the daemon to talk to the Concertmaster."
        }
        disabled={!live}
        aria-label="Message the Concertmaster"
      />
      <button className="cm-composer-send" onClick={send} disabled={!live || !draft.trim()} aria-label="Send" title="Send (Enter)">
        <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path d="M2.5 8h9M8 4.5L11.5 8 8 11.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
    </div>
  );
}

// The demo transcript, rendered read-only in a terminal-styled surface. Deep links are
// scraped from this same text so the bar is clickable without a live PTY.
function DemoTranscript({ text }: { text: string }) {
  return (
    <div className="cm-demo" role="img" aria-label="Concertmaster demo transcript">
      {text.split("\n").map((line, i) => {
        const cls = line.startsWith(">")
          ? "cm-demo-line is-prompt"
          : line.startsWith("●")
            ? "cm-demo-line is-tool"
            : line.startsWith("~")
              ? "cm-demo-line is-meta"
              : "cm-demo-line";
        return (
          <div key={i} className={cls}>
            {line || " "}
          </div>
        );
      })}
    </div>
  );
}

function DisconnectedNote({ onRestart }: { onRestart: () => void }) {
  return (
    <div className="cm-disconnected">
      <h3 className="cm-disconnected-title">The daemon went away</h3>
      <p className="cm-disconnected-sub">
        The Concertmaster lives in dflowd. Reconnect the daemon to attach its terminal, or
        restart the session.
      </p>
      <button className="btn-ghost btn-sm" onClick={onRestart}>
        Restart session
      </button>
    </div>
  );
}
