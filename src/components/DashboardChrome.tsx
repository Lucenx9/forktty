import type { ReactNode } from "react";

interface DashboardChromeProps {
  children: ReactNode;
}

export default function DashboardChrome({ children }: DashboardChromeProps) {
  return (
    <div className="workspace-shell">
      <section className="workspace-stage">{children}</section>
    </div>
  );
}
