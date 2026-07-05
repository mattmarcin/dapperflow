import { ReactNode, useEffect, useRef } from "react";

interface Props {
  title: string;
  subtitle?: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
}

// A scrim-backed dialog. Esc closes, focus lands inside on open, click outside
// dismisses. The one modal shell for card creation, confirmation, and add-project.
export function Modal({ title, subtitle, onClose, children, footer, width = 480 }: Props) {
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    const first = panelRef.current?.querySelector<HTMLElement>(
      "input, textarea, select, button:not(.modal-x)",
    );
    first?.focus();
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="scrim" onMouseDown={onClose}>
      <div
        className="modal"
        style={{ width }}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        ref={panelRef}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-head">
          <div>
            <h2 className="modal-title">{title}</h2>
            {subtitle ? <p className="modal-sub">{subtitle}</p> : null}
          </div>
          <button className="modal-x" onClick={onClose} aria-label="Close">
            <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden>
              <path d="M2 2l10 10M12 2L2 12" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
        </header>
        <div className="modal-body">{children}</div>
        {footer ? <footer className="modal-foot">{footer}</footer> : null}
      </div>
    </div>
  );
}
