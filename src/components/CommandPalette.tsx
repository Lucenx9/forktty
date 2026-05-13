import { Fragment, useEffect, useId, useMemo, useRef, useState } from "react";
import { useOverlayFocus } from "../lib/overlay-focus";
import {
  buildHighlightParts,
  findSearchMatch,
  type SearchMatchResult,
} from "../lib/search-match";

interface CommandEntry {
  id: string;
  label: string;
  shortcut?: string;
  group?: string;
  aliases?: string[];
  action: () => void;
}

interface CommandPaletteProps {
  commands: CommandEntry[];
  onClose: () => void;
}

const GROUP_ORDER = ["Workspace", "Pane", "View", "Notifications", "Settings"];

function renderHighlightedText(text: string, match: SearchMatchResult) {
  return buildHighlightParts(text, match.matchedIndices).map((part, index) =>
    part.matched ? (
      <mark key={`${text}-${index}`} className="command-palette-highlight">
        {part.text}
      </mark>
    ) : (
      <Fragment key={`${text}-${index}`}>{part.text}</Fragment>
    ),
  );
}

function renderShortcutKeycaps(shortcut: string) {
  return shortcut.split("+").map((part) => (
    <kbd key={`${shortcut}-${part}`} className="command-palette-keycap">
      {part}
    </kbd>
  ));
}

export default function CommandPalette({ commands, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const paletteRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef<Record<number, HTMLButtonElement | null>>({});
  const listboxId = useId();
  const optionIdPrefix = useId();

  useOverlayFocus({
    containerRef: paletteRef,
    initialFocusRef: inputRef,
    onClose,
  });

  const filtered = useMemo(() => {
    const queryTrimmed = query.trim();
    const groupRank = new Map(GROUP_ORDER.map((group, index) => [group, index]));

    if (!queryTrimmed) {
      return commands
        .map((cmd) => ({
          cmd,
          score: 0,
          labelMatch: { score: 0, matchedIndices: [] } as SearchMatchResult,
          aliasMatch: null as SearchMatchResult | null,
          aliasLabel: "",
          group: cmd.group ?? "Workspace",
        }))
        .sort((a, b) => {
          const groupDelta =
            (groupRank.get(a.group) ?? Number.MAX_SAFE_INTEGER) -
            (groupRank.get(b.group) ?? Number.MAX_SAFE_INTEGER);
          return groupDelta || a.cmd.label.localeCompare(b.cmd.label);
        });
    }

    return commands
      .map((cmd) => {
        const labelMatch = findSearchMatch(cmd.label, queryTrimmed);
        let bestMatch = labelMatch;
        let aliasLabel = "";
        let bestScore = labelMatch?.score ?? Number.MAX_SAFE_INTEGER;

        for (const alias of cmd.aliases ?? []) {
          const aliasMatch = findSearchMatch(alias, queryTrimmed);
          if (!aliasMatch) continue;
          const aliasScore = aliasMatch.score + 2;
          if (aliasScore < bestScore) {
            bestMatch = aliasMatch;
            bestScore = aliasScore;
            aliasLabel = alias;
          }
        }

        if (!bestMatch) return null;

        return {
          cmd,
          score: bestScore,
          labelMatch: labelMatch ?? { score: bestMatch.score, matchedIndices: [] },
          aliasMatch: aliasLabel ? bestMatch : null,
          aliasLabel,
          group: cmd.group ?? "Workspace",
        };
      })
      .filter(
        (
          match,
        ): match is {
          cmd: CommandEntry;
          score: number;
          labelMatch: SearchMatchResult;
          aliasMatch: SearchMatchResult | null;
          aliasLabel: string;
          group: string;
        } => match !== null,
      )
      .sort((a, b) => {
        const groupDelta =
          (groupRank.get(a.group) ?? Number.MAX_SAFE_INTEGER) -
          (groupRank.get(b.group) ?? Number.MAX_SAFE_INTEGER);
        return (
          groupDelta || a.score - b.score || a.cmd.label.localeCompare(b.cmd.label)
        );
      });
  }, [query, commands]);

  const grouped = useMemo(() => {
    const sections = new Map<
      string,
      Array<{
        flatIndex: number;
        cmd: CommandEntry;
        labelMatch: SearchMatchResult;
        aliasMatch: SearchMatchResult | null;
        aliasLabel: string;
      }>
    >();

    filtered.forEach((entry, flatIndex) => {
      const group = entry.group;
      if (!sections.has(group)) {
        sections.set(group, []);
      }
      sections.get(group)!.push({
        flatIndex,
        cmd: entry.cmd,
        labelMatch: entry.labelMatch,
        aliasMatch: entry.aliasMatch,
        aliasLabel: entry.aliasLabel,
      });
    });

    return Array.from(sections.entries());
  }, [filtered]);

  const queryTrimmed = query.trim();
  const resultSummary = queryTrimmed
    ? `${filtered.length} match${filtered.length === 1 ? "" : "es"} in ${grouped.length} group${grouped.length === 1 ? "" : "s"}`
    : `${commands.length} commands`;
  const safeSelectedIndex =
    filtered.length > 0 ? Math.min(selectedIndex, filtered.length - 1) : 0;
  const activeDescendant =
    filtered.length > 0 ? `${optionIdPrefix}-option-${safeSelectedIndex}` : undefined;

  useEffect(() => {
    if (filtered.length === 0) {
      return;
    }
    itemRefs.current[safeSelectedIndex]?.scrollIntoView({ block: "nearest" });
  }, [filtered.length, safeSelectedIndex]);

  function handleKeyDown(e: React.KeyboardEvent) {
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
      const entry = filtered[safeSelectedIndex];
      if (entry) {
        onClose();
        entry.cmd.action();
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
        ref={paletteRef}
        className="command-palette"
        tabIndex={-1}
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
          <span>Alias-aware search</span>
        </div>
        <div id={listboxId} className="command-palette-list" role="listbox">
          {filtered.length === 0 && (
            <div className="command-palette-empty" role="status">
              <div className="command-palette-empty-title">No matching commands</div>
              <div className="command-palette-empty-body">
                {queryTrimmed
                  ? `No commands match "${queryTrimmed}". Try "wt" for worktrees or "notif" for notifications.`
                  : "No commands available."}
              </div>
            </div>
          )}
          {grouped.map(([group, items]) => (
            <div key={group} className="command-palette-group">
              <div className="command-palette-group-title">{group}</div>
              {items.map(({ flatIndex, cmd, labelMatch, aliasMatch, aliasLabel }) => (
                <button
                  key={cmd.id}
                  id={`${optionIdPrefix}-option-${flatIndex}`}
                  ref={(element) => {
                    itemRefs.current[flatIndex] = element;
                  }}
                  type="button"
                  tabIndex={-1}
                  role="option"
                  aria-selected={flatIndex === safeSelectedIndex}
                  aria-current={flatIndex === safeSelectedIndex ? "true" : undefined}
                  className={`command-palette-item ${flatIndex === safeSelectedIndex ? "command-palette-item-selected" : ""}`}
                  onClick={() => {
                    onClose();
                    cmd.action();
                  }}
                  onMouseMove={() => setSelectedIndex(flatIndex)}
                >
                  <div className="command-palette-label-block">
                    <span className="command-palette-label">
                      {renderHighlightedText(cmd.label, labelMatch)}
                    </span>
                    {aliasLabel && (
                      <span className="command-palette-alias">
                        Alias{" "}
                        {renderHighlightedText(
                          aliasLabel,
                          aliasMatch ?? { score: 0, matchedIndices: [] },
                        )}
                      </span>
                    )}
                  </div>
                  {cmd.shortcut && (
                    <span
                      className="command-palette-shortcut"
                      aria-label={cmd.shortcut}
                    >
                      {renderShortcutKeycaps(cmd.shortcut)}
                    </span>
                  )}
                </button>
              ))}
            </div>
          ))}
        </div>
        <div className="command-palette-footer" aria-hidden="true">
          <span>↑↓ Navigate</span>
          <span>Enter Run</span>
          <span>Esc Close</span>
        </div>
      </div>
    </div>
  );
}

export type { CommandEntry };
