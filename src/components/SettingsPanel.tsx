import { useState, useEffect, useCallback } from "react";
import { useConfigStore } from "../stores/config";
import type { AppConfig } from "../lib/pty-bridge";
import { X } from "lucide-react";
import { ConfirmModal } from "./InlineModal";

const MIN_FONT_SIZE = 8;
const MAX_FONT_SIZE = 32;

export default function SettingsPanel() {
  const config = useConfigStore((s) => s.config);
  const saveConfig = useConfigStore((s) => s.saveConfig);
  const toggleSettings = useConfigStore((s) => s.toggleSettings);

  const [draft, setDraft] = useState<AppConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  useEffect(() => {
    if (config) {
      setDraft(structuredClone(config));
      setSaveError(null);
      setConfirmDiscard(false);
    }
  }, [config]);

  const isDirty =
    config !== null && draft !== null && JSON.stringify(draft) !== JSON.stringify(config);
  const fontSizeValid =
    draft !== null &&
    Number.isFinite(draft.appearance.font_size) &&
    draft.appearance.font_size >= MIN_FONT_SIZE &&
    draft.appearance.font_size <= MAX_FONT_SIZE;

  const requestClose = useCallback(() => {
    if (isDirty) {
      setConfirmDiscard(true);
      return;
    }
    toggleSettings();
  }, [isDirty, toggleSettings]);

  useEffect(() => {
    function handleRequestCloseSettings() {
      requestClose();
    }

    window.addEventListener(
      "forktty-request-close-settings",
      handleRequestCloseSettings,
    );
    return () =>
      window.removeEventListener(
        "forktty-request-close-settings",
        handleRequestCloseSettings,
      );
  }, [requestClose]);

  if (!draft) return null;

  function updateGeneral<K extends keyof AppConfig["general"]>(
    key: K,
    value: AppConfig["general"][K],
  ) {
    setSaveError(null);
    setDraft((d) => (d ? { ...d, general: { ...d.general, [key]: value } } : d));
  }

  function updateAppearance<K extends keyof AppConfig["appearance"]>(
    key: K,
    value: AppConfig["appearance"][K],
  ) {
    setSaveError(null);
    setDraft((d) => (d ? { ...d, appearance: { ...d.appearance, [key]: value } } : d));
  }

  function updateNotifications<K extends keyof AppConfig["notifications"]>(
    key: K,
    value: AppConfig["notifications"][K],
  ) {
    setSaveError(null);
    setDraft((d) =>
      d ? { ...d, notifications: { ...d.notifications, [key]: value } } : d,
    );
  }

  async function handleSave() {
    if (!draft) return;
    if (!fontSizeValid) {
      setSaveError(`Font size must be between ${MIN_FONT_SIZE} and ${MAX_FONT_SIZE}.`);
      return;
    }

    setSaving(true);
    setSaveError(null);
    try {
      await saveConfig(draft);
    } catch (err) {
      setSaveError(`Failed to save settings: ${err}`);
    } finally {
      setSaving(false);
    }
  }

  function handleReset() {
    if (config) {
      setDraft(structuredClone(config));
      setSaveError(null);
      setConfirmDiscard(false);
    }
  }

  return (
    <div
      className="settings-panel"
      data-dirty={isDirty ? "true" : "false"}
      role="dialog"
      aria-modal="false"
      aria-labelledby="settings-panel-title"
    >
      <div className="settings-panel-header">
        <div className="settings-panel-heading">
          <span id="settings-panel-title" className="settings-panel-title">
            Settings
          </span>
          <span className="settings-panel-subtitle">
            Appearance, worktree behavior and notifications
          </span>
        </div>
        <div className="settings-panel-actions">
          <span className={`settings-status ${isDirty ? "settings-status-dirty" : ""}`}>
            {isDirty ? "Unsaved changes" : "Up to date"}
          </span>
          <button
            type="button"
            className="settings-secondary-btn"
            onClick={handleReset}
            disabled={saving || !isDirty}
          >
            Reset
          </button>
          <button
            type="button"
            className="settings-save-btn"
            onClick={handleSave}
            disabled={saving || !isDirty || !fontSizeValid}
          >
            {saving ? "Saving..." : "Save"}
          </button>
          <button
            type="button"
            className="settings-close-btn"
            onClick={requestClose}
            aria-label="Close settings"
          >
            <X size={14} />
          </button>
        </div>
      </div>

      <div className="settings-content">
        {saveError && (
          <div className="settings-error" role="alert">
            {saveError}
          </div>
        )}
        {/* General */}
        <div className="settings-section">
          <h3 className="settings-section-title">General</h3>
          <p className="settings-section-description">
            Control shell startup, theme source and where isolated worktrees are
            created.
          </p>

          <label className="settings-field">
            <span className="settings-label">Theme source</span>
            <select
              className="settings-select"
              value={draft.general.theme_source}
              onChange={(e) => updateGeneral("theme_source", e.target.value)}
            >
              <option value="auto">Auto (detect from Ghostty)</option>
              <option value="builtin">Catppuccin Mocha (built-in)</option>
            </select>
          </label>

          <label className="settings-field">
            <span className="settings-label">Shell</span>
            <input
              className="settings-input"
              type="text"
              value={draft.general.shell}
              onChange={(e) => updateGeneral("shell", e.target.value)}
              placeholder="/bin/bash"
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Worktree layout</span>
            <select
              className="settings-select"
              value={draft.general.worktree_layout}
              onChange={(e) => updateGeneral("worktree_layout", e.target.value)}
            >
              <option value="nested">.worktrees/ (nested)</option>
              <option value="sibling">Sibling directories</option>
              <option value="outer-nested">Outer nested</option>
            </select>
            <span className="settings-field-hint">
              Choose whether worktrees live inside the repo or beside it.
            </span>
          </label>

          <label className="settings-field">
            <span className="settings-label">Notification command</span>
            <input
              className="settings-input"
              type="text"
              value={draft.general.notification_command}
              onChange={(e) => updateGeneral("notification_command", e.target.value)}
              placeholder="Optional custom command"
            />
            <span className="settings-field-hint">
              Runs in addition to desktop alerts when a background agent needs input.
            </span>
          </label>
        </div>

        {/* Appearance */}
        <div className="settings-section">
          <h3 className="settings-section-title">Appearance</h3>
          <p className="settings-section-description">
            Tune terminal ergonomics without leaving the current workspace.
          </p>

          <label className="settings-field">
            <span className="settings-label">Font family</span>
            <input
              className="settings-input"
              type="text"
              value={draft.appearance.font_family}
              onChange={(e) => updateAppearance("font_family", e.target.value)}
              placeholder="Blank uses system monospace"
            />
            <span className="settings-field-hint">
              Accepts a single family or a comma-separated fallback list.
            </span>
          </label>

          <label className="settings-field">
            <span className="settings-label">Font size</span>
            <input
              className="settings-input"
              type="number"
              min={8}
              max={32}
              value={draft.appearance.font_size}
              aria-invalid={!fontSizeValid}
              aria-describedby="settings-font-size-hint"
              onChange={(e) => {
                const next = Number(e.target.value);
                if (Number.isFinite(next)) {
                  updateAppearance("font_size", next);
                }
              }}
              onBlur={() => {
                const clamped = Math.max(
                  MIN_FONT_SIZE,
                  Math.min(MAX_FONT_SIZE, draft.appearance.font_size),
                );
                if (clamped !== draft.appearance.font_size) {
                  updateAppearance("font_size", clamped);
                }
              }}
            />
            <span
              id="settings-font-size-hint"
              className={`settings-field-hint ${!fontSizeValid ? "settings-field-error" : ""}`}
            >
              Range {MIN_FONT_SIZE}-{MAX_FONT_SIZE}px.
            </span>
          </label>

          <label className="settings-field">
            <span className="settings-label">Sidebar position</span>
            <select
              className="settings-select"
              value={draft.appearance.sidebar_position}
              onChange={(e) => updateAppearance("sidebar_position", e.target.value)}
            >
              <option value="left">Left</option>
              <option value="right">Right</option>
            </select>
          </label>
        </div>

        {/* Notifications */}
        <div className="settings-section">
          <h3 className="settings-section-title">Notifications</h3>
          <p className="settings-section-description">
            Decide how aggressively ForkTTY pulls your attention back to waiting panes.
          </p>

          <label className="settings-field settings-checkbox-field">
            <input
              className="settings-checkbox"
              type="checkbox"
              checked={draft.notifications.desktop}
              onChange={(e) => updateNotifications("desktop", e.target.checked)}
            />
            <span className="settings-label">Desktop notifications</span>
          </label>

          {/* Sound and idle threshold controls hidden until backend support lands */}
        </div>
      </div>
      {confirmDiscard && (
        <ConfirmModal
          title="Discard Settings Changes"
          message="Close settings and discard unsaved changes?"
          confirmLabel="Discard"
          danger
          onConfirm={() => {
            setConfirmDiscard(false);
            handleReset();
            toggleSettings();
          }}
          onCancel={() => setConfirmDiscard(false)}
        />
      )}
    </div>
  );
}
