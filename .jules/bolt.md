## 2024-06-25 - Prevent 1Hz Sidebar Re-renders
**Learning:** Found a 1-second `setInterval` in `Sidebar.tsx` that triggers a state update (`now`), causing the entire Sidebar and all workspace entries to re-render every second indefinitely just to check for activity, defeating the purpose of keeping activity tracking out of Zustand.
**Action:** Extract the activity check into a local state inside `WorkspaceEntry` that only triggers a re-render when the `hasActivity` boolean actually changes, and only set up intervals where needed.
