// Generates the PWA PNG icons from the DapperFlow brand mark (brass flow bars + mint
// dot on ink), with zero dependencies: a tiny hand-rolled PNG encoder over Node's
// built-in zlib. Run with `pnpm icons`. Deterministic output, so re-running only
// rewrites identical bytes. Keeping the icons generated (not binary-committed by hand)
// documents exactly how the identity is drawn.

import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "public");
mkdirSync(OUT, { recursive: true });

const INK = [14, 17, 22];
const BARS = [
  { x: 4, y: 6.5, w: 18, h: 3.4, r: 1.7, c: [245, 188, 94] }, // brass-bright
  { x: 4, y: 11.3, w: 14, h: 3.4, r: 1.7, c: [230, 162, 60] }, // brass
  { x: 4, y: 16.1, w: 10, h: 3.4, r: 1.7, c: [176, 124, 52] }, // brass-deep
];
const DOT = { cx: 23.2, cy: 8.2, r: 1.9, c: [123, 208, 168] }; // mint
const DESIGN = 26; // the brand mark's coordinate box

// --- draw -------------------------------------------------------------------

function render(size, contentFrac) {
  const buf = Buffer.alloc(size * size * 4);
  // fill ink
  for (let i = 0; i < size * size; i++) {
    buf[i * 4] = INK[0];
    buf[i * 4 + 1] = INK[1];
    buf[i * 4 + 2] = INK[2];
    buf[i * 4 + 3] = 255;
  }
  const side = size * contentFrac;
  const offset = (size - side) / 2;
  const scale = side / DESIGN;
  const toPx = (u) => offset + u * scale;

  const set = (x, y, c) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    const i = (y * size + x) * 4;
    buf[i] = c[0];
    buf[i + 1] = c[1];
    buf[i + 2] = c[2];
    buf[i + 3] = 255;
  };

  const fillRoundRect = (rx, ry, rw, rh, rr, c) => {
    const x0 = toPx(rx), y0 = toPx(ry), x1 = toPx(rx + rw), y1 = toPx(ry + rh);
    const rad = rr * scale;
    for (let y = Math.floor(y0); y < Math.ceil(y1); y++) {
      for (let x = Math.floor(x0); x < Math.ceil(x1); x++) {
        if (inRoundRect(x + 0.5, y + 0.5, x0, y0, x1, y1, rad)) set(x, y, c);
      }
    }
  };

  const fillCircle = (cx, cy, r, c) => {
    const px = toPx(cx), py = toPx(cy), pr = r * scale;
    for (let y = Math.floor(py - pr); y <= Math.ceil(py + pr); y++) {
      for (let x = Math.floor(px - pr); x <= Math.ceil(px + pr); x++) {
        const dx = x + 0.5 - px, dy = y + 0.5 - py;
        if (dx * dx + dy * dy <= pr * pr) set(x, y, c);
      }
    }
  };

  for (const b of BARS) fillRoundRect(b.x, b.y, b.w, b.h, b.r, b.c);
  fillCircle(DOT.cx, DOT.cy, DOT.r, DOT.c);
  return buf;
}

function inRoundRect(x, y, x0, y0, x1, y1, rad) {
  if (x < x0 || x > x1 || y < y0 || y > y1) return false;
  // corner tests
  const cx = x < x0 + rad ? x0 + rad : x > x1 - rad ? x1 - rad : x;
  const cy = y < y0 + rad ? y0 + rad : y > y1 - rad ? y1 - rad : y;
  const dx = x - cx, dy = y - cy;
  return dx * dx + dy * dy <= rad * rad + 0.0001 || (x >= x0 + rad && x <= x1 - rad) || (y >= y0 + rad && y <= y1 - rad);
}

// --- PNG encode -------------------------------------------------------------

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const body = Buffer.concat([typeBuf, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}

function encodePng(rgba, size) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;
  // filtered scanlines (filter byte 0 per row)
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const idat = deflateSync(raw, { level: 9 });
  return Buffer.concat([sig, chunk("IHDR", ihdr), chunk("IDAT", idat), chunk("IEND", Buffer.alloc(0))]);
}

// --- outputs ----------------------------------------------------------------

const targets = [
  { file: "icon-192.png", size: 192, frac: 0.72 },
  { file: "icon-512.png", size: 512, frac: 0.72 },
  { file: "icon-maskable-512.png", size: 512, frac: 0.6 }, // extra padding for the mask safe zone
  { file: "apple-touch-icon.png", size: 180, frac: 0.66 },
];

for (const t of targets) {
  const rgba = render(t.size, t.frac);
  const png = encodePng(rgba, t.size);
  writeFileSync(join(OUT, t.file), png);
  console.log(`wrote ${t.file} (${t.size}x${t.size}, ${png.length} bytes)`);
}
