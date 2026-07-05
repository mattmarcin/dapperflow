// Pairing bootstrap: how the phone learns which daemon to talk to and with what token.
//
// The QR flow (security.md, mobile.md 3.1): the desktop shows a QR encoding
//   http://<lan-ip>:<port>/m#pair=<base64url {url, token}>
// The OS camera opens that URL in the browser (the page cannot use the camera on a
// plain-HTTP LAN origin - mobile.md's secure-context wall), landing here with the
// pairing payload in the URL fragment. The fragment is chosen over a query string so
// the token is never sent to the server in a request line or written to a server log.
//
// Honest limitation (security.md 5.1): a browser PWA has nothing better than
// localStorage for the token; it cannot reach the OS keychain the way the M7 native
// app will. Settings states this plainly and offers Disconnect (which shreds it).

export interface PairingPayload {
  /** The LAN WebSocket endpoint, e.g. "ws://192.168.1.20:8787/ws". */
  url: string;
  /** The phone-scoped capability token minted by the desktop at QR time. */
  token: string;
  /** Optional human label for the daemon/host, shown in Settings. */
  name?: string;
  /** When the pairing was established on this device (ms epoch). */
  pairedAt?: number;
}

const STORAGE_KEY = "dapperflow.pairing";
const FRAGMENT_KEY = "pair";

/** Decode a base64 or base64url string to UTF-8 text. */
function decodeBase64(input: string): string {
  const normalized = input.replace(/-/g, "+").replace(/_/g, "/");
  const padded = normalized + "===".slice((normalized.length + 3) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

/** Encode UTF-8 text to base64url (documents the QR payload format; used in dev tools). */
function encodeBase64Url(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function isValidPayload(value: unknown): value is PairingPayload {
  if (!value || typeof value !== "object") return false;
  const p = value as Record<string, unknown>;
  return (
    typeof p.url === "string" &&
    /^wss?:\/\//i.test(p.url) &&
    typeof p.token === "string" &&
    p.token.length > 0
  );
}

/** Parse `#pair=<base64>` from a raw fragment string; null on anything malformed. */
export function parsePairingFragment(hash: string): PairingPayload | null {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  if (!raw) return null;
  const params = new URLSearchParams(raw);
  const encoded = params.get(FRAGMENT_KEY);
  if (!encoded) return null;
  try {
    const parsed = JSON.parse(decodeBase64(encoded)) as unknown;
    if (!isValidPayload(parsed)) return null;
    return { ...parsed, pairedAt: Date.now() };
  } catch {
    return null;
  }
}

/**
 * On boot: if the URL fragment carries a pairing, persist it and strip the fragment
 * from the address bar (so the token does not linger in visible history), then return
 * it. Otherwise return whatever pairing is already stored, or null for demo mode.
 */
export function bootstrapPairing(): PairingPayload | null {
  const fromUrl = parsePairingFragment(window.location.hash);
  if (fromUrl) {
    savePairing(fromUrl);
    try {
      history.replaceState(null, "", window.location.pathname + window.location.search);
    } catch {
      // Non-fatal: some embedded browsers refuse replaceState; the pairing is stored.
    }
    return fromUrl;
  }
  return loadPairing();
}

export function loadPairing(): PairingPayload | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as unknown;
    return isValidPayload(parsed) ? (parsed as PairingPayload) : null;
  } catch {
    return null;
  }
}

export function savePairing(payload: PairingPayload): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // Private-mode / storage-disabled: pairing stays in memory for this session only.
  }
}

/** Disconnect: shred the stored pairing token and any cached state keys. */
export function clearPairing(): void {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}

/** Build the pairing URL a desktop QR would encode. Documents the format; dev aid. */
export function buildPairingUrl(origin: string, payload: PairingPayload): string {
  const json = JSON.stringify({ url: payload.url, token: payload.token, name: payload.name });
  const base = origin.replace(/\/$/, "");
  return `${base}/m#${FRAGMENT_KEY}=${encodeBase64Url(json)}`;
}
