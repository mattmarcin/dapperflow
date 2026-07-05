import { Component, ErrorInfo, ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Optional label for the surface that failed, shown in the fallback copy. */
  label?: string;
  /**
   * When this value changes, a boundary that is currently showing its fallback resets and
   * re-renders its children. The main pane passes the current route (view + open card +
   * open session) so navigating away from a broken view recovers automatically.
   */
  resetKey?: string;
}

interface State {
  error: Error | null;
}

/**
 * A boundary around the main content pane so a render error in one view or overlay (a bad
 * data shape, an unexpected null) degrades to an inline, recoverable message instead of
 * unmounting the whole app to a blank screen. The sidebar, top bar, status bar, and the
 * live terminal pool stay mounted, so the user can navigate out and keep working.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Keep the detail in the console for debugging; the UI stays calm.
    console.error("[dapperflow] view error boundary caught:", error, info.componentStack);
  }

  componentDidUpdate(prev: Props): void {
    // Recover on navigation: a new route means a different subtree, so drop the error.
    if (this.state.error && prev.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  private reset = (): void => this.setState({ error: null });

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className="view-error" role="alert">
          <div className="view-error-inner">
            <svg className="view-error-mark" width="32" height="32" viewBox="0 0 32 32" aria-hidden>
              <circle cx="16" cy="16" r="13" fill="none" stroke="currentColor" strokeWidth="2" opacity="0.5" />
              <path d="M16 9v8" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" />
              <circle cx="16" cy="22" r="1.4" fill="currentColor" />
            </svg>
            <h2 className="view-error-title">This view hit a snag</h2>
            <p className="view-error-detail">
              {this.props.label ? `${this.props.label} ` : ""}Something in this pane failed to
              render. The rest of the cockpit is still live - switch views or try again.
            </p>
            <p className="view-error-msg">{this.state.error.message}</p>
            <button className="btn-primary btn-sm" onClick={this.reset}>
              Try again
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
