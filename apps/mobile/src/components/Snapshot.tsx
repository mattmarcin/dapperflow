import { StyledRun } from "../client/protocol";

// The read-only terminal peek surface: a styled screen snapshot rendered as monospace
// text. NOT xterm - there is no input path, no cursor keystroke handling, nothing that
// could type into the PTY. Each StyledRun becomes a colored span; empty lines keep the
// screen's vertical rhythm. Poll-refresh swaps the whole grid (the parent re-fetches).
export function Snapshot({ lines }: { lines: StyledRun[][] }) {
  return (
    <div className="snapshot" role="img" aria-label="Read-only terminal snapshot">
      {lines.map((runs, r) => (
        <div className="snapshot-line" key={r}>
          {runs.length === 0 ? (
            " "
          ) : (
            runs.map((run, i) => (
              <span key={i} style={styleFor(run)}>
                {run.text}
              </span>
            ))
          )}
        </div>
      ))}
    </div>
  );
}

function styleFor(run: StyledRun): React.CSSProperties {
  const s: React.CSSProperties = {};
  if (run.inverse) {
    s.color = run.bg ?? "var(--peek-bg)";
    s.background = run.fg ?? "var(--peek-fg)";
  } else {
    if (run.fg) s.color = run.fg;
    if (run.bg) s.background = run.bg;
  }
  if (run.bold) s.fontWeight = 700;
  if (run.italic) s.fontStyle = "italic";
  if (run.underline) s.textDecoration = "underline";
  return s;
}
