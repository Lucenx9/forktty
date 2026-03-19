import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { gitListBranches } from "../lib/pty-bridge";
import type { BranchInfo } from "../lib/pty-bridge";

type BranchPickerResult =
  | { kind: "new-branch"; name: string }
  | { kind: "attach"; branchName: string }
  | { kind: "cancel" };

interface BranchPickerProps {
  onResult: (result: BranchPickerResult) => void;
}

export default function BranchPicker({ onResult }: BranchPickerProps) {
  const [mode, setMode] = useState<"choose" | "new-branch-name">("choose");
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [newBranchName, setNewBranchName] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const isMounted = useRef(true);

  useEffect(() => {
    return () => {
      isMounted.current = false;
    };
  }, []);

  useEffect(() => {
    setLoading(true);
    gitListBranches()
      .then((result) => {
        if (isMounted.current) {
          setBranches(result);
          setLoading(false);
        }
      })
      .catch(() => {
        if (isMounted.current) {
          setBranches([]);
          setLoading(false);
        }
      });
  }, []);

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

  // Reset selection when filter changes.
  useEffect(() => {
    setSelectedIndex(0);
  }, [filtered.length]);

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

  function handleChooseKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      handleCancel();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, totalItems - 1));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      handleSelect(selectedIndex);
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
      <div className="branch-picker-overlay" onClick={handleCancel}>
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
              placeholder="Branch name..."
            />
            <button
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
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="branch-picker-overlay" onClick={handleCancel}>
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
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search branches or create new..."
        />
        <div className="branch-picker-list">
          {loading && (
            <div className="branch-picker-branch-meta">Loading branches...</div>
          )}
          {!loading && (
            <>
              {/* Synthetic "New branch from HEAD..." entry */}
              <div
                className={`branch-picker-item branch-picker-item-new ${selectedIndex === 0 ? "branch-picker-item-selected" : ""}`}
                onClick={() => handleSelect(0)}
                onMouseEnter={() => setSelectedIndex(0)}
              >
                <span className="branch-picker-branch-name">
                  + New branch from HEAD...
                </span>
              </div>

              {filtered.length === 0 && (
                <div className="branch-picker-branch-meta">
                  No matching branches
                </div>
              )}

              {filtered.map((branch, i) => {
                const itemIndex = i + 1;
                const isSelected = selectedIndex === itemIndex;
                return (
                  <div
                    key={branch.name}
                    className={`branch-picker-item ${isSelected ? "branch-picker-item-selected" : ""} ${branch.is_head ? "branch-picker-item-active" : ""}`}
                    onClick={() => handleSelect(itemIndex)}
                    onMouseEnter={() => setSelectedIndex(itemIndex)}
                  >
                    <div>
                      <span className="branch-picker-branch-name">
                        {branch.name}
                      </span>
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
                  </div>
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
