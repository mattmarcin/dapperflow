// A deliberately small, safe Markdown renderer for GitHub issue bodies and comments
// (the Issue tab). It renders a subset - headings, bold, inline code, fenced code,
// links, and bullet/number lists - into React elements, never raw HTML, so a hostile
// issue body cannot inject markup (attacker class: untrusted repository content).
// Links are rendered as plain styled text (no navigation) since the app has no browser;
// the Issue tab's explicit "Open on GitHub" button is the one sanctioned way out.

import { Fragment, ReactNode } from "react";

// ---- Inline: **bold**, `code`, [text](url) ---------------------------------

function renderInline(text: string, keyBase: string): ReactNode[] {
  const out: ReactNode[] = [];
  // One combined matcher, resolved in priority order per hit.
  const re = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\[[^\]]+\]\([^)]+\))/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(<Fragment key={`${keyBase}-t${i}`}>{text.slice(last, m.index)}</Fragment>);
    const tok = m[0];
    if (tok.startsWith("`")) {
      out.push(
        <code key={`${keyBase}-c${i}`} className="md-code">
          {tok.slice(1, -1)}
        </code>,
      );
    } else if (tok.startsWith("**")) {
      out.push(<strong key={`${keyBase}-b${i}`}>{tok.slice(2, -2)}</strong>);
    } else {
      const label = tok.slice(1, tok.indexOf("]"));
      out.push(
        <span key={`${keyBase}-l${i}`} className="md-link" title={tok.slice(tok.indexOf("(") + 1, -1)}>
          {label}
        </span>,
      );
    }
    last = m.index + tok.length;
    i++;
  }
  if (last < text.length) out.push(<Fragment key={`${keyBase}-t${i}`}>{text.slice(last)}</Fragment>);
  return out;
}

// ---- Block parse -----------------------------------------------------------

export function Markdown({ source }: { source: string }) {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i];

    // Fenced code block.
    if (/^```/.test(line)) {
      const buf: string[] = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      i++; // consume closing fence
      blocks.push(
        <pre key={key++} className="md-pre">
          <code>{buf.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    // Heading.
    const h = /^(#{1,4})\s+(.*)$/.exec(line);
    if (h) {
      const level = h[1].length;
      const Tag = (`h${Math.min(level + 2, 6)}` as unknown) as keyof JSX.IntrinsicElements;
      blocks.push(
        <Tag key={key++} className={`md-h md-h${level}`}>
          {renderInline(h[2], `h${key}`)}
        </Tag>,
      );
      i++;
      continue;
    }

    // Unordered list.
    if (/^\s*[-*]\s+/.test(line)) {
      const items: ReactNode[] = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) {
        const content = lines[i].replace(/^\s*[-*]\s+/, "");
        items.push(<li key={items.length}>{renderInline(content, `ul${key}-${items.length}`)}</li>);
        i++;
      }
      blocks.push(
        <ul key={key++} className="md-ul">
          {items}
        </ul>,
      );
      continue;
    }

    // Ordered list.
    if (/^\s*\d+\.\s+/.test(line)) {
      const items: ReactNode[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        const content = lines[i].replace(/^\s*\d+\.\s+/, "");
        items.push(<li key={items.length}>{renderInline(content, `ol${key}-${items.length}`)}</li>);
        i++;
      }
      blocks.push(
        <ol key={key++} className="md-ol">
          {items}
        </ol>,
      );
      continue;
    }

    // Blank line.
    if (line.trim() === "") {
      i++;
      continue;
    }

    // Paragraph: gather consecutive non-empty, non-block lines.
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !/^```/.test(lines[i]) &&
      !/^(#{1,4})\s+/.test(lines[i]) &&
      !/^\s*[-*]\s+/.test(lines[i]) &&
      !/^\s*\d+\.\s+/.test(lines[i])
    ) {
      para.push(lines[i]);
      i++;
    }
    blocks.push(
      <p key={key++} className="md-p">
        {renderInline(para.join(" "), `p${key}`)}
      </p>,
    );
  }

  return <div className="md">{blocks}</div>;
}
