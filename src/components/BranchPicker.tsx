import { ArrowLeft, Check, GitBranch, Plus } from "lucide-react";
import {
  useState,
  useEffect,
  useRef,
  useMemo,
  useCallback,
  useId,
  Fragment,
} from "react";
import { gitListBranches } from "../lib/pty-bridge";
import type { BranchInfo } from "../lib/pty-bridge";
import { useOverlayFocus } from "../lib/overlay-focus";
import { buildHighlightParts, findSearchMatch } from "../lib/search-match";
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
  const [loadError, setLoadError] = useState("");
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [newBranchName, setNewBranchName] = useState("");
  const dialogRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef<Record<number, HTMLButtonElement | null>>({});
  const listboxId = useId();
  const optionIdPrefix = useId();
  const queryTrimmed = query.trim();

  useEffect(() => {
    let cancelled = false;

    gitListBranches(cwd)
      .then((result) => {
        if (!cancelled) {
          setBranches(result);
          setLoadedCwd(requestCwd);
          setLoadError("");
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setBranches([]);
          setLoadedCwd(requestCwd);
          setLoadError(String(err));
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

  function renderHighlightedText(text: string, queryValue: string) {
    const match = findSearchMatch(text, queryValue) ?? { score: 0, matchedIndices: [] };
    return buildHighlightParts(text, match.matchedIndices).map((part, index) =>
      part.matched ? (
        <mark key={`${text}-${index}`} className="branch-picker-highlight">
          {part.text}
        </mark>
      ) : (
        <Fragment key={`${text}-${index}`}>{part.text}</Fragment>
      ),
    );
  }

  const filtered = useMemo(() => {
    if (!queryTrimmed) {
      return branches;
    }

    return branches
      .map((branch) => {
        const nameMatch = findSearchMatch(branch.name, queryTrimmed);
        const summaryMatch = findSearchMatch(branch.last_commit_summary, queryTrimmed);
        if (!nameMatch && !summaryMatch) return null;
        return branch;
      })
      .filter((item): item is BranchInfo => item !== null);
  }, [queryTrimmed, branches]);
  const loading = loadedCwd !== requestCwd;

  const handleCancel = useCallback(() => {
    onResult({ kind: "cancel" });
  }, [onResult]);

  useOverlayFocus({
    containerRef: dialogRef,
    initialFocusRef: mode === "choose" ? inputRef : nameInputRef,
    onClose: handleCancel,
    closeOnEscape: mode === "choose",
  });

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
  const activeDescendant = `${optionIdPrefix}-option-${safeSelectedIndex}`;

  useEffect(() => {
    if (mode !== "choose" || loading) {
      return;
    }
    itemRefs.current[safeSelectedIndex]?.scrollIntoView({ block: "nearest" });
  }, [loading, mode, safeSelectedIndex]);

  function handleChooseKeyDown(e: React.KeyboardEvent) {
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
          ref={dialogRef}
          className="branch-picker"
          tabIndex={-1}
          onClick={(e) => e.stopPropagation()}
          onKeyDown={handleNameKeyDown}
        >
          <div className="branch-picker-header">
            <button
              type="button"
              className="branch-picker-back-btn"
              onClick={() => setMode("choose")}
              aria-label="Back to branch selection"
            >
              <ArrowLeft size={14} />
            </button>
            <div>
              <div className="branch-picker-header-title">New branch from HEAD</div>
              <div className="branch-picker-header-subtitle">
                Create a new worktree-backed branch from the current checkout.
              </div>
            </div>
          </div>
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
        ref={dialogRef}
        className="branch-picker"
        tabIndex={-1}
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
          <button
            id={`${optionIdPrefix}-option-0`}
            ref={(element) => {
              itemRefs.current[0] = element;
            }}
            type="button"
            tabIndex={-1}
            role="option"
            aria-selected={safeSelectedIndex === 0}
            aria-current={safeSelectedIndex === 0 ? "true" : undefined}
            className={`branch-picker-action ${safeSelectedIndex === 0 ? "branch-picker-action-selected" : ""}`}
            onMouseDown={(e) => e.preventDefault()}
            onMouseMove={() => setSelectedIndex(0)}
            onClick={() => handleSelect(0)}
          >
            <span className="branch-picker-action-icon" aria-hidden="true">
              <Plus size={15} />
            </span>
            <span className="branch-picker-action-copy">
              <span className="branch-picker-action-title">
                {queryTrimmed
                  ? `Create "${queryTrimmed}" from HEAD`
                  : "Create new branch from HEAD"}
              </span>
              <span className="branch-picker-action-body">
                Start an isolated worktree without leaving the picker.
              </span>
            </span>
          </button>
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
              {filtered.length === 0 && (
                <div className="branch-picker-empty" role="status">
                  <div className="branch-picker-empty-title">
                    {loadError
                      ? "Could not inspect branches"
                      : branches.length === 0
                        ? "No repository branches available"
                        : "No matching branches"}
                  </div>
                  <div className="branch-picker-empty-body">
                    {loadError
                      ? loadError
                      : branches.length === 0
                        ? "Open a terminal in a Git repository to create or attach a worktree from this picker."
                        : `No branches match "${query.trim()}".`}
                  </div>
                </div>
              )}

              {filtered.map((branch, i) => {
                const itemIndex = i + 1;
                const isSelected = safeSelectedIndex === itemIndex;
                return (
                  <button
                    key={branch.name}
                    id={`${optionIdPrefix}-option-${itemIndex}`}
                    ref={(element) => {
                      itemRefs.current[itemIndex] = element;
                    }}
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
                    <div className="branch-picker-branch-header">
                      <span className="branch-picker-branch-icon" aria-hidden="true">
                        {branch.is_head ? <Check size={14} /> : <GitBranch size={14} />}
                      </span>
                      <span className="branch-picker-branch-name">
                        {renderHighlightedText(branch.name, queryTrimmed)}
                      </span>
                      {branch.is_head && (
                        <span className="branch-picker-badge">Checked out</span>
                      )}
                    </div>
                    <div className="branch-picker-branch-meta">
                      {renderHighlightedText(branch.last_commit_summary, queryTrimmed)}
                      {branch.last_commit_time > 0 && (
                        <span className="branch-picker-branch-time">
                          {formatTime(branch.last_commit_time)}
                        </span>
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
