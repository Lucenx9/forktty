//! Focused-terminal actions and focus synchronization for `TerminalController`.

use super::*;

impl TerminalController {
    pub(in crate::gtk_app) fn zoom_terminal_in(&mut self) {
        self.set_terminal_zoom_level(next_terminal_zoom_level(self.terminal_zoom_level.get(), 1));
    }

    pub(in crate::gtk_app) fn zoom_terminal_out(&mut self) {
        self.set_terminal_zoom_level(next_terminal_zoom_level(self.terminal_zoom_level.get(), -1));
    }

    pub(in crate::gtk_app) fn reset_terminal_zoom(&mut self) {
        self.set_terminal_zoom_level(0);
    }

    fn set_terminal_zoom_level(&mut self, zoom_level: i32) {
        let zoom_level = next_terminal_zoom_level(zoom_level, 0);
        let previous_zoom_level = self.terminal_zoom_level.replace(zoom_level);
        if previous_zoom_level == zoom_level {
            return;
        }
        for widget in self.widgets.values() {
            widget.set_zoom_level(zoom_level);
        }
        let embedded_action = if zoom_level == 0 {
            EmbeddedSurfaceAction::ResetFontSize
        } else if zoom_level > previous_zoom_level {
            EmbeddedSurfaceAction::IncreaseFontSize
        } else {
            EmbeddedSurfaceAction::DecreaseFontSize
        };
        for pane in self.embedded_ghostty_panes.values() {
            let _ = self.perform_embedded_action(&pane.surface, embedded_action);
        }
    }

    fn model_focused_widget(&self) -> Option<GhosttyTerminalWidget> {
        let surface_id = {
            let model = self.model.lock().ok()?;
            model.active_workspace()?.focused_surface_id
        };
        self.widgets.get(&surface_id).cloned()
    }

    fn gtk_focused_widget(&self) -> Option<GhosttyTerminalWidget> {
        self.widgets
            .values()
            .find(|widget| widget.has_terminal_focus())
            .cloned()
    }

    /// True when GTK focus currently lives inside `widget` (an embedded Ghostty
    /// surface focuses its internal GL area, never the wrapper widget itself).
    fn embedded_widget_has_focus(&self, widget: &gtk::Widget) -> bool {
        let Some(focus) = gtk::prelude::GtkWindowExt::focus(&self.parent_window) else {
            return false;
        };
        &focus == widget || focus.is_ancestor(widget)
    }

    /// The embedded Ghostty surface that owns GTK focus, mirroring
    /// `gtk_focused_widget` for the embedded-pane path.
    fn focused_embedded_pane(&self) -> Option<gtk::Widget> {
        self.embedded_ghostty_panes
            .values()
            .find(|pane| self.embedded_widget_has_focus(&pane.surface))
            .map(|pane| pane.surface.clone())
    }

    /// The embedded Ghostty surface the model considers focused; used by the
    /// command palette, which owns GTK focus itself while open.
    fn model_focused_embedded_pane(&self) -> Option<gtk::Widget> {
        let surface_id = {
            let model = self.model.lock().ok()?;
            model.active_workspace()?.focused_surface_id
        };
        self.embedded_ghostty_panes
            .get(&surface_id)
            .map(|pane| pane.surface.clone())
    }

    /// Routes a clipboard/search action to the embedded Ghostty surface via its
    /// keybinding ABI. Returns `true` once routed (the accelerator is consumed)
    /// even if Ghostty reports the action as a no-op (e.g. copy with no
    /// selection); `false` only when the embedding library lacks the symbol.
    fn perform_embedded_action(&self, widget: &gtk::Widget, action: EmbeddedSurfaceAction) -> bool {
        let Some(embedder) = self.embedded_ghostty.as_ref() else {
            return false;
        };
        match unsafe { embedder.perform_action(widget, action) } {
            Ok(_) => true,
            Err(err) => {
                eprintln!(
                    "forktty: embedded Ghostty {} unavailable: {err}",
                    action.as_ghostty_action()
                );
                false
            }
        }
    }

    // App-wide clipboard accelerators must only affect a terminal that currently
    // owns GTK focus; the model focus can legitimately be stale while dialogs or
    // search entries are active.
    pub(in crate::gtk_app) fn copy_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Copy);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.copy_text();
        true
    }

    pub(in crate::gtk_app) fn paste_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Paste);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.paste_from_clipboard();
        true
    }

    pub(in crate::gtk_app) fn select_all_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::SelectAll);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.select_all_text();
        true
    }

    pub(in crate::gtk_app) fn reset_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::ClearScreen);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.reset_and_clear();
        true
    }

    /// The last non-empty line of `surface_id`'s terminal output plus the
    /// content generation it was read at. `known` short-circuits the bounded
    /// tail read when the generation has not moved; the agent HUD polls this
    /// once per second per agent.
    pub(in crate::gtk_app) fn surface_tail_line(
        &self,
        surface_id: &str,
        known: Option<&AgentTailEntry>,
    ) -> Option<AgentTailEntry> {
        if let Some(widget) = self.widgets.get(surface_id) {
            let generation = widget.content_generation();
            if let Some((known_generation, line)) = known {
                if *known_generation == generation {
                    return Some((generation, line.clone()));
                }
            }
            let snapshot = widget
                .read_text(surface_id, TerminalTextCapture::Tail { lines: 8 }, 4096)
                .ok()?;
            return Some((generation, last_nonempty_line(&snapshot.text)));
        }
        let pane = self.embedded_ghostty_panes.get(surface_id)?;
        let embedder = self.embedded_ghostty.as_ref()?;
        if !embedder.supports_read_text() {
            return None;
        }
        let generation = embedded_agent_tail_generation(known);
        let snapshot = unsafe {
            embedder
                .read_text_snapshot(
                    &pane.surface,
                    surface_id,
                    TerminalTextCapture::Tail { lines: 8 },
                    4096,
                )
                .ok()?
        };
        Some((generation, last_nonempty_line(&snapshot.text)))
    }

    /// Writes `text` to `surface_id`'s terminal as if typed (the caller adds
    /// any trailing `\r`). `false` when the pane has no live widget.
    pub(in crate::gtk_app) fn send_text_to_surface(&self, surface_id: &str, text: &str) -> bool {
        if let Some(widget) = self.widgets.get(surface_id) {
            widget.send_text(text);
            return true;
        }
        let Some(pane) = self.embedded_ghostty_panes.get(surface_id) else {
            return false;
        };
        let Some(embedder) = self.embedded_ghostty.as_ref() else {
            return false;
        };
        match unsafe { embedder.send_text(&pane.surface, text) } {
            Ok(()) => true,
            Err(err) => {
                eprintln!(
                    "Failed to send text to embedded Ghostty GTK surface {surface_id}: {err}"
                );
                false
            }
        }
    }

    /// Reveals the floating search bar of the focused terminal pane. The pane
    /// holding GTK focus wins; the model focus is the fallback so the command
    /// palette (which owns GTK focus itself) can open search too.
    pub(in crate::gtk_app) fn open_search_in_focused_pane(&self) -> bool {
        let surface_id = self
            .widgets
            .iter()
            .find(|(_, widget)| widget.has_terminal_focus())
            .map(|(surface_id, _)| surface_id.clone())
            .or_else(|| {
                self.embedded_ghostty_panes
                    .iter()
                    .find(|(_, pane)| self.embedded_widget_has_focus(&pane.surface))
                    .map(|(surface_id, _)| surface_id.clone())
            })
            .or_else(|| {
                let model = self.model.lock().ok()?;
                Some(model.active_workspace()?.focused_surface_id)
            });
        let Some(surface_id) = surface_id else {
            return false;
        };
        // Embedded panes have no ForkTTY search bar; open Ghostty's own overlay.
        if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
            return self.perform_embedded_action(&pane.surface, EmbeddedSurfaceAction::StartSearch);
        }
        let Some(chrome) = self.chromes.get(&surface_id) else {
            return false;
        };
        if !chrome.search_supported {
            return false;
        }
        chrome.search_bar.container.set_visible(true);
        chrome.search_bar.entry.grab_focus();
        true
    }

    // Explicit commands from the command palette intentionally target the active
    // terminal, because the palette itself owns GTK focus while the user chooses.
    pub(in crate::gtk_app) fn copy_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Copy);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.copy_text();
        true
    }

    pub(in crate::gtk_app) fn paste_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Paste);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.paste_from_clipboard();
        true
    }

    pub(in crate::gtk_app) fn select_all_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::SelectAll);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.select_all_text();
        true
    }

    pub(in crate::gtk_app) fn reset_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::ClearScreen);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.reset_and_clear();
        true
    }

    pub(in crate::gtk_app) fn send_model_focused_navigation_input(
        &self,
        input: TerminalInput,
    ) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        forward_terminal_navigation_input(&widget, input);
        true
    }

    pub(super) fn set_terminal_sibling_flags(
        &self,
        visible_surface_ids: &BTreeSet<String>,
        has_siblings: bool,
    ) {
        for (surface_id, widget) in &self.widgets {
            widget.set_has_siblings(has_siblings && visible_surface_ids.contains(surface_id));
        }
    }

    /// Pushes the model's focused surface into each terminal widget so the
    /// renderer never dims the logically active pane while GTK focus sits in
    /// a non-terminal widget (the pane's search entry, the command palette,
    /// dialogs). Runs from `ensure_layout_current`:
    /// focus-only changes don't alter the layout signature, so the sibling
    /// flag path alone would miss them.
    fn sync_terminal_model_focus_flags(&self) {
        let focused_surface_id = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id);
        for (surface_id, widget) in &self.widgets {
            widget.set_is_model_focused(focused_surface_id.as_deref() == Some(surface_id.as_str()));
        }
    }

    pub(in crate::gtk_app) fn sync_model_focus_to_ui(&mut self) {
        self.ensure_layout_current();
        if let Some(surface_id) = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id)
        {
            self.queue_focus_for_surface(&surface_id);
        }
    }

    pub(in crate::gtk_app) fn ensure_layout_current(&mut self) {
        self.spawn_active_surfaces_if_needed();
        let Some((signature, _, focused_surface_id, _)) = active_layout_snapshot(&self.model)
        else {
            if self.last_layout_signature.as_deref() != Some(EMPTY_LAYOUT_SIGNATURE) {
                self.rebuild_layout();
            }
            self.sync_terminal_model_focus_flags();
            return;
        };
        let signature =
            effective_layout_signature(&signature, self.maximized_pane, &focused_surface_id);
        if self.last_layout_signature.as_deref() != Some(signature.as_str()) {
            self.rebuild_layout();
        } else {
            self.refresh_chromes();
        }
        self.sync_terminal_model_focus_flags();
    }
}
