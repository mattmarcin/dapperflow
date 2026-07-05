import { useStore } from "../../state/store";
import { DeepLinkToken } from "../../lib/deep-links";
import { resolveMention, ResolvedMention } from "./mentions";

// The one-click invariant on the Concertmaster's mouth (product.md): a compact bar,
// above the composer, of the cards/sessions/projects the Concertmaster just mentioned.
// Each is a one-click deep link; unknown ids render muted and inert, honestly.
export function DeepLinkBar({ tokens }: { tokens: DeepLinkToken[] }) {
  const store = useStore();
  if (tokens.length === 0) return null;
  const mentions = tokens.map((t) => resolveMention(t, store));

  return (
    <div className="cm-links" role="group" aria-label="Mentioned by the Concertmaster">
      <span className="cm-links-label" aria-hidden>
        mentioned
      </span>
      <div className="cm-links-row">
        {mentions.map((m) => (
          <MentionChip key={m.token.raw} mention={m} />
        ))}
      </div>
    </div>
  );
}

function MentionChip({ mention }: { mention: ResolvedMention }) {
  const { kind, label, navigable, known, onClick } = mention;
  const title = navigable ? `Open ${kind.replace("_", " ")}` : `${kind.replace("_", " ")} - not reachable from here`;
  const className = `cm-link cm-link-${kind}${known ? "" : " is-unknown"}`;

  if (!navigable) {
    return (
      <span className={className} title={title}>
        <KindGlyph kind={kind} />
        <span className="cm-link-label">{label}</span>
      </span>
    );
  }
  return (
    <button type="button" className={className} onClick={onClick} title={title}>
      <KindGlyph kind={kind} />
      <span className="cm-link-label">{label}</span>
    </button>
  );
}

// Small kind markers for the link chips. Neutral stroke; the chip tint carries the kind.
function KindGlyph({ kind }: { kind: string }) {
  const p = (d: string) => (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden className="cm-link-glyph">
      <path d={d} stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
  switch (kind) {
    case "card":
      return p("M3 4.5h10v7H3zM3 7.5h10");
    case "session":
      return p("M3.5 4l3 3-3 3M8 10.5h4.5");
    case "project":
      return p("M2.5 3.5h4l1.2 1.6h5.8v7H2.5z");
    case "needs_you":
      return p("M8 2v1M5 6.5a3 3 0 016 0c0 3 1 4 1 4H4s1-1 1-4zM6.5 13a1.5 1.5 0 003 0");
    case "note":
      return p("M4.5 2.5h5l2.5 2.5v8.5h-7.5zM9 2.5V5h2.5M6 8h4M6 10.5h4");
    default:
      return p("M8 4.5v4.5M8 11.5h.01");
  }
}
