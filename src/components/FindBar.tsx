import { useState, useRef, useEffect } from "react";
import { ChevronDown, ChevronUp, X, CaseSensitive } from "lucide-react";

interface FindBarProps {
  onFind: (term: string, options: { caseSensitive: boolean }) => void;
  onFindNext: () => void;
  onFindPrevious: () => void;
  onClose: () => void;
}

export default function FindBar({
  onFind,
  onFindNext,
  onFindPrevious,
  onClose,
}: FindBarProps) {
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    if (query) {
      onFind(query, { caseSensitive });
    }
  }, [query, caseSensitive, onFind]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) {
        onFindPrevious();
      } else {
        onFindNext();
      }
    }
  }

  return (
    <div className="find-bar" onKeyDown={handleKeyDown}>
      <input
        ref={inputRef}
        className="find-bar-input"
        type="text"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="Find..."
      />
      <button
        className={`find-bar-btn ${caseSensitive ? "find-bar-btn-active" : ""}`}
        onClick={() => setCaseSensitive(!caseSensitive)}
        title="Case sensitive"
        aria-label="Toggle case sensitive search"
        aria-pressed={caseSensitive}
      >
        <CaseSensitive size={14} />
      </button>
      <button
        className="find-bar-btn"
        onClick={onFindPrevious}
        title="Previous (Shift+Enter)"
        aria-label="Find previous match"
      >
        <ChevronUp size={14} />
      </button>
      <button
        className="find-bar-btn"
        onClick={onFindNext}
        title="Next (Enter)"
        aria-label="Find next match"
      >
        <ChevronDown size={14} />
      </button>
      <button
        className="find-bar-btn"
        onClick={onClose}
        title="Close (Esc)"
        aria-label="Close find bar"
      >
        <X size={14} />
      </button>
    </div>
  );
}
