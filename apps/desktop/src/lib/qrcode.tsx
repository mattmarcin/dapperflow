// A self-contained QR renderer for the M6 pairing screen. Uses the bundled
// `qrcode-generator` (MIT, zero-dependency, no CDN - Vite inlines it into the app),
// and draws the modules as one crisp SVG path so it scales without blurring.
//
// QR codes must be dark modules on a light quiet zone to scan reliably, so this always
// paints near-black modules on a light plate regardless of the app's dark theme; the
// caller frames it with the app's panel chrome.

import { useMemo } from "react";
import qrcode from "qrcode-generator";

interface Props {
  /** The string to encode (the pairing payload URL). */
  value: string;
  /** Rendered pixel size of the square (default 208). */
  size?: number;
  /** Quiet-zone width in modules (QR spec minimum is 4). */
  margin?: number;
  className?: string;
  /** Accessible label; the payload is not read aloud. */
  title?: string;
}

// Build an SVG path covering every dark module as a 1x1 rect in module units.
function modulesToPath(qr: ReturnType<typeof qrcode>): { d: string; count: number } {
  const count = qr.getModuleCount();
  let d = "";
  for (let r = 0; r < count; r++) {
    for (let c = 0; c < count; c++) {
      if (qr.isDark(r, c)) d += `M${c} ${r}h1v1h-1z`;
    }
  }
  return { d, count };
}

export function QRCode({ value, size = 208, margin = 4, className, title = "Pairing QR code" }: Props) {
  const { d, count } = useMemo(() => {
    // typeNumber 0 auto-sizes to the payload; 'M' error correction survives a scuffed
    // phone screen while keeping the module count low enough to stay crisp.
    const qr = qrcode(0, "M");
    qr.addData(value);
    qr.make();
    return modulesToPath(qr);
  }, [value]);

  const dim = count + margin * 2;

  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox={`0 0 ${dim} ${dim}`}
      role="img"
      aria-label={title}
      shapeRendering="crispEdges"
    >
      <rect width={dim} height={dim} fill="#f5f3ee" />
      <g transform={`translate(${margin} ${margin})`}>
        <path d={d} fill="#12161d" />
      </g>
    </svg>
  );
}
