import type { ReactNode } from "react";

interface DashboardChromeProps {
  children: ReactNode;
  onCreateWorkspace: () => void;
  onOpenBranchPicker: () => void;
  onOpenCommandPalette: () => void;
  onOpenSettings: () => void;
  onToggleNotifications: () => void;
  onSplitRight: () => void;
  onSplitDown: () => void;
  showNotificationPanel: boolean;
}

export default function DashboardChrome({ children }: DashboardChromeProps) {
  return (
    <div className="workspace-shell">
      <section className="workspace-stage">{children}</section>
    </div>
  );
}
