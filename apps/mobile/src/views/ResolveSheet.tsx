import { useStore } from "../state/app-store";
import { Sheet } from "../components/Sheet";
import { needsYouMeta } from "../lib/needs-you";
import { NeedsYouIcon } from "../lib/glyphs";
import { capability } from "../capabilities";
import { timeAgo } from "../lib/format";

// The approval + detail surface. Two Needs You kinds land here:
//   pr_ready  - a green PR merge decision. Merge is a real phone capability, but the
//               delivery pipeline arrives with M5, so it renders as a disabled preview
//               with an honest note - never a dead button pretending to work.
//   detail    - gate findings and unknown kinds that resolve on the desktop; the phone
//               shows the context and a dismiss, never fakes an action it cannot take.
export function ResolveSheet() {
  const store = useStore();
  const overlay = store.overlay;
  if (!overlay || (overlay.kind !== "approval" && overlay.kind !== "detail")) return null;
  const item = store.itemById(overlay.itemId);
  if (!item) {
    // The item was dismissed/resolved out from under us; close cleanly.
    store.closeOverlay();
    return null;
  }

  const meta = needsYouMeta(item.kind);
  const card = store.cardById(item.card_id);
  const project = card ? store.snapshot?.projects.find((p) => p.id === card.project_id) : undefined;
  const isMerge = overlay.kind === "approval";
  const mergeCap = capability("merge");

  return (
    <Sheet title={isMerge ? "Merge decision" : "Details"} onClose={store.closeOverlay}>
      <div className="resolve">
        <div className={`resolve-head tone-${meta.tone}`}>
          <span className="resolve-glyph" aria-hidden>
            <NeedsYouIcon glyph={meta.glyph} size={20} />
          </span>
          <div>
            <div className="resolve-kind">{meta.label}</div>
            <div className="resolve-age">{timeAgo(item.raised_at)}</div>
          </div>
        </div>

        <h3 className="resolve-title">{card?.title ?? "Untitled card"}</h3>
        {project ? <div className="resolve-proj">{project.name}</div> : null}
        {item.note ? <p className="resolve-note">{item.note}</p> : null}

        {isMerge ? (
          <div className="resolve-block">
            <button className="btn-primary btn-block" disabled aria-disabled="true">
              Merge PR
            </button>
            <div className="gate-note" role="note">
              <strong>Preview.</strong> {mergeCap?.note}
            </div>
            <p className="resolve-hint">
              The PR is green. Merge it from the desktop until the delivery pipeline lands here.
            </p>
          </div>
        ) : (
          <div className="resolve-block">
            <p className="resolve-hint">
              This needs judgment. Open the card on the desktop to resolve it; the phone shows it here so
              you know it is waiting.
            </p>
          </div>
        )}

        <div className="resolve-actions">
          <button className="btn-ghost btn-block" onClick={() => void store.dismissNeedsYou(item.id).then(store.closeOverlay)}>
            Dismiss from queue
          </button>
        </div>
      </div>
    </Sheet>
  );
}
