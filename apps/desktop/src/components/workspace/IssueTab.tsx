// The Issue tab: shown on the card workspace for origin=github_issue cards
// (product.md / Card sources: GitHub issue import). Renders the issue body, labels,
// and comments, links out to GitHub, and offers a prominent "Dispatch to fix" that
// starts normal dispatch (reusing the same DispatchAffordance the Terminal tab uses).
// Fixture-tolerant: if the daemon cannot serve the issue body, the tab still shows the
// origin reference, the link out, and the dispatch action.

import { useEffect, useState } from "react";
import { useStore } from "../../state/store";
import { Card, GithubIssue, Harness, Session } from "../../model";
import { isLive } from "../../lib/session-state";
import { Markdown } from "../../lib/markdown";
import { LabelChip } from "../settings/GithubSection";
import { DispatchAffordance } from "./DispatchAffordance";
import { isGrantPending } from "../../lib/recipes";
import { elapsed } from "../../lib/format";
import { useNow } from "../../lib/use-now";

interface Props {
  card: Card;
  sessions: Session[];
}

function openExternal(url: string) {
  // The app has no in-webview browser; open the issue in the OS browser. In a plain
  // dev browser this opens a tab; in the Tauri shell it hands off to the default browser.
  window.open(url, "_blank", "noopener,noreferrer");
}

export function IssueTab({ card, sessions }: Props) {
  const store = useStore();
  const [issue, setIssue] = useState<GithubIssue | null | "loading">("loading");
  const [busy, setBusy] = useState(false);
  const now = useNow(30_000);

  useEffect(() => {
    let cancelled = false;
    setIssue("loading");
    store
      .getGithubIssueForCard(card)
      .then((i) => !cancelled && setIssue(i))
      .catch(() => !cancelled && setIssue(null));
    return () => {
      cancelled = true;
    };
  }, [store, card]);

  const hasLive = sessions.some((s) => isLive(s.state));

  const dispatchFix = async (harness: Harness) => {
    if (busy) return;
    setBusy(true);
    try {
      await store.dispatch({ card_id: card.id, harness });
      store.flash(`Dispatched ${harness} to fix ${issueRef(card)}.`);
      store.setWorkspaceTab("terminal");
    } catch (e) {
      if (!isGrantPending(e)) store.flash(String(e instanceof Error ? e.message : e), { tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  // Prefer the fetched issue's number/url; fall back to the card's origin_ref so the
  // tab is useful even when the body could not be fetched.
  const ref = issueRef(card);
  const number = issue && issue !== "loading" ? issue.number : Number(card.origin_ref?.split("#")[1]) || null;
  const url =
    issue && issue !== "loading"
      ? issue.url
      : card.origin_ref
        ? `https://github.com/${card.origin_ref.replace("#", "/issues/")}`
        : null;

  return (
    <div className="issue-tab">
      <div className="issue-scroll">
        <article className="issue-doc">
          <header className="issue-head">
            <div className="issue-eyebrow">
              <GithubMark />
              <span className="issue-origin">{ref}</span>
              {issue && issue !== "loading" ? (
                <span className={`issue-state is-${issue.state}`}>
                  <span className="issue-state-dot" aria-hidden />
                  {issue.state}
                </span>
              ) : null}
            </div>
            <h1 className="issue-title">
              {issue && issue !== "loading" ? issue.title : card.title}
            </h1>
            {issue && issue !== "loading" ? (
              <div className="issue-byline">
                opened by <strong>@{issue.author}</strong> · updated {elapsed(issue.updated_at, now)} ago
                {issue.assignees.length > 0 ? (
                  <>
                    {" "}
                    · assigned <strong>@{issue.assignees.join(", @")}</strong>
                  </>
                ) : null}
                {issue.milestone ? <> · ◈ {issue.milestone}</> : null}
              </div>
            ) : null}
            {issue && issue !== "loading" && issue.labels.length > 0 ? (
              <div className="issue-labels">
                {issue.labels.map((l) => (
                  <LabelChip key={l.name} name={l.name} color={l.color} />
                ))}
              </div>
            ) : null}
          </header>

          {issue === "loading" ? (
            <p className="issue-loading">Loading issue from gh…</p>
          ) : issue ? (
            <section className="issue-body">
              <Markdown source={issue.body} />
            </section>
          ) : (
            <section className="issue-body issue-body-degraded">
              <p>
                The issue body is not cached locally yet. Open it on GitHub to read the full thread, then
                dispatch an agent to fix it.
              </p>
            </section>
          )}

          {issue && issue !== "loading" && issue.comments.length > 0 ? (
            <section className="issue-comments">
              <div className="issue-comments-head">
                {issue.comments.length} comment{issue.comments.length === 1 ? "" : "s"}
              </div>
              {issue.comments.map((c, i) => (
                <div className="issue-comment" key={i}>
                  <div className="issue-comment-head">
                    <span className="issue-comment-author">@{c.author}</span>
                    <span className="issue-comment-time">{elapsed(c.created_at, now)} ago</span>
                  </div>
                  <div className="issue-comment-body">
                    <Markdown source={c.body} />
                  </div>
                </div>
              ))}
            </section>
          ) : null}
        </article>
      </div>

      <aside className="issue-side">
        <div className="issue-action-card">
          <h3 className="issue-action-title">Delegate this issue</h3>
          <p className="issue-action-sub">
            Delegation is normal dispatch: pick a harness and go. When the fix ships through the gate, the PR
            body's <code>Fixes {ref}</code> closes the issue on merge.
          </p>
          {hasLive ? (
            <div className="issue-live">
              <span className="issue-live-note">An agent is already working this card.</span>
              <button className="btn-primary issue-dispatch" onClick={() => store.setWorkspaceTab("terminal")}>
                Open terminal
              </button>
            </div>
          ) : (
            <DispatchAffordance
              title="Dispatch to fix"
              subtitle="A gate-verified fix, straight from the issue."
              cta="Dispatch to fix"
              busy={busy}
              onGo={dispatchFix}
            />
          )}
        </div>

        {url ? (
          <button className="btn-ghost issue-linkout" onClick={() => openExternal(url)}>
            <IconExternal />
            Open {number ? `#${number}` : "issue"} on GitHub
          </button>
        ) : null}
      </aside>
    </div>
  );
}

function issueRef(card: Card): string {
  if (!card.origin_ref) return "this issue";
  const [, num] = card.origin_ref.split("#");
  return `#${num}`;
}

function GithubMark() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

function IconExternal() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M6 3H3.5v9.5H13V10M9 3h4v4M13 3L7.5 8.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
