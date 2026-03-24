import { Component } from "react";
import type { ReactNode, ErrorInfo } from "react";
import { logError } from "../lib/pty-bridge";

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

/**
 * Top-level error boundary that prevents blank screen on runtime errors.
 * Shows a recovery UI with error details and a reload button.
 */
export default class AppErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false, error: null };

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logError(`AppErrorBoundary caught: ${error.message} ${info.componentStack ?? ""}`);
  }

  private handleReload = () => {
    window.location.reload();
  };

  private handleDismiss = () => {
    this.setState({ hasError: false, error: null });
  };

  render() {
    if (!this.state.hasError) {
      return this.props.children;
    }

    return (
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          height: "100vh",
          background: "var(--bg-primary, #1e1e2e)",
          color: "var(--fg-primary, #cdd6f4)",
          fontFamily: "system-ui, -apple-system, sans-serif",
          padding: "2rem",
          textAlign: "center",
        }}
      >
        <h1 style={{ fontSize: "1.5rem", marginBottom: "0.5rem" }}>
          Something went wrong
        </h1>
        <p
          style={{
            fontSize: "0.875rem",
            color: "var(--fg-muted, #a6adc8)",
            marginBottom: "1.5rem",
            maxWidth: "480px",
          }}
        >
          ForkTTY encountered an unexpected error. Your session data is safe.
        </p>
        {this.state.error && (
          <pre
            style={{
              fontSize: "0.75rem",
              background: "var(--bg-secondary, #181825)",
              padding: "1rem",
              borderRadius: "6px",
              maxWidth: "600px",
              maxHeight: "120px",
              overflow: "auto",
              marginBottom: "1.5rem",
              textAlign: "left",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {this.state.error.message}
          </pre>
        )}
        <div style={{ display: "flex", gap: "0.75rem" }}>
          <button
            onClick={this.handleReload}
            style={{
              padding: "0.5rem 1.25rem",
              borderRadius: "6px",
              border: "none",
              background: "var(--accent, #89b4fa)",
              color: "var(--bg-primary, #1e1e2e)",
              fontWeight: 600,
              cursor: "pointer",
              fontSize: "0.875rem",
            }}
          >
            Reload
          </button>
          <button
            onClick={this.handleDismiss}
            style={{
              padding: "0.5rem 1.25rem",
              borderRadius: "6px",
              border: "1px solid var(--border, #45475a)",
              background: "transparent",
              color: "var(--fg-primary, #cdd6f4)",
              cursor: "pointer",
              fontSize: "0.875rem",
            }}
          >
            Try to continue
          </button>
        </div>
      </div>
    );
  }
}
