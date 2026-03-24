import { useEffect } from "react";

interface WelcomeScreenProps {
  onDismiss: () => void;
}

export default function WelcomeScreen({ onDismiss }: WelcomeScreenProps) {
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") onDismiss();
    }
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [onDismiss]);

  return (
    <div className="welcome-overlay" onClick={onDismiss}>
      <div className="welcome-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="welcome-header">
          <div className="welcome-title">Welcome to ForkTTY</div>
          <div className="welcome-subtitle">
            Parallel terminals with isolated worktrees and fast recovery
          </div>
        </div>
        <div className="welcome-copy">
          One workspace is already ready. Split it for quick parallelism, or create a
          dedicated worktree when you want full git isolation.
        </div>

        <div className="welcome-actions">
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+N</span>
            <span>Spin up another workspace without leaving the current flow</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+Shift+N</span>
            <span>Create a worktree-backed workspace for isolated changes</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+D</span>
            <span>Split the active pane when one task needs multiple terminals</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+Shift+P</span>
            <span>Open the command palette for every action in one place</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+,</span>
            <span>Adjust fonts, notifications and worktree defaults</span>
          </div>
        </div>

        <div className="welcome-footer">
          <div className="welcome-hint">
            Use the Shortcuts button in the sidebar for the full keymap.
          </div>
          <button className="welcome-dismiss" onClick={onDismiss}>
            Open Workspace
          </button>
        </div>
      </div>
    </div>
  );
}
