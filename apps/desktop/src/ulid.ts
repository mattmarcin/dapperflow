// ULID <-> 16-byte conversion. Session ids are ULIDs; binary PTY frames carry the
// session id as its 16-byte big-endian form (see dflow-proto / frame.rs). BigInt
// keeps the 128-bit conversion exact where 32-bit bitwise ops would overflow.

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** Decode a 26-character ULID string to its 16 big-endian bytes. */
export function ulidToBytes(ulid: string): Uint8Array {
  const upper = ulid.toUpperCase();
  let value = 0n;
  for (const ch of upper) {
    const idx = CROCKFORD.indexOf(ch);
    if (idx < 0) throw new Error(`invalid ULID character: ${ch}`);
    value = value * 32n + BigInt(idx);
  }
  const out = new Uint8Array(16);
  for (let i = 15; i >= 0; i--) {
    out[i] = Number(value & 0xffn);
    value >>= 8n;
  }
  return out;
}

/** Lowercase hex of a byte array, used as a stable key for frame routing. */
export function toHex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

/**
 * Generate a 26-character Crockford ULID (48-bit time + 80-bit randomness).
 * Used for optimistic client ids and fixture entities so they sort like real
 * daemon-minted ULIDs and work as event-stream cursors.
 */
export function generateUlid(timeMs: number = Date.now()): string {
  let time = BigInt(timeMs);
  const timeChars: string[] = [];
  for (let i = 0; i < 10; i++) {
    timeChars.unshift(CROCKFORD[Number(time % 32n)]);
    time /= 32n;
  }
  let rand = "";
  for (let i = 0; i < 16; i++) {
    rand += CROCKFORD[Math.floor(Math.random() * 32)];
  }
  return timeChars.join("") + rand;
}
