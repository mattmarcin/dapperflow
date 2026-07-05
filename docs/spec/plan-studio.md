# Plan Studio Specification

Interactive plan review, native: plans and designs are living HTML artifacts reviewed in rich chrome, never markdown scrolled in a terminal.
Implemented inside the app with no external server or browser.

## Why HTML

Markdown is flat: it cannot express containment, layout, interactive decision controls, or visual hierarchy, so plan review degrades into wall-of-text skimming.
HTML artifacts let the planning agent present real structure (columns, diagrams, mockups, embedded questions) and let the human answer precisely where the question lives.

## The loop

1. The planning agent writes a self-contained `plan.html` into its card's artifact directory.
2. It calls `dflow plan open plan.html`, then loops on `dflow plan poll` (bounded polls, see agent-cli.md; its session state becomes `awaiting_feedback`, which suspends stuck detection).
3. The card workspace's Plan tab renders the artifact in a sandboxed iframe inside the app webview with the injected review SDK; the Needs You queue gains a "plan round awaiting feedback" item.
4. The human explores, annotates, answers embedded controls, and queues feedback; Enter sends the batch.
5. The poll returns the batch as structured items; the agent revises the artifact in place and polls again.
6. Rounds repeat until the human approves (a first-class Approve action, recorded as `plan_approved`) or ends the session.

Feedback is never lost: queued items persist across GUI reloads and poll interruptions.
Both `ended` and final responses carry `next_step` guidance so the agent knows to stop polling and proceed in its main channel.

## Review chrome capabilities

- **Two modes**: explore (scroll, pan, interact) and annotate (capture targets); single-shortcut toggle.
- **Text-range annotations**: selection anchored by selector + quoted text + advisory range offsets; the **quote is the load-bearing anchor**, matched whitespace-normalized (spike 5 proved numeric offsets fragile across source line-wraps). Anchor lifecycle: `anchored | drifted | re-anchored | unanchored`; an unanchored annotation still delivers `{quote, body}` so feedback is never lost.
- **Native controls capture decisions**: radios, checkboxes, selects, inputs, textareas, and contenteditable regions work with no special markup; values arrive as structured answers.
- **Question keys**: elements may carry a question key so a re-answer replaces the earlier one instead of duplicating.
- **Custom actions**: non-native clickables opt in via a `data-action` attribute.
- **Mermaid diagrams**: pannable and zoomable while exploring; in annotate mode a node click captures diagram id, node id, and rendered label.
- **Queued feedback**: all annotations and answers accumulate visibly and send as one batch, encouraging complete review rounds over piecemeal drip.

## Layout audit gate

Broken artifacts waste review rounds, so rendering is gated:

- On load, the SDK audits for horizontal overflow, element overflow, clipped text, and overlapping text.
- Error-severity findings mask the artifact ("Show anyway" available, with a persistent banner); warnings render normally.
- Findings return to the agent through the poll as structured `layout_warnings` (selector, kind, overflow px, viewport width, severity), so the agent fixes its own rendering bugs without human involvement.
- `kind` enum includes `external_reference` (CDN/remote asset blocked by the CSP - always an error) alongside the overflow/clipping kinds; `overlapping_text` is a best-effort heuristic and never blocks alone (spike 5).

## Feedback payload (poll response items)

```json
{
  "round": 3,
  "items": [
    { "kind": "text_range", "anchor": { "selector": "#retry-plan p:nth-of-type(2)", "start": 14, "end": 52, "quote": "retry with exponential backoff" }, "body": "cap at 3 attempts, then dead-letter" },
    { "kind": "control", "question_key": "storage", "value": "sqlite" },
    { "kind": "diagram_node", "diagram": "arch", "node": "gateway", "body": "this should be optional in v1" },
    { "kind": "action", "action": "approve-migration-plan", "body": null },
    { "kind": "chat", "body": "overall direction is right, simplify section 4" }
  ],
  "layout_warnings": [],
  "next_step": "revise the artifact in place, then poll again"
}
```

## Artifact contract for authoring agents

- Self-contained HTML: local assets copied beside the artifact with relative paths; no root-absolute paths; renders identically outside the app.
- Design system priority: user-requested aesthetic > the subject project's design system > the mocked product's own UI > the bundled fallback stylesheet.
- Playbooks (authoring guidance for plan, diagram, comparison, mockup, slides) are injected into planning briefs per the recipe; they encode what good artifacts look like so quality is prompted, then enforced by the audit.

## Sandboxing and serving

Owned by `security.md / Artifact sandbox architecture`; summary:

- Sandboxed `<iframe>` inside the app webview (Tauri multiwebview was evaluated and rejected: unstable feature flag with open rendering bugs).
- The artifact document and its assets are served by the daemon's loopback HTTP endpoint via short-lived signed URLs; the iframe never holds a bearer token.
- Strict CSP (`connect-src 'none'`, assets only from the artifact origin); the review SDK and a bundled Mermaid build are injected server-side, so agent HTML must not reference CDNs (the playbooks say this and the layout audit flags external references as errors).
- The SDK communicates with the app only via postMessage with an allowlisted, versioned schema; with `sandbox` minus `allow-same-origin` the iframe origin is opaque, so **origin checks are impossible by construction** - source identity (the signed URL) plus schema validation is the trust mechanism (spike 5, proven with a 6/6 malformed-rejection self-test).
- Mermaid ships as a bundled build served same-origin (no CDN, no `unsafe-eval`; ~3.5MB, served compressed and lazy-loaded; note its API surfaces under the bundler namespace, not `window.mermaid`).
- Artifacts are user-reviewable files on disk; export produces plain HTML with the SDK stripped and assets inlined.

## Reuse

The same chrome and poll loop serve UI mockups, architecture diagrams, gate-finding reviews (M5), and any future surface where an agent needs structured human judgment on a rich document.
