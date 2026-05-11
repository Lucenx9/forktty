import { useState, useEffect, useRef, useMemo, useId } from "react";

interface CommandEntry {
  id: string;
  label: string;
  shortcut?: string;
  action: () => void;
}

interface CommandPaletteProps {
  commands: CommandEntry[];
  onClose: () => void;
}

export default function CommandPalette({ commands, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const listboxId = useId();
  const optionIdPrefix = useId();

  useEffect(() => {
    previousFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    inputRef.current?.focus();

    return () => {
      if (previousFocusRef.current && document.contains(previousFocusRef.current)) {
        previousFocusRef.current.focus();
      }
    };
  }, []);

  const filtered = useMemo(() => {
    if (!query.trim()) return commands;
    const needle = query.toLowerCase().replace(/\s+/g, "");
    return commands
      .map((cmd) => {
        const label = cmd.label.toLowerCase();
        const compact = label.replace(/\s+/g, "");
        let cursor = 0;
        let score = 0;

        for (const char of needle) {
          const index = compact.indexOf(char, cursor);
          if (index === -1) return null;
          score += index === cursor ? 0 : index - cursor + 1;
          cursor = index + 1;
        }

        if (label.includes(query.toLowerCase())) {
          score -= 8;
        }

        return { cmd, score };
      })
      .filter((match): match is { cmd: CommandEntry; score: number } => match !== null)
      .sort((a, b) => a.score - b.score || a.cmd.label.localeCompare(b.cmd.label))
      .map((match) => match.cmd);
  }, [query, commands]);

  const queryTrimmed = query.trim();
  const resultSummary = queryTrimmed
    ? `${filtered.length} match${filtered.length === 1 ? "" : "es"}`
    : `${commands.length} commands`;
  const safeSelectedIndex =
    filtered.length > 0 ? Math.min(selectedIndex, filtered.length - 1) : 0;
  const activeDescendant =
    filtered.length > 0 ? `${optionIdPrefix}-option-${safeSelectedIndex}` : undefined;

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (filtered.length > 0) {
        setSelectedIndex(Math.min(safeSelectedIndex + 1, filtered.length - 1));
      }
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex(Math.max(safeSelectedIndex - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const cmd = filtered[safeSelectedIndex];
      if (cmd) {
        onClose();
        cmd.action();
      }
      return;
    }
  }

  return (
    <div
      className="command-palette-overlay"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
    >
      <div
        className="command-palette"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <input
          ref={inputRef}
          className="command-palette-input"
          type="text"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setSelectedIndex(0);
          }}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded="true"
          aria-controls={listboxId}
          aria-activedescendant={activeDescendant}
          aria-label="Search commands"
          placeholder="Type a command..."
        />
        <div className="command-palette-meta">
          <span>{resultSummary}</span>
          <span>Enter to run</span>
        </div>
        <div id={listboxId} className="command-palette-list" role="listbox">
          {filtered.length === 0 && (
            <div className="command-palette-empty" role="status">
              {queryTrimmed
                ? `No commands match "${queryTrimmed}".`
                : "No commands available."}
            </div>
          )}
          {filtered.map((cmd, i) => (
            <button
              key={cmd.id}
              id={`${optionIdPrefix}-option-${i}`}
              type="button"
              tabIndex={-1}
              role="option"
              aria-selected={i === safeSelectedIndex}
              aria-current={i === safeSelectedIndex ? "true" : undefined}
              className={`command-palette-item ${i === safeSelectedIndex ? "command-palette-item-selected" : ""}`}
              onClick={() => {
                onClose();
                cmd.action();
              }}
              onMouseMove={() => setSelectedIndex(i)}
            >
              <span className="command-palette-label">{cmd.label}</span>
              {cmd.shortcut && (
                <span className="command-palette-shortcut">{cmd.shortcut}</span>
              )}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

export type { CommandEntry };
