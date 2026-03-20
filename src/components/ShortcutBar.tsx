import { memo } from "react";

const ShortcutBar = memo(function ShortcutBar() {
  return (
    <div className="shortcut-bar">
      <span className="shortcut-hint">
        <kbd>Ctrl+D</kbd> Split
      </span>
      <span className="shortcut-hint">
        <kbd>Alt+Arrow</kbd> Navigate
      </span>
      <span className="shortcut-hint">
        <kbd>Ctrl+W</kbd> Close Pane
      </span>
      <span className="shortcut-hint">
        <kbd>Ctrl+F</kbd> Find
      </span>
      <span className="shortcut-hint">
        <kbd>Ctrl+Shift+P</kbd> Palette
      </span>
    </div>
  );
});

export default ShortcutBar;
