import {
  ArrowRight,
  Columns2,
  Command,
  GitBranch,
  Plus,
  Settings,
  Terminal,
} from "lucide-react";
import { useId, useRef } from "react";
import { useOverlayFocus } from "../lib/overlay-focus";

interface WelcomeScreenProps {
  onDismiss: () => void;
}

export default function WelcomeScreen({ onDismiss }: WelcomeScreenProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const dismissBtnRef = useRef<HTMLButtonElement>(null);

  useOverlayFocus({
    containerRef: dialogRef,
    initialFocusRef: dismissBtnRef,
    onClose: onDismiss,
  });

  const actions = [
    {
      icon: Plus,
      title: "New workspace",
      shortcut: "Ctrl+N",
      body: "Spin up another workspace without leaving the current flow.",
    },
    {
      icon: GitBranch,
      title: "Create worktree",
      shortcut: "Ctrl+Shift+N",
      body: "Branch into an isolated worktree when changes need full git separation.",
    },
    {
      icon: Columns2,
      title: "Split the active pane",
      shortcut: "Ctrl+D",
      body: "Keep build, test and agent output visible side by side.",
    },
    {
      icon: Command,
      title: "Open the command palette",
      shortcut: "Ctrl+Shift+P",
      body: "Jump to every workspace, pane and settings action from one place.",
    },
    {
      icon: Settings,
      title: "Adjust defaults",
      shortcut: "Ctrl+,",
      body: "Tune fonts, notifications and worktree behavior before the session gets busy.",
    },
  ] as const;

  function openCommandPalette() {
    onDismiss();
    window.dispatchEvent(new CustomEvent("forktty-open-command-palette"));
  }

  function openBranchPicker() {
    onDismiss();
    window.dispatchEvent(new CustomEvent("forktty-open-branch-picker"));
  }

  return (
    <div className="welcome-overlay" onClick={onDismiss}>
      <div
        ref={dialogRef}
        className="welcome-dialog"
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="welcome-header">
          <div className="welcome-eyebrow">
            <span className="welcome-eyebrow-icon" aria-hidden="true">
              <Terminal size={14} />
            </span>
            Ready for parallel terminal work
          </div>
          <div id={titleId} className="welcome-title">
            Welcome to ForkTTY
          </div>
          <div className="welcome-subtitle">
            Parallel terminals with isolated worktrees and fast recovery
          </div>
        </div>
        <div className="welcome-copy">
          One workspace is already ready. Start with a quick split for immediate
          parallelism, or launch a dedicated worktree when a task deserves isolated git
          history.
        </div>

        <div className="welcome-actions">
          {actions.map(({ icon: Icon, title, shortcut, body }) => (
            <div key={title} className="welcome-action">
              <span className="welcome-action-icon" aria-hidden="true">
                <Icon size={16} />
              </span>
              <div className="welcome-action-copy">
                <div className="welcome-action-title-row">
                  <span className="welcome-action-title">{title}</span>
                  <span className="welcome-shortcut">{shortcut}</span>
                </div>
                <span>{body}</span>
              </div>
            </div>
          ))}
        </div>

        <div className="welcome-footer">
          <div className="welcome-footer-copy">
            <div className="welcome-hint">Start with a high-signal next action.</div>
            <div className="welcome-footer-actions">
              <button
                type="button"
                className="welcome-cta welcome-cta-primary"
                onClick={openBranchPicker}
              >
                Create worktree
                <ArrowRight size={14} />
              </button>
              <button
                type="button"
                className="welcome-cta welcome-cta-secondary"
                onClick={openCommandPalette}
              >
                Open palette
              </button>
            </div>
          </div>
          <button
            ref={dismissBtnRef}
            type="button"
            className="welcome-dismiss"
            onClick={onDismiss}
          >
            Get started
          </button>
        </div>
      </div>
    </div>
  );
}
