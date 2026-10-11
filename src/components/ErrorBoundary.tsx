import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Keeps one broken component from taking the window with it.
 *
 * React unmounts the whole tree when a render or a cleanup throws, and with
 * nothing to catch it the result is a blank window — no tabs, no sidebar, no
 * way to tell what happened. A terminal that loses its session list because a
 * renderer threw while being disposed is a worse failure than the one that
 * caused it.
 *
 * This is a backstop, not a fix: anything that lands here is a bug, and it says
 * so plainly rather than pretending otherwise.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // The webview inspector is how this project is debugged, so put it there.
    console.error("afterm: unhandled error", error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="crash">
        <div>
          <h2>afterm hit a bug</h2>
          <p>
            The interface stopped rather than carrying on in an unknown state.
            Your sessions are still running; reloading reattaches to them.
          </p>
          <pre>{this.state.error.message}</pre>
          <button className="primary" onClick={() => window.location.reload()}>
            Reload the interface
          </button>
        </div>
      </div>
    );
  }
}
