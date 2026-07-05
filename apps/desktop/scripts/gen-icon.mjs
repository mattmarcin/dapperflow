// Generate the DapperFlow source icon with no image dependencies.
// A cool-ink rounded tile with three brass "channel" bars (the app's session-strip
// signature) and one mint live-dot. Rendered at 2x and box-downsampled for smooth
// edges, then written as a PNG. `tauri icon` derives every platform size from it.
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const OUT = 1024;
const SS = 2; // supersample factor
const W = OUT * SS;
const H = OUT * SS;

const INK = [15, 18, 23];
const BRASS_BRIGHT = [245, 188, 94];
const BRASS = [230, 162, 60];
const BRASS_DEEP = [176, 124, 52];
const MINT = [123, 208, 168];

function roundedRectCoverage(px, py, x, y, w, h, r) {
  if (px < x || px > x + w || py < y || py > y + h) return false;
  const rx = Math.min(r, w / 2);
  const cxs = [x + rx, x + w - rx];
  const cys = [y + rx, y + h - rx];
  // Only the four corner squares need the radius test.
  const inCornerX = px < cxs[0] ? 0 : px > cxs[1] ? 1 : -1;
  const inCornerY = py < cys[0] ? 0 : py > cys[1] ? 1 : -1;
  if (inCornerX >= 0 && inCornerY >= 0) {
    const dx = px - cxs[inCornerX];
    const dy = py - cys[inCornerY];
    return dx * dx + dy * dy <= rx * rx;
  }
  return true;
}

function circle(px, py, cx, cy, r) {
  const dx = px - cx;
  const dy = py - cy;
  return dx * dx + dy * dy <= r * r;
}

// Render the supersampled RGBA buffer.
const buf = new Uint8Array(W * H * 4);
const tileR = 180 * SS;
const glowCx = 0.30 * W;
const glowCy = 0.24 * H;
const glowR = 0.9 * W;

// Three channel bars.
const barH = 92 * SS;
const barX = 250 * SS;
const bars = [
  { y: 348 * SS, w: 540 * SS, c: BRASS_BRIGHT },
  { y: 476 * SS, w: 430 * SS, c: BRASS },
  { y: 604 * SS, w: 320 * SS, c: BRASS_DEEP },
];
const liveR = 30 * SS;
const liveCx = bars[0].y ? 250 * SS + 540 * SS + 70 * SS : 0;
const liveCy = 348 * SS + barH / 2;

for (let y = 0; y < H; y++) {
  for (let x = 0; x < W; x++) {
    const i = (y * W + x) * 4;
    // Outside the rounded tile -> transparent.
    if (!roundedRectCoverage(x, y, 0, 0, W, H, tileR)) {
      buf[i + 3] = 0;
      continue;
    }
    // Ink base with a soft brass stand-lamp glow toward the top-left.
    const dx = x - glowCx;
    const dy = y - glowCy;
    const dist = Math.sqrt(dx * dx + dy * dy);
    const g = Math.max(0, 1 - dist / glowR) ** 2 * 0.10;
    let r = INK[0] + (BRASS[0] - INK[0]) * g;
    let gg = INK[1] + (BRASS[1] - INK[1]) * g;
    let b = INK[2] + (BRASS[2] - INK[2]) * g;

    // Channel bars on top.
    for (const bar of bars) {
      if (roundedRectCoverage(x, y, barX, bar.y, bar.w, barH, barH / 2)) {
        r = bar.c[0];
        gg = bar.c[1];
        b = bar.c[2];
      }
    }
    // Live dot at the end of the top channel.
    if (circle(x, y, liveCx, liveCy, liveR)) {
      r = MINT[0];
      gg = MINT[1];
      b = MINT[2];
    }

    buf[i] = Math.round(r);
    buf[i + 1] = Math.round(gg);
    buf[i + 2] = Math.round(b);
    buf[i + 3] = 255;
  }
}

// Box-downsample SSxSS -> 1x for antialiasing.
const out = new Uint8Array(OUT * OUT * 4);
for (let y = 0; y < OUT; y++) {
  for (let x = 0; x < OUT; x++) {
    let r = 0, g = 0, b = 0, a = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const si = ((y * SS + sy) * W + (x * SS + sx)) * 4;
        r += buf[si];
        g += buf[si + 1];
        b += buf[si + 2];
        a += buf[si + 3];
      }
    }
    const n = SS * SS;
    const oi = (y * OUT + x) * 4;
    out[oi] = Math.round(r / n);
    out[oi + 1] = Math.round(g / n);
    out[oi + 2] = Math.round(b / n);
    out[oi + 3] = Math.round(a / n);
  }
}

// Encode PNG (RGBA, 8-bit, filter 0 per scanline).
function crc32(bytes) {
  let c = ~0;
  for (let i = 0; i < bytes.length; i++) {
    c ^= bytes[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}
function chunk(type, data) {
  const typeBytes = Buffer.from(type, "ascii");
  const body = Buffer.concat([typeBytes, Buffer.from(data)]);
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(OUT, 0);
ihdr.writeUInt32BE(OUT, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
ihdr[10] = 0;
ihdr[11] = 0;
ihdr[12] = 0;

const raw = Buffer.alloc(OUT * (OUT * 4 + 1));
for (let y = 0; y < OUT; y++) {
  raw[y * (OUT * 4 + 1)] = 0; // filter: none
  out.subarray(y * OUT * 4, (y + 1) * OUT * 4).forEach((v, i) => {
    raw[y * (OUT * 4 + 1) + 1 + i] = v;
  });
}

const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const here = dirname(fileURLToPath(import.meta.url));
const iconsDir = join(here, "..", "src-tauri", "icons");
mkdirSync(iconsDir, { recursive: true });
const dest = join(iconsDir, "source.png");
writeFileSync(dest, png);
console.log(`wrote ${dest} (${OUT}x${OUT}, ${png.length} bytes)`);
