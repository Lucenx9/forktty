import { useState, useEffect } from "react";
import { useConfigStore } from "../stores/config";
import type { AppConfig } from "../lib/pty-bridge";
import { X } from "lucide-react";

export default function SettingsPanel() {
  const config = useConfigStore((s) => s.config);
  const saveConfig = useConfigStore((s) => s.saveConfig);
  const toggleSettings = useConfigStore((s) => s.toggleSettings);

  const [draft, setDraft] = useState<AppConfig | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (config) {
      setDraft(structuredClone(config));
    }
  }, [config]);

  if (!draft) return null;

  function updateGeneral<K extends keyof AppConfig["general"]>(
    key: K,
    value: AppConfig["general"][K],
  ) {
    setDraft((d) =>
      d ? { ...d, general: { ...d.general, [key]: value } } : d,
    );
  }

  function updateAppearance<K extends keyof AppConfig["appearance"]>(
    key: K,
    value: AppConfig["appearance"][K],
  ) {
    setDraft((d) =>
      d ? { ...d, appearance: { ...d.appearance, [key]: value } } : d,
    );
  }

  function updateNotifications<K extends keyof AppConfig["notifications"]>(
    key: K,
    value: AppConfig["notifications"][K],
  ) {
    setDraft((d) =>
      d ? { ...d, notifications: { ...d.notifications, [key]: value } } : d,
    );
  }

  async function handleSave() {
    if (!draft) return;
    setSaving(true);
    try {
      await saveConfig(draft);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="settings-panel">
      <div className="settings-panel-header">
        <span className="settings-panel-title">Settings</span>
        <div className="settings-panel-actions">
          <button
            className="settings-save-btn"
            onClick={handleSave}
            disabled={saving}
          >
            {saving ? "Saving..." : "Save"}
          </button>
          <button
            className="settings-close-btn"
            onClick={toggleSettings}
            aria-label="Close settings"
          >
            <X size={14} />
          </button>
        </div>
      </div>

      <div className="settings-content">
        {/* General */}
        <div className="settings-section">
          <h3 className="settings-section-title">General</h3>

          <label className="settings-field">
            <span className="settings-label">Theme</span>
            <select
              className="settings-select"
              value={draft.general.theme}
              onChange={(e) => updateGeneral("theme", e.target.value)}
            >
              <option value="ghostty">Ghostty (auto-detect)</option>
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
          </label>

          <label className="settings-field">
            <span className="settings-label">Notification command</span>
            <input
              className="settings-input"
              type="text"
              value={draft.general.notification_command}
              onChange={(e) =>
                updateGeneral("notification_command", e.target.value)
              }
              placeholder="Optional custom command"
            />
          </label>
        </div>

        {/* Appearance */}
        <div className="settings-section">
          <h3 className="settings-section-title">Appearance</h3>

          <label className="settings-field">
            <span className="settings-label">Font family</span>
            <input
              className="settings-input"
              type="text"
              value={draft.appearance.font_family}
              onChange={(e) => updateAppearance("font_family", e.target.value)}
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Font size</span>
            <input
              className="settings-input"
              type="number"
              min={8}
              max={32}
              value={draft.appearance.font_size}
              onChange={(e) =>
                updateAppearance(
                  "font_size",
                  parseInt(e.target.value, 10) || 14,
                )
              }
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Sidebar position</span>
            <select
              className="settings-select"
              value={draft.appearance.sidebar_position}
              onChange={(e) =>
                updateAppearance("sidebar_position", e.target.value)
              }
            >
              <option value="left">Left</option>
              <option value="right">Right</option>
            </select>
          </label>
        </div>

        {/* Notifications */}
        <div className="settings-section">
          <h3 className="settings-section-title">Notifications</h3>

          <label className="settings-field settings-checkbox-field">
            <input
              type="checkbox"
              checked={draft.notifications.desktop}
              onChange={(e) => updateNotifications("desktop", e.target.checked)}
            />
            <span className="settings-label">Desktop notifications</span>
          </label>

          {/* Sound and idle threshold controls hidden until backend support lands */}
        </div>
      </div>
    </div>
  );
}
