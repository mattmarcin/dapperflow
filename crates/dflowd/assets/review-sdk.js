/*
 * DapperFlow Plan Studio review SDK (`plan-studio.md`, `spike5-plan-studio-chrome.md`).
 *
 * Injected server-side by the daemon's loopback artifact service as a same-origin
 * <script src>, so it runs under the strict CSP (`script-src 'self'`, no unsafe-inline,
 * no unsafe-eval) with no import/export - a self-contained IIFE (the spike's proven
 * shape). It talks to the app only through postMessage with a versioned, allowlisted
 * schema; with `sandbox` minus `allow-same-origin` the iframe origin is opaque, so the
 * app trusts source identity + schema, never event.origin.
 *
 * Packaging note: in a packaged build the daemon serves the desktop app's promoted
 * `src/review/review-sdk.ts` build output over this same message schema; this embedded
 * asset is the daemon-owned, dependency-free implementation of that contract so the
 * loopback artifact service is self-sufficient and testable on its own.
 */
(function () {
  "use strict";

  var SCHEMA = "dflow.plan.v1";
  var script = document.currentScript || (function () {
    var s = document.querySelectorAll('script[data-artifact-id]');
    return s.length ? s[s.length - 1] : null;
  })();
  var ARTIFACT_ID = script ? script.getAttribute("data-artifact-id") : "";
  var ROUND = script ? parseInt(script.getAttribute("data-round") || "1", 10) : 1;

  var state = {
    mode: "explore",
    queue: [],          // FeedbackItem[]
    controls: {},        // question_key -> item index in queue
    warnings: [],
    masked: false,
  };

  // ---- postMessage plumbing -------------------------------------------------

  function post(type, payload) {
    try {
      parent.postMessage({ v: SCHEMA, type: type, artifact_id: ARTIFACT_ID, payload: payload || {} }, "*");
    } catch (e) { /* opaque origin; parent gates on source identity */ }
  }

  // Only accept messages from the true parent window, validated against the allowlist.
  window.addEventListener("message", function (ev) {
    if (ev.source !== window.parent) return;
    var msg = ev.data;
    if (!msg || msg.v !== SCHEMA || typeof msg.type !== "string") return;
    switch (msg.type) {
      case "set_mode":
        if (msg.payload && (msg.payload.mode === "explore" || msg.payload.mode === "annotate")) {
          setMode(msg.payload.mode);
        }
        break;
      case "reveal_masked":
        unmask();
        break;
      case "request_batch":
        sendBatch();
        break;
      case "clear_queue":
        state.queue = [];
        state.controls = {};
        post("queue_changed", { count: 0 });
        break;
      default:
        break; // unknown types are preserved, never acted on
    }
  });

  // ---- mode -----------------------------------------------------------------

  function setMode(mode) {
    state.mode = mode;
    document.documentElement.setAttribute("data-dflow-mode", mode);
    post("mode_changed", { mode: mode });
  }

  document.addEventListener("keydown", function (ev) {
    var t = ev.target;
    var typing = t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
    if (typing) return;
    if (ev.key === "a" || ev.key === "A") {
      setMode(state.mode === "explore" ? "annotate" : "explore");
    } else if (ev.key === "Enter" && (ev.metaKey || ev.ctrlKey)) {
      sendBatch();
    }
  });

  // ---- queue ----------------------------------------------------------------

  function enqueue(item) {
    if (item.kind === "control" && item.question_key) {
      // A re-answer replaces the earlier one instead of duplicating (question keys).
      if (Object.prototype.hasOwnProperty.call(state.controls, item.question_key)) {
        state.queue[state.controls[item.question_key]] = item;
      } else {
        state.controls[item.question_key] = state.queue.length;
        state.queue.push(item);
      }
    } else {
      state.queue.push(item);
    }
    post("queue_changed", { count: state.queue.length, item: item });
  }

  function sendBatch() {
    post("submit", { round: ROUND, items: state.queue.slice(), layout_warnings: state.warnings });
  }

  // ---- text-range annotation (quote is the load-bearing anchor) -------------

  function cssSelector(el) {
    if (!el || el.nodeType !== 1) return "";
    if (el.id) return "#" + CSS.escape(el.id);
    var parts = [];
    while (el && el.nodeType === 1 && parts.length < 6) {
      var name = el.tagName.toLowerCase();
      var parent = el.parentElement;
      if (parent) {
        var same = Array.prototype.filter.call(parent.children, function (c) { return c.tagName === el.tagName; });
        if (same.length > 1) name += ":nth-of-type(" + (same.indexOf(el) + 1) + ")";
      }
      parts.unshift(name);
      if (el.id) { parts[0] = "#" + CSS.escape(el.id); break; }
      el = parent;
    }
    return parts.join(" > ");
  }

  document.addEventListener("mouseup", function () {
    if (state.mode !== "annotate") return;
    var sel = window.getSelection();
    if (!sel || sel.isCollapsed) return;
    var quote = sel.toString().replace(/\s+/g, " ").trim();
    if (!quote) return;
    var anchorNode = sel.anchorNode;
    var el = anchorNode && anchorNode.nodeType === 3 ? anchorNode.parentElement : anchorNode;
    var selector = cssSelector(el);
    var text = el ? (el.textContent || "").replace(/\s+/g, " ") : "";
    var start = text.indexOf(quote);
    var end = start >= 0 ? start + quote.length : -1;
    highlight(sel);
    // The prompt for a body is the app's job; the SDK captures the anchor and posts it.
    post("annotation_captured", {
      kind: "text_range",
      anchor: { selector: selector, start: start, end: end, quote: quote },
      status: "anchored",
    });
  });

  function highlight(sel) {
    try {
      if (window.CSS && CSS.highlights && window.Highlight) {
        var h = CSS.highlights.get("dflow-annotation") || new Highlight();
        for (var i = 0; i < sel.rangeCount; i++) h.add(sel.getRangeAt(i).cloneRange());
        CSS.highlights.set("dflow-annotation", h);
      }
    } catch (e) { /* non-mutating highlight is best-effort */ }
  }

  // The app calls back with the human's body text for a captured anchor.
  window.addEventListener("message", function (ev) {
    if (ev.source !== window.parent) return;
    var msg = ev.data;
    if (!msg || msg.v !== SCHEMA) return;
    if (msg.type === "queue_annotation" && msg.payload) {
      enqueue(msg.payload); // a fully-formed FeedbackItem from the app UI
    }
  });

  // ---- native control capture ----------------------------------------------

  function controlValue(el) {
    if (el.type === "checkbox") return el.checked;
    if (el.type === "radio") return el.value;
    if (el.isContentEditable) return el.textContent;
    return el.value;
  }
  function questionKey(el) {
    return el.getAttribute("data-question-key") || el.name || cssSelector(el);
  }
  function captureControl(el) {
    enqueue({ kind: "control", question_key: questionKey(el), value: controlValue(el) });
  }
  document.addEventListener("change", function (ev) {
    var el = ev.target;
    if (!el || !el.tagName) return;
    if (["INPUT", "SELECT", "TEXTAREA"].indexOf(el.tagName) >= 0) captureControl(el);
  });
  document.addEventListener("blur", function (ev) {
    var el = ev.target;
    if (el && (el.tagName === "TEXTAREA" || el.isContentEditable)) captureControl(el);
  }, true);

  // ---- data-action clickables ----------------------------------------------

  document.addEventListener("click", function (ev) {
    var el = ev.target.closest ? ev.target.closest("[data-action]") : null;
    if (!el) return;
    ev.preventDefault();
    enqueue({ kind: "action", action: el.getAttribute("data-action"), body: el.getAttribute("data-action-body") || null });
  });

  // ---- layout audit (mask until clean) -------------------------------------

  function audit() {
    var warnings = [];
    var vw = document.documentElement.clientWidth;
    if (document.documentElement.scrollWidth > vw + 2) {
      warnings.push(warn("html", "horizontal_overflow", document.documentElement.scrollWidth - vw, vw, "error"));
    }
    var all = document.body ? document.body.querySelectorAll("*") : [];
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      var r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      if (r.right > vw + 2) warnings.push(warn(cssSelector(el), "element_overflow", r.right - vw, vw, "error"));
      if (el.scrollWidth > el.clientWidth + 2 && getComputedStyle(el).overflow === "hidden") {
        warnings.push(warn(cssSelector(el), "clipped_text", el.scrollWidth - el.clientWidth, vw, "warning"));
      }
    }
    // external references blocked by the CSP surface as failed loads (always error).
    var imgs = document.querySelectorAll("img");
    for (var j = 0; j < imgs.length; j++) {
      var im = imgs[j];
      if (im.getAttribute("src") && /^https?:\/\//.test(im.getAttribute("src"))) {
        warnings.push(warn(cssSelector(im), "external_reference", 0, vw, "error"));
      }
    }
    state.warnings = warnings;
    var hasError = warnings.some(function (w) { return w.severity === "error"; });
    if (hasError) mask(); else unmask();
    post("audit", { layout_warnings: warnings });
  }
  function warn(selector, kind, px, vw, sev) {
    return { selector: selector, kind: kind, overflow_px: Math.round(px), viewport_width: vw, severity: sev };
  }
  function mask() {
    state.masked = true;
    post("masked", { warnings: state.warnings });
  }
  function unmask() {
    state.masked = false;
    post("unmasked", {});
  }

  // ---- mermaid (bundled build, if injected) --------------------------------

  function initMermaid() {
    var ns = window.__esbuild_esm_mermaid_nm && window.__esbuild_esm_mermaid_nm.mermaid;
    var mermaid = ns || window.mermaid;
    if (!mermaid || typeof mermaid.run !== "function") return;
    try { mermaid.run({ querySelector: ".mermaid" }); } catch (e) { /* best effort */ }
  }

  // ---- boot -----------------------------------------------------------------

  function boot() {
    setMode("explore");
    initMermaid();
    audit();
    post("ready", { schema: SCHEMA, artifact_id: ARTIFACT_ID, round: ROUND });
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
  window.addEventListener("resize", audit);
  window.addEventListener("error", function (ev) {
    post("sdk_error", { message: String(ev && ev.message || "error") });
  });
})();
