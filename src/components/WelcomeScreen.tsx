interface WelcomeScreenProps {
  onDismiss: () => void;
}

export default function WelcomeScreen({ onDismiss }: WelcomeScreenProps) {
  return (
    <div className="welcome-overlay" onClick={onDismiss}>
      <div className="welcome-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="welcome-title">Welcome to ForkTTY</div>
        <div className="welcome-subtitle">
          Multi-agent terminal with isolated worktrees
        </div>

        <div className="welcome-actions">
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+N</span>
            <span>New workspace</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+Shift+N</span>
            <span>New worktree workspace</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+D</span>
            <span>Split terminal right</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+Shift+P</span>
            <span>Command palette</span>
          </div>
          <div className="welcome-action">
            <span className="welcome-shortcut">Ctrl+,</span>
            <span>Settings</span>
          </div>
        </div>

        <button className="welcome-dismiss" onClick={onDismiss}>
          Start working
        </button>

        <div className="welcome-hint">
          Press ? in the sidebar for all shortcuts
        </div>
      </div>
    </div>
  );
}
