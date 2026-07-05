// One designed context-menu primitive for the whole app. The browser default menu is
// suppressed globally (App.tsx) except inside text fields; every surface that wants a
// menu opens this one, so styling, positioning, keyboard nav, Esc, and click-away are
// uniform. Data-driven: callers pass a list of items (with optional submenus, danger
// styling, disabled state and separators) and the menu renders itself at the cursor.

import {
  createContext,
  ReactNode,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

export interface MenuItem {
  id: string;
  label: string;
  onSelect?: () => void;
  danger?: boolean;
  disabled?: boolean;
  submenu?: MenuItem[];
  separatorBefore?: boolean;
  hint?: string; // right-aligned muted hint (e.g. current lane)
}

interface MenuState {
  items: MenuItem[];
  x: number;
  y: number;
}

interface ContextMenuValue {
  openMenu: (e: { clientX: number; clientY: number; preventDefault?: () => void }, items: MenuItem[]) => void;
  closeMenu: () => void;
}

const ContextMenuContext = createContext<ContextMenuValue | null>(null);

export function ContextMenuProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<MenuState | null>(null);

  const openMenu = useCallback<ContextMenuValue["openMenu"]>((e, items) => {
    e.preventDefault?.();
    if (items.length === 0) return;
    setState({ items, x: e.clientX, y: e.clientY });
  }, []);

  const closeMenu = useCallback(() => setState(null), []);

  const value = useMemo(() => ({ openMenu, closeMenu }), [openMenu, closeMenu]);

  return (
    <ContextMenuContext.Provider value={value}>
      {children}
      {state ? <ContextMenuView state={state} onClose={closeMenu} /> : null}
    </ContextMenuContext.Provider>
  );
}

export function useContextMenu(): ContextMenuValue {
  const ctx = useContext(ContextMenuContext);
  if (!ctx) throw new Error("useContextMenu must be used within ContextMenuProvider");
  return ctx;
}

function ContextMenuView({ state, onClose }: { state: MenuState; onClose: () => void }) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: state.x, y: state.y });

  // Keep the menu on-screen: flip left/up when it would overflow the viewport.
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    let x = state.x;
    let y = state.y;
    const pad = 8;
    if (x + r.width > window.innerWidth - pad) x = Math.max(pad, window.innerWidth - r.width - pad);
    if (y + r.height > window.innerHeight - pad) y = Math.max(pad, window.innerHeight - r.height - pad);
    setPos({ x, y });
  }, [state]);

  // Focus the first enabled item so arrow-key navigation works immediately.
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const first = el.querySelector<HTMLButtonElement>('button.ctx-item:not(:disabled)');
    first?.focus();
  }, [state]);

  // Esc + click-away close.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const items = state.items;

  return (
    <div className="ctx-scrim" onMouseDown={onClose} onContextMenu={(e) => { e.preventDefault(); onClose(); }}>
      <div
        ref={rootRef}
        className="ctx-menu"
        role="menu"
        style={{ left: pos.x, top: pos.y }}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => handleMenuKeys(e)}
      >
        {items.map((item) => (
          <MenuRow key={item.id} item={item} onClose={onClose} />
        ))}
      </div>
    </div>
  );
}

// Roving focus with arrow keys within a menu level.
function handleMenuKeys(e: React.KeyboardEvent<HTMLDivElement>) {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const menu = e.currentTarget;
  const buttons = Array.from(menu.querySelectorAll<HTMLButtonElement>(":scope > button.ctx-item:not(:disabled)"));
  if (buttons.length === 0) return;
  const idx = buttons.indexOf(document.activeElement as HTMLButtonElement);
  e.preventDefault();
  const next = e.key === "ArrowDown" ? (idx + 1) % buttons.length : (idx - 1 + buttons.length) % buttons.length;
  buttons[next]?.focus();
}

function MenuRow({ item, onClose }: { item: MenuItem; onClose: () => void }) {
  const [open, setOpen] = useState(false);
  const hasSub = !!item.submenu && item.submenu.length > 0;

  const activate = () => {
    if (item.disabled) return;
    if (hasSub) {
      setOpen((v) => !v);
      return;
    }
    item.onSelect?.();
    onClose();
  };

  return (
    <>
      {item.separatorBefore ? <div className="ctx-sep" role="separator" /> : null}
      <button
        className={`ctx-item${item.danger ? " is-danger" : ""}${hasSub ? " has-sub" : ""}`}
        role="menuitem"
        disabled={item.disabled}
        aria-haspopup={hasSub || undefined}
        aria-expanded={hasSub ? open : undefined}
        onClick={activate}
        onMouseEnter={() => hasSub && setOpen(true)}
        onKeyDown={(e) => {
          if (hasSub && (e.key === "ArrowRight" || e.key === "Enter")) {
            e.preventDefault();
            setOpen(true);
            // Focus into the submenu next frame.
            requestAnimationFrame(() => {
              const sub = (e.currentTarget.parentElement?.querySelector(".ctx-submenu button.ctx-item:not(:disabled)")) as HTMLButtonElement | null;
              sub?.focus();
            });
          }
          if (e.key === "ArrowLeft") setOpen(false);
        }}
      >
        <span className="ctx-item-label">{item.label}</span>
        {item.hint ? <span className="ctx-item-hint">{item.hint}</span> : null}
        {hasSub ? (
          <span className="ctx-item-caret" aria-hidden>
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
              <path d="M4 3l3 2-3 2" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </span>
        ) : null}
      </button>
      {hasSub && open ? (
        <div className="ctx-submenu" role="menu" onKeyDown={(e) => handleMenuKeys(e)}>
          {item.submenu!.map((sub) => (
            <MenuRow key={sub.id} item={sub} onClose={onClose} />
          ))}
        </div>
      ) : null}
    </>
  );
}
