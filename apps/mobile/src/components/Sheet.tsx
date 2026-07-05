import { ReactNode, useEffect } from "react";

// A full-screen overlay sheet: the phone's "pushed view" for a peek, a plan review, or
// an approval. Slides up over the tab surface; a back affordance and Escape both close
// it, and the body scrolls independently so the header stays put.
export function Sheet({
  title,
  onClose,
  accessory,
  children,
}: {
  title: string;
  onClose: () => void;
  accessory?: ReactNode;
  children: ReactNode;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="sheet" role="dialog" aria-modal="true" aria-label={title}>
      <header className="sheet-head">
        <button className="sheet-back" onClick={onClose} aria-label="Back">
          <svg width="18" height="18" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="M9.5 3.5L5 8l4.5 4.5" />
          </svg>
        </button>
        <h2 className="sheet-title">{title}</h2>
        <div className="sheet-accessory">{accessory}</div>
      </header>
      <div className="sheet-body">{children}</div>
    </div>
  );
}
