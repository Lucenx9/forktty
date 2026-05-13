import { useEffect, useId, useRef, useState } from "react";
import { useOverlayFocus } from "../lib/overlay-focus";

interface ConfirmModalProps {
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmModal({
  title,
  message,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  danger = false,
  onConfirm,
  onCancel,
}: ConfirmModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelButtonRef = useRef<HTMLButtonElement>(null);
  const confirmButtonRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const messageId = useId();
  const hintId = useId();
  // Default focus to Cancel for destructive prompts so Enter does not
  // accidentally confirm a destructive action.
  const focusConfirm = !danger;

  useOverlayFocus({
    containerRef: dialogRef,
    initialFocusRef: focusConfirm ? confirmButtonRef : cancelButtonRef,
    onClose: onCancel,
  });

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        ref={dialogRef}
        className="modal-dialog"
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={`${messageId} ${hintId}`}
        onClick={(e) => e.stopPropagation()}
      >
        <div id={titleId} className="modal-title">
          {title}
        </div>
        <div id={messageId} className="modal-message">
          {message}
        </div>
        <div id={hintId} className="modal-hint">
          Esc to cancel
        </div>
        <div className="modal-actions">
          <button
            ref={cancelButtonRef}
            type="button"
            className="modal-btn modal-btn-cancel"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            ref={confirmButtonRef}
            type="button"
            className={`modal-btn ${danger ? "modal-btn-danger" : "modal-btn-confirm"}`}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

interface PromptModalProps {
  title: string;
  placeholder?: string;
  defaultValue?: string;
  confirmLabel?: string;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}

export function PromptModal({
  title,
  placeholder = "",
  defaultValue = "",
  confirmLabel = "OK",
  onConfirm,
  onCancel,
}: PromptModalProps) {
  const [value, setValue] = useState(defaultValue);
  const inputRef = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const hintId = useId();

  useOverlayFocus({
    containerRef: dialogRef,
    initialFocusRef: inputRef,
    onClose: onCancel,
  });

  useEffect(() => {
    inputRef.current?.select();
  }, []);

  function handleSubmit() {
    const trimmed = value.trim();
    if (trimmed) onConfirm(trimmed);
  }

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        ref={dialogRef}
        className="modal-dialog"
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={hintId}
        onClick={(e) => e.stopPropagation()}
      >
        <div id={titleId} className="modal-title">
          {title}
        </div>
        <input
          ref={inputRef}
          className="modal-input"
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleSubmit();
          }}
          placeholder={placeholder}
        />
        <div id={hintId} className="modal-hint">
          Esc to cancel
        </div>
        <div className="modal-actions">
          <button
            type="button"
            className="modal-btn modal-btn-cancel"
            onClick={onCancel}
          >
            Cancel
          </button>
          {/* Wrapper carries the title so the tooltip remains visible when
              the disabled button can't reliably receive hover/focus events. */}
          <span title={!value.trim() ? "Enter a name to continue" : undefined}>
            <button
              type="button"
              className="modal-btn modal-btn-confirm"
              onClick={handleSubmit}
              disabled={!value.trim()}
            >
              {confirmLabel}
            </button>
          </span>
        </div>
      </div>
    </div>
  );
}
