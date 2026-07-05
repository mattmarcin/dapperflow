// Plan Studio review SDK. Injected server-side into the served artifact document;
// NEVER referenced by agent HTML. Runs inside the sandboxed iframe
// (sandbox="allow-scripts allow-forms", opaque origin, no same-origin), so it can
// talk to the app ONLY through postMessage. Promoted from spike 5 (proven in the
// real WebView2 window); see the design notes
//
// The artifact HTTP service (dev-artifact-plugin.ts today; the daemon's loopback
// endpoint once it lands) transpiles this file with esbuild (format:'iife') and
// serves it as a classic same-origin <script src> so the specced CSP
// `script-src <artifact-origin>` (no 'unsafe-inline') admits it.
//
// This file is authored as a TS module for type-checking (tsc sees it under src),
// but it emits a self-contained IIFE with no runtime imports. Only `import type`
// is used, which esbuild erases. The wire types are the single source of truth in
// ./protocol; PROTOCOL_VERSION is duplicated as a literal below (kept in sync by
// the assertion just under it).

import type {
  FromSdkMessage,
  ToSdkMessage,
  TextAnchor,
  LayoutWarning,
  LayoutWarningKind,
  ReviewMode,
  Severity,
  NativeControlType,
} from "./protocol";

(function reviewSdk(): void {
  "use strict";

  const PROTOCOL_VERSION = "dflow.plan.v1"; // must equal protocol.ts PROTOCOL_VERSION
  const SDK_VERSION = "dflow-review-1.0.0";
  const ARTIFACT_ID =
    document.documentElement.getAttribute("data-artifact-id") ?? "unknown";

  // ---- postMessage plumbing -----------------------------------------------
  // No same-origin, so we target "*" and rely on the APP to validate source ===
  // this iframe's contentWindow. Inbound, we accept only messages from our own
  // parent with the right version, so a sibling frame cannot drive us.

  function post(msg: FromSdkMessage): void {
    try {
      parent.postMessage(msg, "*");
    } catch (err) {
      // Nothing we can do if the channel is gone; avoid throwing into the audit.
      void err;
    }
  }

  function reportError(where: string, err: unknown): void {
    const message = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
    post({ v: PROTOCOL_VERSION, type: "sdk_error", where, message });
  }

  window.addEventListener("error", (e) => {
    post({
      v: PROTOCOL_VERSION,
      type: "sdk_error",
      where: "window.onerror",
      message: `${e.message} @ ${e.filename}:${e.lineno}`,
    });
  });
  window.addEventListener("unhandledrejection", (e) => {
    post({
      v: PROTOCOL_VERSION,
      type: "sdk_error",
      where: "unhandledrejection",
      message: String((e as PromiseRejectionEvent).reason),
    });
  });

  // ---- CSS injection (style-src allows 'unsafe-inline' per spec) -----------

  function injectStyles(): void {
    const css = `
      :root { --dflow-violet: #c98bdb; --dflow-violet-wash: rgba(201,139,219,0.16); }
      html.dflow-annotate, html.dflow-annotate body { cursor: crosshair; }
      html.dflow-annotate ::selection { background: var(--dflow-violet-wash); }
      ::highlight(dflow-annotation) {
        background: var(--dflow-violet-wash);
        text-decoration: underline wavy var(--dflow-violet);
      }
      ::highlight(dflow-annotation-focus) {
        background: rgba(201,139,219,0.34);
      }
      .dflow-anno-overlay {
        position: absolute; pointer-events: none; z-index: 2147483646;
        background: var(--dflow-violet-wash);
        border-bottom: 2px solid var(--dflow-violet);
      }
      [data-action] { cursor: pointer; }
      html.dflow-annotate [data-question-key], html.dflow-annotate [data-action] {
        outline: 1px dashed rgba(201,139,219,0.5); outline-offset: 2px;
      }
      .dflow-mermaid-wrap { overflow: hidden; position: relative; touch-action: none; }
      .dflow-mermaid-wrap svg { transform-origin: 0 0; cursor: grab; }
      html.dflow-annotate .dflow-mermaid-wrap .node { cursor: crosshair; }
      html.dflow-annotate .dflow-mermaid-wrap .node:hover * { filter: brightness(1.25); }
      .dflow-mask {
        position: fixed; inset: 0; z-index: 2147483647;
        background: repeating-linear-gradient(45deg, #12161d, #12161d 12px, #171c25 12px, #171c25 24px);
        color: #e9e7e2; font: 14px/1.5 "Segoe UI", system-ui, sans-serif;
        display: flex; align-items: center; justify-content: center; text-align: center; padding: 40px;
      }
      .dflow-mask-card { max-width: 460px; }
      .dflow-mask-card h2 { color: #e5686a; margin: 0 0 8px; font-size: 16px; }
      .dflow-mask-card p { color: #99a1ae; margin: 0; }
    `;
    const style = document.createElement("style");
    style.setAttribute("data-dflow-sdk", "");
    style.textContent = css;
    document.head.appendChild(style);
  }

  // ---- Selector + offset <-> Range mapping (item 3 core) ------------------

  /** A unique-enough CSS selector for `el`: shortest path using an id anchor. */
  function cssPath(el: Element): string {
    if (el.id) return `#${cssEscape(el.id)}`;
    const parts: string[] = [];
    let node: Element | null = el;
    let depth = 0;
    while (node && node.nodeType === 1 && depth < 8) {
      if (node.id) {
        parts.unshift(`#${cssEscape(node.id)}`);
        break;
      }
      const tag = node.tagName.toLowerCase();
      if (tag === "html" || tag === "body") {
        parts.unshift(tag);
        break;
      }
      const parent: Element | null = node.parentElement;
      if (!parent) {
        parts.unshift(tag);
        break;
      }
      const sameTag = Array.prototype.filter.call(
        parent.children,
        (c: Element) => c.tagName === node!.tagName,
      ) as Element[];
      const idx = sameTag.indexOf(node) + 1;
      parts.unshift(sameTag.length > 1 ? `${tag}:nth-of-type(${idx})` : tag);
      node = parent;
      depth++;
    }
    return parts.join(" > ");
  }

  function cssEscape(s: string): string {
    const cssApi = (window as unknown as { CSS?: { escape?: (v: string) => string } }).CSS;
    if (cssApi && typeof cssApi.escape === "function") return cssApi.escape(s);
    return s.replace(/[^a-zA-Z0-9_-]/g, (c) => `\\${c}`);
  }

  /** Character offsets of `range` within `el`'s concatenated text content. */
  function offsetsInElement(el: Element, range: Range): { start: number; end: number } | null {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    let acc = 0;
    let start = -1;
    let end = -1;
    let n: Node | null = walker.nextNode();
    while (n) {
      const len = n.textContent?.length ?? 0;
      if (n === range.startContainer) start = acc + range.startOffset;
      if (n === range.endContainer) end = acc + range.endOffset;
      acc += len;
      n = walker.nextNode();
    }
    if (start < 0 || end < 0 || end < start) return null;
    return { start, end };
  }

  /**
   * Whitespace-tolerant quote search within `root`. Real anchoring must survive the
   * mismatch between a human selection (visually collapsed whitespace) and the DOM
   * textContent (raw source whitespace, including line-wrap newlines + indentation),
   * and it must find a quote across adjacent text nodes. Returns a live Range plus
   * the raw start offset within `root`'s textContent (for the anchored/drifted call).
   */
  function findQuoteInElement(
    root: Element,
    quote: string,
  ): { range: Range; startOffset: number } | null {
    const needle = quote.replace(/\s+/g, " ").trim();
    if (needle.length < 2) return null;
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const owner: { node: Node; local: number }[] = [];
    let raw = "";
    let n: Node | null = walker.nextNode();
    while (n) {
      const s = n.textContent ?? "";
      for (let k = 0; k < s.length; k++) owner.push({ node: n, local: k });
      raw += s;
      n = walker.nextNode();
    }
    // Collapse whitespace runs, keeping a back-map from normalized -> raw index.
    let norm = "";
    const back: number[] = [];
    let inWs = false;
    for (let i = 0; i < raw.length; i++) {
      if (/\s/.test(raw[i])) {
        if (!inWs) {
          norm += " ";
          back.push(i);
          inWs = true;
        }
      } else {
        norm += raw[i];
        back.push(i);
        inWs = false;
      }
    }
    const j = norm.indexOf(needle);
    if (j < 0) return null;
    const s = owner[back[j]];
    const e = owner[back[j + needle.length - 1]];
    if (!s || !e) return null;
    try {
      const r = document.createRange();
      r.setStart(s.node, s.local);
      r.setEnd(e.node, e.local + 1);
      return { range: r, startOffset: back[j] };
    } catch (err) {
      reportError("findQuoteInElement", err);
      return null;
    }
  }

  // ---- Annotation store + highlight rendering -----------------------------

  interface StoredAnnotation {
    id: string;
    anchor: TextAnchor;
    range: Range | null;
  }
  const annotations = new Map<string, StoredAnnotation>();
  let annoSeq = 0;

  const highlightApi = (window as unknown as {
    Highlight?: new (...ranges: Range[]) => object;
    CSS?: { highlights?: Map<string, object> };
  });
  const supportsHighlightApi =
    typeof highlightApi.Highlight === "function" && !!highlightApi.CSS?.highlights;

  function renderHighlights(focusId?: string): void {
    if (supportsHighlightApi && highlightApi.Highlight && highlightApi.CSS?.highlights) {
      const base: Range[] = [];
      const focus: Range[] = [];
      for (const a of annotations.values()) {
        if (!a.range) continue;
        (a.id === focusId ? focus : base).push(a.range);
      }
      const reg = highlightApi.CSS.highlights;
      reg.set("dflow-annotation", new highlightApi.Highlight(...base));
      reg.set("dflow-annotation-focus", new highlightApi.Highlight(...focus));
    } else {
      renderOverlayFallback(focusId);
    }
  }

  // Non-mutating fallback for engines without the CSS Custom Highlight API:
  // absolutely-positioned rectangles over each range's client rects. Never mutates
  // the artifact DOM (mutation would invalidate the character offsets).
  function renderOverlayFallback(focusId?: string): void {
    document.querySelectorAll(".dflow-anno-overlay").forEach((n) => n.remove());
    for (const a of annotations.values()) {
      if (!a.range) continue;
      const rects = a.range.getClientRects();
      for (let i = 0; i < rects.length; i++) {
        const rect = rects[i];
        const box = document.createElement("div");
        box.className = "dflow-anno-overlay";
        box.style.left = `${rect.left + window.scrollX}px`;
        box.style.top = `${rect.top + window.scrollY}px`;
        box.style.width = `${rect.width}px`;
        box.style.height = `${rect.height}px`;
        if (a.id === focusId) box.style.background = "rgba(201,139,219,0.34)";
        document.body.appendChild(box);
      }
    }
  }

  // ---- Text-range annotation capture (item 3) -----------------------------

  function captureSelection(): void {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
    const range = sel.getRangeAt(0);
    const quote = sel.toString();
    if (!quote.trim()) return;

    // The anchor element is the nearest common-ancestor ELEMENT of the selection.
    let anchorEl: Node | null = range.commonAncestorContainer;
    if (anchorEl.nodeType === Node.TEXT_NODE) anchorEl = anchorEl.parentElement;
    if (!(anchorEl instanceof Element)) return;

    const offsets = offsetsInElement(anchorEl, range);
    if (!offsets) return;

    const anchor: TextAnchor = {
      selector: cssPath(anchorEl),
      start: offsets.start,
      end: offsets.end,
      quote: quote.length > 4000 ? quote.slice(0, 4000) : quote,
    };
    const id = `anno-${++annoSeq}`;
    annotations.set(id, { id, anchor, range: range.cloneRange() });
    renderHighlights(id);
    sel.removeAllRanges();
    post({ v: PROTOCOL_VERSION, type: "annotation", id, anchor, status: "anchored" });
  }

  // Re-anchor a stored/incoming anchor against the CURRENT DOM. The imprecision
  // ladder (item 3): same element + same offset = anchored; same element + moved =
  // drifted; found elsewhere = re-anchored; gone = unanchored. All matching is
  // whitespace-tolerant so a re-render (or selection-vs-textContent whitespace)
  // never spuriously breaks an anchor.
  function reanchor(anchor: TextAnchor): {
    range: Range | null;
    status: "anchored" | "drifted" | "reanchored" | "unanchored";
  } {
    let el: Element | null = null;
    try {
      el = document.querySelector(anchor.selector);
    } catch {
      el = null;
    }

    // 1/2. Selector still resolves and still contains the quote.
    if (el) {
      const hit = findQuoteInElement(el, anchor.quote);
      if (hit) {
        const status = Math.abs(hit.startOffset - anchor.start) <= 1 ? "anchored" : "drifted";
        return { range: hit.range, status };
      }
    }

    // 3. Document-wide quote search (selector or element changed entirely).
    const doc = findQuoteInElement(document.body, anchor.quote);
    if (doc) return { range: doc.range, status: "reanchored" };

    // 4. Lost: the app keeps the quote+body so feedback still reaches the agent.
    return { range: null, status: "unanchored" };
  }

  // App-initiated annotation by quote: locate the quote, derive the same anchor a
  // manual selection would (nearest enclosing element + offsets), store and emit.
  function annotateByQuote(id: string, quote: string): void {
    const range = findQuoteInDocument(quote);
    if (!range) {
      const anchor: TextAnchor = { selector: "body", start: 0, end: 0, quote };
      annotations.set(id, { id, anchor, range: null });
      post({ v: PROTOCOL_VERSION, type: "annotation", id, anchor, status: "unanchored" });
      return;
    }
    let anchorEl: Node | null = range.commonAncestorContainer;
    if (anchorEl.nodeType === Node.TEXT_NODE) anchorEl = anchorEl.parentElement;
    if (!(anchorEl instanceof Element)) return;
    const offsets = offsetsInElement(anchorEl, range);
    if (!offsets) return;
    const anchor: TextAnchor = {
      selector: cssPath(anchorEl),
      start: offsets.start,
      end: offsets.end,
      quote,
    };
    annotations.set(id, { id, anchor, range: range.cloneRange() });
    renderHighlights();
    post({ v: PROTOCOL_VERSION, type: "annotation", id, anchor, status: "anchored" });
  }

  function findQuoteInDocument(quote: string): Range | null {
    return findQuoteInElement(document.body, quote)?.range ?? null;
  }

  // ---- Native control capture (item 4) ------------------------------------

  function controlKey(el: Element): string {
    return (
      el.getAttribute("data-question-key") ||
      el.getAttribute("name") ||
      cssPath(el)
    );
  }

  function controlLabel(el: Element): string | undefined {
    const id = el.getAttribute("id");
    if (id) {
      const lab = document.querySelector(`label[for="${cssEscape(id)}"]`);
      if (lab?.textContent) return lab.textContent.trim().slice(0, 120);
    }
    const wrapLabel = el.closest("label");
    if (wrapLabel?.textContent) return wrapLabel.textContent.trim().slice(0, 120);
    const aria = el.getAttribute("aria-label");
    return aria ? aria.slice(0, 120) : undefined;
  }

  function readControl(el: Element): {
    value: string | string[] | boolean;
    control_type: NativeControlType;
  } | null {
    if (el instanceof HTMLInputElement) {
      if (el.type === "radio") {
        if (!el.checked) return null; // only the chosen radio reports
        return { value: el.value, control_type: "radio" };
      }
      if (el.type === "checkbox") {
        return { value: el.checked, control_type: "checkbox" };
      }
      return { value: el.value, control_type: "text" };
    }
    if (el instanceof HTMLTextAreaElement) {
      return { value: el.value, control_type: "textarea" };
    }
    if (el instanceof HTMLSelectElement) {
      if (el.multiple) {
        return {
          value: Array.from(el.selectedOptions).map((o) => o.value),
          control_type: "select",
        };
      }
      return { value: el.value, control_type: "select" };
    }
    if (el instanceof HTMLElement && el.isContentEditable) {
      return { value: (el.textContent ?? "").trim(), control_type: "contenteditable" };
    }
    return null;
  }

  function emitControl(el: Element): void {
    const read = readControl(el);
    if (!read) return;
    // Radio groups key by the group name so a re-answer replaces (question keys).
    const keyEl =
      el instanceof HTMLInputElement && el.type === "radio" && el.name
        ? el
        : el;
    const question_key =
      el instanceof HTMLInputElement && el.type === "radio"
        ? el.getAttribute("data-question-key") || el.name || cssPath(el)
        : controlKey(keyEl);
    post({
      v: PROTOCOL_VERSION,
      type: "control",
      question_key,
      value: read.value,
      control_type: read.control_type,
      label: controlLabel(el),
    });
  }

  function wireControls(): void {
    const onChange = (e: Event) => {
      const t = e.target as Element | null;
      if (!t) return;
      if (
        t instanceof HTMLInputElement ||
        t instanceof HTMLTextAreaElement ||
        t instanceof HTMLSelectElement
      ) {
        emitControl(t);
      }
    };
    document.addEventListener("change", onChange, true);
    // contenteditable: capture on blur so we send the settled value once.
    document.addEventListener(
      "blur",
      (e) => {
        const t = e.target as Element | null;
        if (t instanceof HTMLElement && t.isContentEditable) emitControl(t);
      },
      true,
    );
    // Custom data-action clickables (item 4b): fire in any mode.
    document.addEventListener(
      "click",
      (e) => {
        const t = (e.target as Element | null)?.closest("[data-action]");
        if (!t) return;
        e.preventDefault();
        const data: Record<string, string> = {};
        for (const attr of Array.from(t.attributes)) {
          if (attr.name.startsWith("data-") && attr.name !== "data-action") {
            data[attr.name.slice(5)] = attr.value;
          }
        }
        post({
          v: PROTOCOL_VERSION,
          type: "action",
          action: t.getAttribute("data-action") || "",
          data: Object.keys(data).length ? data : undefined,
        });
      },
      true,
    );
  }

  // ---- Layout audit (item 5) ----------------------------------------------

  const OVERFLOW_TOL = 2; // px slack so sub-pixel rounding is not a finding

  function auditLayout(): { warnings: LayoutWarning[]; hasError: boolean } {
    const warnings: LayoutWarning[] = [];
    const vw = document.documentElement.clientWidth;
    const push = (
      selector: string,
      kind: LayoutWarningKind,
      overflow_px: number,
      severity: Severity,
      detail?: string,
    ) => warnings.push({ selector, kind, overflow_px: Math.round(overflow_px), viewport_width: vw, severity, detail });

    try {
      // A. Page-level horizontal overflow.
      const docW = document.documentElement.scrollWidth;
      if (docW - vw > OVERFLOW_TOL) {
        push("html", "horizontal_overflow", docW - vw, "error", "document scrolls horizontally");
      }

      // B/C/D. Per-element checks over a bounded element set.
      const all = Array.from(document.body.querySelectorAll("*")).slice(0, 4000);
      const textLeaves: { el: Element; rect: DOMRect }[] = [];
      for (const el of all) {
        if (el.closest("[data-dflow-sdk]")) continue;
        const cs = getComputedStyle(el);
        if (cs.display === "none" || cs.visibility === "hidden") continue;
        const rect = el.getBoundingClientRect();

        // Element extends past the right viewport edge.
        if (rect.width > 0 && rect.right - vw > OVERFLOW_TOL) {
          push(cssPath(el), "element_overflow", rect.right - vw, "error", "element crosses the right edge");
        }

        // Content overflows the element's own box while clipped.
        const clipsX = cs.overflowX === "hidden" || cs.overflowX === "clip";
        const clipsY = cs.overflowY === "hidden" || cs.overflowY === "clip";
        if (clipsX && el.scrollWidth - el.clientWidth > OVERFLOW_TOL) {
          push(cssPath(el), "clipped_text", el.scrollWidth - el.clientWidth, "warning", "text clipped horizontally");
        }
        if (clipsY && el.scrollHeight - el.clientHeight > OVERFLOW_TOL) {
          push(cssPath(el), "clipped_text", el.scrollHeight - el.clientHeight, "warning", "content clipped vertically");
        }

        // Collect leaf text boxes for the overlap pass.
        if (
          rect.width > 4 &&
          rect.height > 4 &&
          el.children.length === 0 &&
          (el.textContent?.trim().length ?? 0) > 0
        ) {
          textLeaves.push({ el, rect });
        }
      }

      // E. Overlapping text (heuristic, bounded pairwise over leaf text boxes).
      const cap = Math.min(textLeaves.length, 300);
      for (let i = 0; i < cap; i++) {
        for (let j = i + 1; j < cap; j++) {
          const a = textLeaves[i].rect;
          const b = textLeaves[j].rect;
          const ox = Math.min(a.right, b.right) - Math.max(a.left, b.left);
          const oy = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
          if (ox > 3 && oy > 3) {
            const area = ox * oy;
            const minArea = Math.min(a.width * a.height, b.width * b.height);
            if (area > minArea * 0.35) {
              push(cssPath(textLeaves[j].el), "overlapping_text", Math.round(ox), "error", "text boxes overlap");
            }
          }
        }
      }

      // F. External references (CSP posture): any subresource off the artifact
      // origin. Real CSP would block these; the audit flags them proactively.
      const origin = location.origin;
      const refs = document.querySelectorAll<HTMLElement>("[src],[href]");
      for (const el of Array.from(refs)) {
        if (el.closest("[data-dflow-sdk]")) continue;
        const url = el.getAttribute("src") || el.getAttribute("href") || "";
        if (/^https?:\/\//i.test(url) && !url.startsWith(origin)) {
          push(cssPath(el), "external_reference", 0, "error", `off-origin reference: ${url.slice(0, 80)}`);
        }
      }
    } catch (err) {
      reportError("auditLayout", err);
    }

    return { warnings, hasError: warnings.some((w) => w.severity === "error") };
  }

  // ---- Mask-until-clean gate (item 5) -------------------------------------

  let maskEl: HTMLElement | null = null;
  function showMask(errorCount: number): void {
    if (maskEl) return;
    maskEl = document.createElement("div");
    maskEl.className = "dflow-mask";
    maskEl.setAttribute("data-dflow-sdk", "");
    maskEl.innerHTML =
      `<div class="dflow-mask-card"><h2>Rendering issues found</h2>` +
      `<p>${errorCount} error-severity layout finding${errorCount === 1 ? "" : "s"} ` +
      `masked this artifact. Use “Show anyway” in the review bar to inspect it.</p></div>`;
    document.body.appendChild(maskEl);
  }
  function hideMask(): void {
    maskEl?.remove();
    maskEl = null;
  }

  // ---- Mode + Mermaid (item 2 toggle + Mermaid diagrams) ------------------

  let mode: ReviewMode = "explore";
  function setMode(next: ReviewMode, announce: boolean): void {
    if (mode === next) return;
    mode = next;
    document.documentElement.classList.toggle("dflow-annotate", mode === "annotate");
    if (announce) post({ v: PROTOCOL_VERSION, type: "mode_changed", mode });
  }

  function wireModeShortcut(): void {
    window.addEventListener("keydown", (e) => {
      // Single-shortcut toggle ("a"), suppressed while editing a field.
      const t = e.target as HTMLElement | null;
      const editing =
        t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.isContentEditable);
      if (editing) return;
      if ((e.key === "a" || e.key === "A") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        setMode(mode === "annotate" ? "explore" : "annotate", true);
      }
    });
    // In annotate mode a settled selection becomes an annotation.
    document.addEventListener("mouseup", () => {
      if (mode !== "annotate") return;
      // Defer so the selection is finalized.
      window.setTimeout(() => {
        try {
          captureSelection();
        } catch (err) {
          reportError("captureSelection", err);
        }
      }, 0);
    });
  }

  // The bundled mermaid.min.js is an esbuild IIFE that exposes the module at
  // window.__esbuild_esm_mermaid_nm.mermaid (default export = the API), not a bare
  // window.mermaid. Resolve it defensively so a future build shape still works.
  function resolveMermaid(): MermaidLike | null {
    const w = window as unknown as {
      mermaid?: unknown;
      __esbuild_esm_mermaid_nm?: { mermaid?: { default?: unknown } | unknown };
    };
    const ns = w.__esbuild_esm_mermaid_nm?.mermaid as { default?: unknown } | undefined;
    const cand = (ns && (ns.default ?? ns)) ?? w.mermaid;
    if (cand && typeof (cand as MermaidLike).run === "function") return cand as MermaidLike;
    return null;
  }

  function wireMermaid(): void {
    const mermaid = resolveMermaid();
    const blocks = Array.from(document.querySelectorAll<HTMLElement>(".mermaid"));
    if (!mermaid || blocks.length === 0) return;
    try {
      mermaid.initialize({ startOnLoad: false, theme: "dark", securityLevel: "strict" });
      mermaid
        .run({ nodes: blocks })
        .then(() => {
          for (const block of blocks) enhanceMermaid(block);
        })
        .catch((err: unknown) => reportError("mermaid.run", err));
    } catch (err) {
      reportError("mermaid.init", err);
    }
  }

  function enhanceMermaid(block: HTMLElement): void {
    const svg = block.querySelector("svg");
    if (!svg) return;
    const diagramId = block.getAttribute("data-diagram") || block.id || "diagram";

    // Pan/zoom wrapper (explore mode). A CSS transform keeps it non-destructive.
    const wrap = document.createElement("div");
    wrap.className = "dflow-mermaid-wrap";
    svg.parentElement?.insertBefore(wrap, svg);
    wrap.appendChild(svg);
    wrap.style.height = `${Math.min(svg.getBoundingClientRect().height || 320, 460)}px`;

    let scale = 1;
    let tx = 0;
    let ty = 0;
    let dragging = false;
    let lastX = 0;
    let lastY = 0;
    const apply = () => {
      svg.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
    };
    wrap.addEventListener(
      "wheel",
      (e) => {
        if (mode !== "explore") return;
        e.preventDefault();
        const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
        scale = Math.min(4, Math.max(0.3, scale * factor));
        apply();
      },
      { passive: false },
    );
    wrap.addEventListener("pointerdown", (e) => {
      if (mode !== "explore") return;
      dragging = true;
      lastX = e.clientX;
      lastY = e.clientY;
      (e.target as Element).setPointerCapture?.(e.pointerId);
      svg.style.cursor = "grabbing";
    });
    wrap.addEventListener("pointermove", (e) => {
      if (!dragging) return;
      tx += e.clientX - lastX;
      ty += e.clientY - lastY;
      lastX = e.clientX;
      lastY = e.clientY;
      apply();
    });
    const endDrag = () => {
      dragging = false;
      svg.style.cursor = "grab";
    };
    wrap.addEventListener("pointerup", endDrag);
    wrap.addEventListener("pointercancel", endDrag);

    // Annotate mode: a node click captures {diagram, node, label}.
    const nodes = svg.querySelectorAll<SVGElement>(".node");
    nodes.forEach((node) => {
      node.addEventListener("click", (e) => {
        if (mode !== "annotate") return;
        e.stopPropagation();
        const rawId = node.id || "";
        // mermaid ids look like "mermaid-<ts>-flowchart-gateway-1"; recover the
        // author's node id ("gateway") from the trailing flowchart-<id>-<n> segment.
        const m = rawId.match(/flowchart-(.+?)-\d+$/);
        const nodeId = m ? m[1] : rawId;
        const label = (node.querySelector(".nodeLabel")?.textContent || node.textContent || "").trim();
        post({
          v: PROTOCOL_VERSION,
          type: "diagram_node",
          diagram: diagramId,
          node: nodeId,
          label: label.slice(0, 200),
        });
      });
    });
  }

  interface MermaidLike {
    initialize(cfg: Record<string, unknown>): void;
    run(opts: { nodes: HTMLElement[] }): Promise<void>;
  }

  // ---- Inbound app -> SDK messages ----------------------------------------

  window.addEventListener("message", (e: MessageEvent) => {
    if (e.source !== parent) return; // only our host may drive us
    const msg = e.data as ToSdkMessage;
    if (!msg || typeof msg !== "object" || msg.v !== PROTOCOL_VERSION) return;
    try {
      switch (msg.type) {
        case "set_mode":
          setMode(msg.mode, false);
          break;
        case "focus_annotation": {
          const { range } = reanchor(msg.anchor);
          if (range) {
            const id = `focus-${++annoSeq}`;
            annotations.set(id, { id, anchor: msg.anchor, range });
            renderHighlights(id);
            range.startContainer.parentElement?.scrollIntoView({ block: "center", behavior: "smooth" });
            annotations.delete(id);
          }
          break;
        }
        case "clear_annotation":
          annotations.delete(msg.id);
          renderHighlights();
          break;
        case "reveal_masked":
          hideMask();
          break;
        case "reanchor": {
          for (const { id, anchor } of msg.anchors) {
            const { range, status } = reanchor(anchor);
            annotations.set(id, { id, anchor, range });
            post({ v: PROTOCOL_VERSION, type: "annotation", id, anchor, status });
          }
          renderHighlights();
          break;
        }
        case "annotate_quote": {
          for (const { id, quote } of msg.quotes) annotateByQuote(id, quote);
          break;
        }
        default:
          break;
      }
    } catch (err) {
      reportError("inbound", err);
    }
  });

  // ---- Boot ----------------------------------------------------------------

  function boot(): void {
    injectStyles();
    wireControls();
    wireModeShortcut();
    wireMermaid();

    const { warnings, hasError } = auditLayout();
    if (hasError) showMask(warnings.filter((w) => w.severity === "error").length);
    post({
      v: PROTOCOL_VERSION,
      type: "layout_audit",
      warnings,
      viewportWidth: document.documentElement.clientWidth,
      masked: hasError,
    });

    post({ v: PROTOCOL_VERSION, type: "ready", artifactId: ARTIFACT_ID, sdkVersion: SDK_VERSION });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", () => {
      try {
        boot();
      } catch (err) {
        reportError("boot", err);
      }
    });
  } else {
    try {
      boot();
    } catch (err) {
      reportError("boot", err);
    }
  }
})();
