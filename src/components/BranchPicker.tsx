import { useState, useEffect, useRef, useMemo, useCallback, useId } from "react";
import { gitListBranches } from "../lib/pty-bridge";
import type { BranchInfo } from "../lib/pty-bridge";
import { showToast } from "./ErrorToast";

type BranchPickerResult =
  | { kind: "new-branch"; name: string }
  | { kind: "attach"; branchName: string }
  | { kind: "cancel" };

interface BranchPickerProps {
  cwd?: string;
  onResult: (result: BranchPickerResult) => void;
}

export default function BranchPicker({ cwd, onResult }: BranchPickerProps) {
  const requestCwd = cwd ?? null;
  const [mode, setMode] = useState<"choose" | "new-branch-name">("choose");
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [loadedCwd, setLoadedCwd] = useState<string | null | undefined>(undefined);
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [newBranchName, setNewBranchName] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const listboxId = useId();
  const optionIdPrefix = useId();

  useEffect(() => {
    previousFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    return () => {
      if (previousFocusRef.current && document.contains(previousFocusRef.current)) {
        previousFocusRef.current.focus();
      }
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    gitListBranches(cwd)
      .then((result) => {
        if (!cancelled) {
          setBranches(result);
          setLoadedCwd(requestCwd);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setBranches([]);
          setLoadedCwd(requestCwd);
          showToast(`Failed to load branches: ${err}`, "error");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [cwd, requestCwd]);

  useEffect(() => {
    if (mode === "choose") {
      inputRef.current?.focus();
    } else {
      nameInputRef.current?.focus();
    }
  }, [mode]);

  // Filtered list: synthetic "New branch" entry at index 0, then matching branches.
  const filtered = useMemo(() => {
    if (!query.trim()) return branches;
    const lower = query.toLowerCase();
    return branches.filter((b) => b.name.toLowerCase().includes(lower));
  }, [query, branches]);
  const loading = loadedCwd !== requestCwd;

  const handleCancel = useCallback(() => {
    onResult({ kind: "cancel" });
  }, [onResult]);

  function handleSelect(index: number) {
    if (index === 0) {
      // "New branch from HEAD..."
      setMode("new-branch-name");
      setNewBranchName(query.trim());
      return;
    }
    const branch = filtered[index - 1];
    if (!branch || branch.is_head) return;
    onResult({ kind: "attach", branchName: branch.name });
  }

  // Total items = 1 (synthetic) + filtered.length
  const totalItems = 1 + filtered.length;
  const safeSelectedIndex = Math.min(selectedIndex, totalItems - 1);
  const activeDescendant = loading
    ? undefined
    : `${optionIdPrefix}-option-${safeSelectedIndex}`;

  function handleChooseKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      handleCancel();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex(Math.min(safeSelectedIndex + 1, totalItems - 1));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex(Math.max(safeSelectedIndex - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      handleSelect(safeSelectedIndex);
      return;
    }
  }

  function handleNameKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      setMode("choose");
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const trimmed = newBranchName.trim();
      if (trimmed) {
        onResult({ kind: "new-branch", name: trimmed });
      }
      return;
    }
  }

  function formatTime(epochSeconds: number): string {
    if (epochSeconds === 0) return "";
    const date = new Date(epochSeconds * 1000);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));
    if (diffDays === 0) return "today";
    if (diffDays === 1) return "yesterday";
    if (diffDays < 30) return `${diffDays}d ago`;
    if (diffDays < 365) return `${Math.floor(diffDays / 30)}mo ago`;
    return `${Math.floor(diffDays / 365)}y ago`;
  }

  if (mode === "new-branch-name") {
    return (
      <div
        className="branch-picker-overlay"
        onClick={handleCancel}
        role="dialog"
        aria-modal="true"
        aria-label="Create branch"
      >
        <div
          className="branch-picker"
          onClick={(e) => e.stopPropagation()}
          onKeyDown={handleNameKeyDown}
        >
          <div className="branch-picker-header">New branch from HEAD</div>
          <div className="branch-picker-name-form">
            <input
              ref={nameInputRef}
              className="branch-picker-input"
              type="text"
              value={newBranchName}
              onChange={(e) => setNewBranchName(e.target.value)}
              aria-describedby="branch-picker-create-hint"
              aria-label="Branch name"
              placeholder="Branch name..."
            />
            {/* Wrapper carries the title so the tooltip remains visible when
                the disabled button can't receive hover/focus reliably. */}
            <span
              title={
                !newBranchName.trim() ? "Enter a branch name to continue" : undefined
              }
            >
              <button
                type="button"
                className="branch-picker-confirm-btn"
                onClick={() => {
                  const trimmed = newBranchName.trim();
                  if (trimmed) {
                    onResult({ kind: "new-branch", name: trimmed });
                  }
                }}
                disabled={!newBranchName.trim()}
              >
                Create
              </button>
            </span>
          </div>
          <div id="branch-picker-create-hint" className="branch-picker-hint">
            Escape returns to branch selection.
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      className="branch-picker-overlay"
      onClick={handleCancel}
      role="dialog"
      aria-modal="true"
      aria-label="Choose branch"
    >
      <div
        className="branch-picker"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleChooseKeyDown}
      >
        <input
          ref={inputRef}
          className="branch-picker-input"
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
          aria-label="Search branches"
          placeholder="Search branches or create new..."
        />
        <div id={listboxId} className="branch-picker-list" role="listbox">
          {loading && (
            <div
              className="branch-picker-branch-meta branch-picker-loading"
              role="status"
              aria-live="polite"
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                gap: 10,
              }}
            >
              <span className="app-loading-spinner" aria-hidden="true" />
              <span>Loading branches…</span>
            </div>
          )}
          {!loading && (
            <>
              {/* Synthetic "New branch from HEAD..." entry */}
              <button
                id={`${optionIdPrefix}-option-0`}
                type="button"
                tabIndex={-1}
                role="option"
                aria-selected={safeSelectedIndex === 0}
                className={`branch-picker-item branch-picker-item-new ${safeSelectedIndex === 0 ? "branch-picker-item-selected" : ""}`}
                aria-current={safeSelectedIndex === 0 ? "true" : undefined}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => handleSelect(0)}
                onMouseMove={() => setSelectedIndex(0)}
              >
                <span className="branch-picker-branch-name">
                  + New branch from HEAD...
                </span>
              </button>

              {filtered.length === 0 && (
                <div className="branch-picker-branch-meta" role="status">
                  {branches.length === 0
                    ? "No branches found in this repository."
                    : query.trim()
                      ? `No branches match "${query.trim()}".`
                      : "No matching branches."}
                </div>
              )}

              {filtered.map((branch, i) => {
                const itemIndex = i + 1;
                const isSelected = safeSelectedIndex === itemIndex;
                return (
                  <button
                    key={branch.name}
                    id={`${optionIdPrefix}-option-${itemIndex}`}
                    type="button"
                    tabIndex={-1}
                    role="option"
                    aria-selected={isSelected}
                    className={`branch-picker-item ${isSelected ? "branch-picker-item-selected" : ""} ${branch.is_head ? "branch-picker-item-active" : ""}`}
                    aria-current={isSelected ? "true" : undefined}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => handleSelect(itemIndex)}
                    onMouseMove={() => setSelectedIndex(itemIndex)}
                    disabled={branch.is_head}
                  >
                    <div>
                      <span className="branch-picker-branch-name">{branch.name}</span>
                      {branch.is_head && (
                        <span className="branch-picker-badge">[active]</span>
                      )}
                    </div>
                    <div className="branch-picker-branch-meta">
                      {branch.last_commit_summary}
                      {branch.last_commit_time > 0 && (
                        <span> -- {formatTime(branch.last_commit_time)}</span>
                      )}
                    </div>
                  </button>
                );
              })}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export type { BranchPickerResult };
