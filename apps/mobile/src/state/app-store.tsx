import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  ReactNode,
} from "react";
import { bootstrapPairing, clearPairing, PairingPayload } from "../pairing";
import { DflowMobileClient } from "../client/client";
import { Card, FleetSnapshot, NeedsYouItem, Session } from "../client/model";
import { ActionResult, MobileDataSource } from "../data/source";
import { FixtureMobileSource } from "../data/fixtures";
import { LiveMobileSource } from "../data/live";
import { needsYouMeta } from "../lib/needs-you";

export type Tab = "needs" | "fleet" | "settings";
export type ConnState = "demo" | "connecting" | "connected" | "disconnected" | "error";

export type Overlay =
  | { kind: "peek"; sessionId: string }
  | { kind: "plan"; cardId: string }
  | { kind: "approval"; itemId: string }
  | { kind: "detail"; itemId: string }
  | null;

export interface AppStore {
  mode: "fixture" | "live";
  isDemo: boolean;
  conn: ConnState;
  pairing: PairingPayload | null;
  daemonVersion: string;
  grantedScope: string;

  snapshot: FleetSnapshot | null;
  loading: boolean;
  error: string | null;

  tab: Tab;
  setTab: (t: Tab) => void;

  overlay: Overlay;
  openPeek: (sessionId: string) => void;
  openResolve: (item: NeedsYouItem) => void;
  closeOverlay: () => void;

  refresh: () => Promise<void>;
  source: MobileDataSource;
  approvePlan: (planId: string, feedback: string) => Promise<ActionResult>;
  dismissNeedsYou: (itemId: string) => Promise<ActionResult>;
  disconnect: () => void;

  toast: string | null;
  flash: (msg: string) => void;

  cardById: (id: string | null) => Card | undefined;
  sessionForCard: (cardId: string) => Session | undefined;
  itemById: (id: string) => NeedsYouItem | undefined;

  demoEmpty: boolean;
  toggleDemoEmpty: () => void;
}

const Ctx = createContext<AppStore | null>(null);

export function useStore(): AppStore {
  const s = useContext(Ctx);
  if (!s) throw new Error("useStore outside provider");
  return s;
}

export function AppStoreProvider({ children }: { children: ReactNode }) {
  // Pairing + source are established once on mount.
  const pairingRef = useRef<PairingPayload | null>(null);
  const clientRef = useRef<DflowMobileClient | null>(null);
  const sourceRef = useRef<MobileDataSource | null>(null);
  if (sourceRef.current === null) {
    const pairing = bootstrapPairing();
    pairingRef.current = pairing;
    if (pairing) {
      const client = new DflowMobileClient(pairing.url, pairing.token);
      clientRef.current = client;
      sourceRef.current = new LiveMobileSource(client);
    } else {
      sourceRef.current = new FixtureMobileSource();
    }
  }
  const source = sourceRef.current;
  const isDemo = source.mode === "fixture";

  const [conn, setConn] = useState<ConnState>(isDemo ? "demo" : "connecting");
  const [daemonVersion, setDaemonVersion] = useState("");
  const [grantedScope, setGrantedScope] = useState("");
  const [snapshot, setSnapshot] = useState<FleetSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("needs");
  const [overlay, setOverlay] = useState<Overlay>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [demoEmpty, setDemoEmpty] = useState(false);

  const flash = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((cur) => (cur === msg ? null : cur)), 2600);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const snap = await source.loadFleet();
      setSnapshot(snap);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [source]);

  // Connect (live) or go straight to fixtures (demo), then load and subscribe.
  useEffect(() => {
    let active = true;
    const client = clientRef.current;

    if (client) {
      client.onStatus = (s) => {
        if (!active) return;
        setConn(s === "connected" ? "connected" : s === "connecting" ? "connecting" : "disconnected");
        setDaemonVersion(client.daemonVersion);
        setGrantedScope(client.grantedScope);
      };
      client.onReconnect = () => {
        void refresh();
      };
      client
        .connect()
        .then(() => {
          if (!active) return;
          setDaemonVersion(client.daemonVersion);
          setGrantedScope(client.grantedScope);
          void refresh();
        })
        .catch((e) => {
          if (!active) return;
          setConn("disconnected");
          setError(e instanceof Error ? e.message : String(e));
          setLoading(false);
        });
    } else {
      void refresh();
    }

    const unsub = source.subscribeEvents(() => {
      void refresh();
    });

    return () => {
      active = false;
      unsub();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const openPeek = useCallback((sessionId: string) => setOverlay({ kind: "peek", sessionId }), []);
  const closeOverlay = useCallback(() => setOverlay(null), []);

  const sessionForCard = useCallback(
    (cardId: string) => snapshot?.sessions.find((s) => s.card_id === cardId),
    [snapshot],
  );
  const cardById = useCallback(
    (id: string | null) => (id ? snapshot?.cards.find((c) => c.id === id) : undefined),
    [snapshot],
  );
  const itemById = useCallback(
    (id: string) => snapshot?.needsYou.find((n) => n.id === id),
    [snapshot],
  );

  const openResolve = useCallback(
    (item: NeedsYouItem) => {
      const meta = needsYouMeta(item.kind);
      switch (meta.surface) {
        case "peek": {
          const s = snapshot?.sessions.find((x) => x.card_id === item.card_id);
          if (s) setOverlay({ kind: "peek", sessionId: s.id });
          else setOverlay({ kind: "detail", itemId: item.id });
          break;
        }
        case "plan":
          setOverlay({ kind: "plan", cardId: item.card_id });
          break;
        case "approval":
          setOverlay({ kind: "approval", itemId: item.id });
          break;
        default:
          setOverlay({ kind: "detail", itemId: item.id });
      }
    },
    [snapshot],
  );

  const approvePlan = useCallback(
    async (planId: string, feedback: string) => {
      const res = await source.approvePlan(planId, feedback);
      if (res.ok) {
        flash("Plan approved");
        await refresh();
      } else {
        flash(res.error ?? "Approve failed");
      }
      return res;
    },
    [source, flash, refresh],
  );

  const dismissNeedsYou = useCallback(
    async (itemId: string) => {
      // Optimistic removal so the queue feels instant; the source reconciles.
      setSnapshot((snap) => (snap ? { ...snap, needsYou: snap.needsYou.filter((n) => n.id !== itemId) } : snap));
      const res = await source.dismissNeedsYou(itemId);
      if (!res.ok) {
        flash(res.error ?? "Could not dismiss");
        await refresh();
      }
      return res;
    },
    [source, flash, refresh],
  );

  const disconnect = useCallback(() => {
    clearPairing();
    clientRef.current?.close();
    // Re-bootstrap cleanly into demo mode.
    window.location.reload();
  }, []);

  const toggleDemoEmpty = useCallback(() => {
    if (!source.demo) return;
    const next = !source.demo.isNeedsYouEmpty();
    source.demo.setNeedsYouEmpty(next);
    setDemoEmpty(next);
  }, [source]);

  const value: AppStore = useMemo(
    () => ({
      mode: source.mode,
      isDemo,
      conn,
      pairing: pairingRef.current,
      daemonVersion,
      grantedScope,
      snapshot,
      loading,
      error,
      tab,
      setTab,
      overlay,
      openPeek,
      openResolve,
      closeOverlay,
      refresh,
      source,
      approvePlan,
      dismissNeedsYou,
      disconnect,
      toast,
      flash,
      cardById,
      sessionForCard,
      itemById,
      demoEmpty,
      toggleDemoEmpty,
    }),
    [
      source,
      isDemo,
      conn,
      daemonVersion,
      grantedScope,
      snapshot,
      loading,
      error,
      tab,
      overlay,
      openPeek,
      openResolve,
      closeOverlay,
      refresh,
      approvePlan,
      dismissNeedsYou,
      disconnect,
      toast,
      flash,
      cardById,
      sessionForCard,
      itemById,
      demoEmpty,
      toggleDemoEmpty,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
