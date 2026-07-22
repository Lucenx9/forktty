//! GTK workspace controller that keeps the model, pane widgets, and socket state in sync.

mod embedded_spawn;
mod focused_terminal;

use super::*;

pub(super) struct PaneChrome {
    pub(super) pane: gtk::Box,
    pub(super) header_revealer: gtk::Revealer,
    pub(super) single_pane_actions: gtk::Box,
    pub(super) title: gtk::Label,
    pub(super) agent_badge: gtk::Label,
    pub(super) cwd: gtk::Label,
    pub(super) attention_dot: gtk::Box,
    pub(super) search_bar: PaneSearchBar,
    pub(super) search_supported: bool,
}

pub(super) type SplitResizeCallback = Rc<dyn Fn(&[String], &[String], f64)>;
pub(super) type SettingsApplyCallback = Rc<dyn Fn(&config::AppConfig)>;

pub(super) struct TerminalController {
    container: gtk::Box,
    parent_window: adw::ApplicationWindow,
    pub(super) model: Arc<Mutex<WorkspaceModel>>,
    /// Concrete twin of `SocketAppState::terminal`, retained for private
    /// generation checks that are intentionally absent from the public trait.
    backend: Arc<GtkTerminalBackend>,
    state: Option<SocketAppState>,
    toast_handle: Option<ToastHandle>,
    pub(super) widgets: BTreeMap<String, GhosttyTerminalWidget>,
    embedded_ghostty_panes: BTreeMap<String, EmbeddedGhosttyPane>,
    embedded_ghostty: Option<Rc<GhosttyGtkEmbedder>>,
    chromes: BTreeMap<String, PaneChrome>,
    pending_spawns: BTreeMap<String, PendingSpawn>,
    last_layout_signature: Option<String>,
    last_chrome_signature: Option<String>,
    maximized_pane: bool,
    /// Spawned child PID per surface, used to discover listening ports.
    pub(super) surface_pids: Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    pane_tab_strips: Rc<RefCell<Vec<PaneTabStrip>>>,
    terminal_zoom_level: Rc<Cell<i32>>,
    #[cfg(feature = "browser")]
    browser_panes: Rc<RefCell<BTreeMap<String, Rc<crate::browser_pane::BrowserPaneWidget>>>>,
}

#[derive(Clone)]
pub(super) struct PaneTabStrip {
    tabs: Vec<String>,
    scroller: gtk::ScrolledWindow,
    tabstrip: gtk::Box,
    stack: gtk::Stack,
    tab_widgets: Vec<gtk::Box>,
    labels: Vec<gtk::Label>,
    select_areas: Vec<gtk::Button>,
}

fn real_pane_count(pane_tree: Option<&PaneNode>) -> usize {
    pane_tree.map(collect_panes).map_or(0, |panes| panes.len())
}

pub(super) fn normalize_maximized_pane_state(
    maximized: bool,
    pane_tree: Option<&PaneNode>,
) -> bool {
    maximized && real_pane_count(pane_tree) > 1
}

pub(super) fn toggle_maximized_pane_state(maximized: bool, pane_tree: Option<&PaneNode>) -> bool {
    if maximized {
        false
    } else {
        real_pane_count(pane_tree) > 1
    }
}

fn surface_has_agent_session(surface: &forktty_core::Surface) -> bool {
    surface.agent_session.is_some()
}

fn embedded_title_is_launcher_wrapper(title: &str) -> bool {
    matches!(title.trim(), "/usr/bin/env" | "/bin/env" | "env")
}

fn select_tab_from_tabstrip(
    state: &Option<SocketAppState>,
    model: &Arc<Mutex<WorkspaceModel>>,
    surface_id: &str,
) -> bool {
    if let Some(state) = state {
        return select_tab_surface(state, surface_id);
    }
    match model.lock() {
        Ok(mut model) => model.select_tab(surface_id),
        Err(_) => false,
    }
}

fn tab_scroll_target(
    current: f64,
    page_size: f64,
    lower: f64,
    upper: f64,
    tab_start: f64,
    tab_width: f64,
) -> Option<f64> {
    let tab_end = tab_start + tab_width;
    let desired = if tab_start < current {
        tab_start
    } else if tab_end > current + page_size {
        tab_end - page_size
    } else {
        return None;
    };
    Some(desired.clamp(lower, (upper - page_size).max(lower)))
}

fn reveal_tab(scroller: &gtk::ScrolledWindow, tabstrip: &gtk::Box, tab: &gtk::Box) {
    let adjustment = scroller.hadjustment();
    let Some((tab_start, _)) = tab.translate_coordinates(tabstrip, 0.0, 0.0) else {
        return;
    };
    let Some(target) = tab_scroll_target(
        adjustment.value(),
        adjustment.page_size(),
        adjustment.lower(),
        adjustment.upper(),
        tab_start,
        f64::from(tab.allocated_width()),
    ) else {
        return;
    };
    adjustment.set_value(target);
}

fn queue_reveal_tab(scroller: &gtk::ScrolledWindow, tabstrip: &gtk::Box, tab: &gtk::Box) {
    let scroller = scroller.downgrade();
    let tabstrip = tabstrip.downgrade();
    let tab = tab.downgrade();
    glib::idle_add_local_once(move || {
        let (Some(scroller), Some(tabstrip), Some(tab)) =
            (scroller.upgrade(), tabstrip.upgrade(), tab.upgrade())
        else {
            return;
        };
        reveal_tab(&scroller, &tabstrip, &tab);
    });
}

fn queue_tab_focus_after_selection(
    focus_target: gtk::Widget,
    model: Arc<Mutex<WorkspaceModel>>,
    surface_id: String,
) {
    queue_focusable_descendant_focus_when(
        focus_target,
        Rc::new(move || model_focus_still_targets_surface(&model, &surface_id)),
    );
}

pub(super) fn surface_is_eligible_for_auto_spawn(
    statuses: &[StatusEntry],
    suppressed_surface_ids: &BTreeSet<String>,
    surface_id: &str,
) -> bool {
    !suppressed_surface_ids.contains(surface_id)
        && !surface_status_blocks_auto_spawn(statuses, surface_id)
}

pub(super) fn execute_terminal_command<T>(
    execution: &CommandExecution,
    surface_id: &str,
    generation: u64,
    operation: impl FnOnce() -> Result<T, TerminalError>,
) -> Option<Result<T, TerminalError>> {
    if !execution.claim() {
        return None;
    }
    Some(execution.run_if_current_generation(surface_id, generation, operation))
}

impl TerminalController {
    pub(super) fn new(
        container: gtk::Box,
        parent_window: adw::ApplicationWindow,
        model: Arc<Mutex<WorkspaceModel>>,
        backend: Arc<GtkTerminalBackend>,
    ) -> Self {
        Self {
            container,
            parent_window,
            model,
            backend,
            state: None,
            toast_handle: None,
            widgets: BTreeMap::new(),
            embedded_ghostty_panes: BTreeMap::new(),
            embedded_ghostty: None,
            chromes: BTreeMap::new(),
            pending_spawns: BTreeMap::new(),
            last_layout_signature: None,
            last_chrome_signature: None,
            maximized_pane: false,
            surface_pids: Rc::new(RefCell::new(BTreeMap::new())),
            pane_tab_strips: Rc::new(RefCell::new(Vec::new())),
            terminal_zoom_level: Rc::new(Cell::new(0)),
            #[cfg(feature = "browser")]
            browser_panes: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    pub(super) fn attach_state(&mut self, state: SocketAppState) {
        self.state = Some(state);
    }

    pub(super) fn attach_toast_handle(&mut self, toast_handle: ToastHandle) {
        self.toast_handle = Some(toast_handle);
    }

    pub(super) fn show_toast(&self, message: &str) {
        if let Some(toast_handle) = &self.toast_handle {
            toast_handle.show(message);
        }
    }

    pub(super) fn backend_for_generation_checks(&self) -> Arc<GtkTerminalBackend> {
        self.backend.clone()
    }

    /// Capture every live embedded pane's final scrollback tail. Ghostty reads
    /// run without the model mutex; successful current-generation results are
    /// committed together under one brief lock.
    #[allow(dead_code)] // Used by the two-phase close finalizer introduced in Task 3.6.
    pub(super) fn snapshot_live_embedded_scrollback(&self, lines: u32) {
        let Some(embedder) = self.embedded_ghostty.as_ref() else {
            return;
        };
        if lines == 0 || !embedder.supports_read_text() {
            return;
        }
        let targets = self
            .embedded_ghostty_panes
            .iter()
            .map(|(surface_id, pane)| (surface_id.clone(), pane.generation, pane.surface.clone()))
            .collect::<Vec<_>>();
        snapshot_current_generation_scrollback_with(
            &self.backend,
            &self.model,
            targets,
            |surface_id, widget| read_embedded_scrollback_tail(embedder, widget, surface_id, lines),
        );
    }

    pub(super) fn handle(&mut self, command: GtkTerminalCommand) {
        match command {
            GtkTerminalCommand::Spawn {
                request,
                generation,
            } => self.spawn(request, generation),
            GtkTerminalCommand::ShowSurface {
                surface_id,
                generation,
            } => {
                let backend = self.backend.clone();
                let focused =
                    run_embedded_callback_for_generation(&backend, &surface_id, generation, || {
                        if let Ok(mut model) = self.model.lock() {
                            let _ = model.focus_surface_and_select_workspace(&surface_id);
                        }
                    });
                if focused.is_some() {
                    self.sync_model_focus_to_ui();
                }
            }
            GtkTerminalCommand::SendText {
                surface_id,
                generation,
                text,
                execution,
                reply,
            } => {
                let result = execute_terminal_command(&execution, &surface_id, generation, || {
                    if let Some(widget) = self.widgets.get(&surface_id) {
                        widget.send_text(&text);
                        Ok(())
                    } else if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
                        if run_embedded_pane_command_for_generation(
                            pane.generation,
                            generation,
                            || (),
                        )
                        .is_none()
                        {
                            return Err(TerminalError::NotReady(surface_id.clone()));
                        }
                        match &self.embedded_ghostty {
                            Some(embedder) => unsafe { embedder.send_text(&pane.surface, &text) }
                                .map_err(TerminalError::Backend),
                            None => Err(TerminalError::Backend(
                                "embedded Ghostty GTK surface has no embedder".to_string(),
                            )),
                        }
                    } else {
                        Err(TerminalError::NotReady(surface_id.clone()))
                    }
                });
                if let Some(result) = result {
                    let _ = reply.send(result);
                }
            }
            GtkTerminalCommand::ReadText {
                surface_id,
                generation,
                capture,
                max_bytes,
                execution,
                reply,
            } => {
                let result = execute_terminal_command(&execution, &surface_id, generation, || {
                    if let Some(widget) = self.widgets.get(&surface_id) {
                        widget.read_text(&surface_id, capture, max_bytes)
                    } else if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
                        if run_embedded_pane_command_for_generation(
                            pane.generation,
                            generation,
                            || (),
                        )
                        .is_none()
                        {
                            return Err(TerminalError::NotReady(surface_id.clone()));
                        }
                        match &self.embedded_ghostty {
                            Some(embedder) if embedder.supports_read_text() => unsafe {
                                embedder.read_text_snapshot(
                                    &pane.surface,
                                    &surface_id,
                                    capture,
                                    max_bytes,
                                )
                            },
                            Some(_) => Err(TerminalError::Backend(
                                "embedded Ghostty GTK library does not export read-text support"
                                    .to_string(),
                            )),
                            None => Err(TerminalError::Backend(
                                "embedded Ghostty GTK surface has no embedder".to_string(),
                            )),
                        }
                    } else {
                        Err(TerminalError::NotReady(surface_id.clone()))
                    }
                });
                if let Some(result) = result {
                    let _ = reply.send(result);
                }
            }
            GtkTerminalCommand::Resize {
                surface_id,
                generation,
                cols,
                rows,
            } => {
                if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
                    let _ = run_embedded_pane_command_for_generation(
                        pane.generation,
                        generation,
                        || (),
                    );
                    return;
                }
                let _ = run_embedded_callback_for_generation(
                    &self.backend,
                    &surface_id,
                    generation,
                    || {
                        if let Some(widget) = self.widgets.get(&surface_id) {
                            widget.resize_cells(cols, rows);
                        }
                    },
                );
            }
            GtkTerminalCommand::Close {
                surface_id,
                generation,
            } => {
                if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
                    if run_embedded_pane_command_for_generation(pane.generation, generation, || ())
                        .is_none()
                    {
                        return;
                    }
                } else if !self.widgets.contains_key(&surface_id) {
                    // A close for an incarnation whose queued Spawn was already
                    // rejected must not tear down a newer pending incarnation.
                    return;
                }
                self.pending_spawns.remove(&surface_id);
                if let (Some(embedder), Some(pane)) = (
                    self.embedded_ghostty.as_ref(),
                    self.embedded_ghostty_panes.get(&surface_id),
                ) {
                    let persistent_scrollback_lines = config::load_config()
                        .map(|config| config.appearance.persistent_scrollback_lines)
                        .unwrap_or(0);
                    snapshot_embedded_scrollback_tail_to_model(
                        &self.model,
                        embedder,
                        &pane.surface,
                        &surface_id,
                        persistent_scrollback_lines,
                    );
                }
                self.cleanup_pty_persistence_session(&surface_id);
                if let Some(chrome) = self.chromes.remove(&surface_id) {
                    detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
                }
                self.embedded_ghostty_panes.remove(&surface_id);
                self.widgets.remove(&surface_id);
                self.surface_pids.borrow_mut().remove(&surface_id);
                #[cfg(feature = "browser")]
                if let Some(pane) = self.browser_panes.borrow_mut().remove(&surface_id) {
                    pane.prepare_close();
                }
                self.rebuild_layout();
            }
        }
    }

    fn cleanup_pty_persistence_session(&self, surface_id: &str) {
        let Some(state) = &self.state else {
            return;
        };
        let Some(runtime_dir) = state.socket_path.parent() else {
            return;
        };
        match forktty_core::pty_persistence::cleanup_managed_session(runtime_dir, surface_id) {
            Ok(summary)
                if summary.sockets_removed > 0
                    || summary.processes_signaled > 0
                    || summary.process_signal_errors > 0 =>
            {
                eprintln!(
                    "ForkTTY: PTY persistence cleanup for {surface_id} removed {} socket(s), signaled {} process(es), {} signal error(s)",
                    summary.sockets_removed,
                    summary.processes_signaled,
                    summary.process_signal_errors
                );
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("ForkTTY: failed to clean up PTY persistence for {surface_id}: {err}");
            }
        }
    }

    fn spawn(&mut self, request: SpawnRequest, generation: u64) {
        if !self
            .backend
            .surface_generation_is_current(&request.surface_id, generation)
            .unwrap_or(false)
        {
            return;
        }
        if self.widgets.contains_key(&request.surface_id)
            || self
                .embedded_ghostty_panes
                .contains_key(&request.surface_id)
        {
            return;
        }
        mark_spawn_command_pending(&mut self.pending_spawns, &request.surface_id);
        match self.spawn_embedded_ghostty(request.clone(), generation) {
            Ok(()) => {}
            Err(err) => {
                self.pending_spawns.remove(&request.surface_id);
                record_terminal_spawn_failure(
                    &self.model,
                    &request.workspace_id,
                    &request.surface_id,
                    &format!("Failed to spawn embedded Ghostty GTK pane: {err}"),
                    self.state
                        .as_ref()
                        .is_none_or(|state| state.notification_dispatch),
                );
                let _ = self
                    .backend
                    .close_for_generation(&request.surface_id, generation);
                self.last_layout_signature = None;
                self.rebuild_layout();
            }
        }
    }

    pub(super) fn rebuild_layout(&mut self) {
        self.spawn_active_surfaces_if_needed();
        self.rebuild_layout_without_surface_reconciliation();
    }

    fn rebuild_layout_without_surface_reconciliation(&mut self) {
        self.last_chrome_signature = None;
        // Drop browser panes whose surfaces were removed from the model so the
        // webviews don't linger after a model-driven (e.g. socket) close.
        #[cfg(feature = "browser")]
        {
            let live_surface_ids = self
                .model
                .lock()
                .ok()
                .map(|model| {
                    model
                        .list_surfaces(None)
                        .into_iter()
                        .map(|surface| surface.id)
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let stale_browser_ids = self
                .browser_panes
                .borrow()
                .keys()
                .filter(|surface_id| !live_surface_ids.contains(*surface_id))
                .cloned()
                .collect::<Vec<_>>();
            for surface_id in stale_browser_ids {
                if let Some(pane) = self.browser_panes.borrow_mut().remove(&surface_id) {
                    pane.prepare_close();
                }
            }
            // Cached browser widgets are reused across rebuilds; detach them from
            // their previous parent first, or set_start/end_child asserts
            // (gtk_widget_get_parent(child) == NULL) when re-inserting.
            for pane in self.browser_panes.borrow().values() {
                detach_widget(&pane.widget());
            }
        }
        for chrome in self.chromes.values() {
            detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
        }
        self.pane_tab_strips.borrow_mut().clear();
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }

        let Some((signature, pane_tree, focused_surface_id, workspace_id)) =
            active_layout_snapshot(&self.model)
        else {
            self.maximized_pane = normalize_maximized_pane_state(self.maximized_pane, None);
            self.set_terminal_sibling_flags(&BTreeSet::new(), false);
            self.last_layout_signature = Some(EMPTY_LAYOUT_SIGNATURE.to_string());
            self.container.append(&empty_terminal_stage(
                self.state.as_ref(),
                Some(&self.parent_window),
            ));
            return;
        };
        self.maximized_pane = normalize_maximized_pane_state(self.maximized_pane, Some(&pane_tree));
        let visible_tree = if self.maximized_pane {
            PaneNode::single_leaf(focused_surface_id.clone())
        } else {
            pane_tree
        };
        let visible_panes = collect_panes(&visible_tree);
        let visible_surface_ids = collect_leaves(&visible_tree).into_iter().collect();
        let single_pane = visible_panes.len() == 1;
        self.set_terminal_sibling_flags(&visible_surface_ids, !single_pane);
        let widget = self.widget_for_pane(&visible_tree, &workspace_id);
        for chrome in self.chromes.values() {
            chrome.header_revealer.set_reveal_child(!single_pane);
            chrome.single_pane_actions.set_visible(single_pane);
            chrome.single_pane_actions.set_sensitive(single_pane);
        }
        self.container.append(&widget);
        self.queue_focus_for_surface(&focused_surface_id);
        self.last_layout_signature = Some(effective_layout_signature(
            &signature,
            self.maximized_pane,
            &focused_surface_id,
        ));
    }

    pub(super) fn toggle_maximized_pane(&mut self) {
        let pane_tree = active_layout_snapshot(&self.model).map(|(_, tree, _, _)| tree);
        let next = toggle_maximized_pane_state(self.maximized_pane, pane_tree.as_ref());
        if next == self.maximized_pane {
            return;
        }
        self.maximized_pane = next;
        self.last_layout_signature = None;
        self.rebuild_layout();
    }

    fn queue_focus_for_surface(&self, surface_id: &str) {
        if let Some(widget) = self.widgets.get(surface_id) {
            queue_widget_focus(widget.widget());
        } else if let Some(pane) = self.embedded_ghostty_panes.get(surface_id) {
            let model = self.model.clone();
            let surface_id = surface_id.to_string();
            queue_focusable_descendant_focus_when(
                pane.surface.clone(),
                Rc::new(move || model_focus_still_targets_surface(&model, &surface_id)),
            );
        } else {
            // Browser panes are not in self.widgets; hand keyboard focus to the
            // pane's focus target so keyboard-only nav reaches the browser.
            #[cfg(feature = "browser")]
            if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
                queue_widget_focus(pane.focus_target());
            }
        }
    }

    fn tab_focus_target_for_surface(&self, surface_id: &str) -> Option<gtk::Widget> {
        if let Some(widget) = self.widgets.get(surface_id) {
            return Some(widget.widget());
        }
        if let Some(pane) = self.embedded_ghostty_panes.get(surface_id) {
            return Some(pane.surface.clone());
        }
        #[cfg(feature = "browser")]
        if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
            return Some(pane.focus_target());
        }
        None
    }

    fn spawn_active_surfaces_if_needed(&mut self) {
        let Some(state) = self.state.clone() else {
            return;
        };
        // Rebuilds run from synchronous GTK callbacks and backend events. Do
        // not block the main loop (or self-deadlock if an owning transaction
        // triggered the event): the periodic layout refresh retries after the
        // current topology transaction releases the shared guard.
        let Some(_surface_set_guard) = state.try_surface_set_guard() else {
            return;
        };
        let backend_surface_ids = state
            .terminal
            .surfaces()
            .map(|surfaces| {
                surfaces
                    .into_iter()
                    .map(|surface| surface.surface_id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let suppressed_surface_ids = state.suppressed_auto_spawn_surface_ids();
        let (surfaces, model_surface_ids) = {
            let Ok(model) = self.model.lock() else {
                return;
            };
            let Some(workspace) = model.active_workspace() else {
                return;
            };
            let statuses = model.list_status(&workspace.id);
            let surfaces = model
                .list_surfaces(Some(&workspace.id))
                .into_iter()
                .map(|surface| {
                    let eligible = surface_is_eligible_for_auto_spawn(
                        &statuses,
                        &suppressed_surface_ids,
                        &surface.id,
                    );
                    (surface, eligible)
                })
                .collect::<Vec<_>>();
            // Reconcile against every workspace's surfaces, not just the active
            // one, so a terminal backgrounded in another workspace is never
            // mistaken for an orphan.
            let model_surface_ids = model
                .list_surfaces(None)
                .into_iter()
                .map(|surface| surface.id)
                .collect::<BTreeSet<_>>();
            (surfaces, model_surface_ids)
        };
        clear_modeled_pending_spawns(
            &mut self.pending_spawns,
            &model_surface_ids,
            &backend_surface_ids,
        );
        // A surface restarted or closed on the GTK thread while a socket client
        // concurrently removed it from the model can leave a backend terminal
        // (PTY + widget) with no model counterpart. Tear those orphans down;
        // the queued Close event drops the widget on the next rebuild.
        for surface_id in orphaned_backend_surfaces(
            &backend_surface_ids,
            &model_surface_ids,
            &self.pending_spawns,
        ) {
            match state.terminal.close(&surface_id) {
                Ok(()) | Err(TerminalError::NotFound(_)) => {}
                Err(err) => {
                    eprintln!("Failed to reap orphaned terminal surface {surface_id}: {err}");
                }
            }
        }
        for (surface, auto_spawn_eligible) in surfaces {
            if !auto_spawn_eligible {
                continue;
            }
            if self.widgets.contains_key(&surface.id)
                || self.embedded_ghostty_panes.contains_key(&surface.id)
                || self.pending_spawns.contains_key(&surface.id)
                || backend_surface_ids.contains(&surface.id)
            {
                continue;
            }
            // Browser surfaces are rendered by browser_panes and never get a
            // terminal backend; spawning a PTY for them would leak a hidden
            // shell process and emit bogus terminal status/port/close events.
            // Ssh surfaces rewrite the request to launch ssh <host>; this is
            // what respawns restored remote workspaces on session restore.
            // Agent terminal surfaces rewrite to the provider resume argv.
            let base =
                SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
            let surface_id = surface.id.clone();
            let workspace_id = surface.workspace_id.clone();
            let request = match forktty_socket::spawn_request_for_surface(base, &surface) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    record_terminal_spawn_failure(
                        &self.model,
                        &workspace_id,
                        &surface_id,
                        &error.to_string(),
                        state.notification_dispatch,
                    );
                    continue;
                }
            };
            mark_spawn_command_pending(&mut self.pending_spawns, &surface_id);
            if let Err(err) = state.terminal.spawn(request) {
                self.pending_spawns.remove(&surface_id);
                record_terminal_spawn_failure(
                    &self.model,
                    &workspace_id,
                    &surface_id,
                    &err.to_string(),
                    state.notification_dispatch,
                );
            }
        }
    }

    fn refresh_chromes(&mut self) {
        let Ok(model) = self.model.lock() else {
            return;
        };
        let focused_surface_id = model
            .active_workspace()
            .map(|workspace| workspace.focused_surface_id);
        let chrome_surface_ids = self.chromes.keys().cloned().collect::<Vec<_>>();
        let tab_strip_tabs = self
            .pane_tab_strips
            .borrow()
            .iter()
            .map(|strip| strip.tabs.clone())
            .collect::<Vec<_>>();
        let signature = chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strip_tabs);
        if self.last_chrome_signature.as_deref() == Some(signature.as_str()) {
            return;
        }
        self.last_chrome_signature = Some(signature);
        let chrome_updates = self
            .chromes
            .keys()
            .filter_map(|surface_id| {
                model.surface(surface_id).map(|surface| {
                    (
                        surface_id.clone(),
                        surface.clone(),
                        focused_surface_id.as_deref() == Some(surface_id.as_str()),
                    )
                })
            })
            .collect::<Vec<_>>();
        let tab_updates = tab_strip_refreshes(&model, &tab_strip_tabs);
        // Browser navigation only mutates a surface's url (same layout structure),
        // so the layout signature is unchanged and rebuild_layout does not fire.
        // Snapshot before touching live GTK/WebKit widgets, then release the
        // model lock so focus and load callbacks can safely sync back into it.
        #[cfg(feature = "browser")]
        let browser_targets = model
            .list_surfaces(None)
            .into_iter()
            .filter_map(|surface| match surface.kind {
                forktty_core::SurfaceKind::Browser { url, .. } => {
                    let active = focused_surface_id.as_deref() == Some(surface.id.as_str());
                    Some((surface.id, (url, active)))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        drop(model);
        for (surface_id, surface, active) in chrome_updates {
            if let Some(widget) = self.widgets.get(&surface_id) {
                widget.set_local_selection_on_mouse_drag(surface_has_agent_session(&surface));
            }
            if let Some(chrome) = self.chromes.get(&surface_id) {
                update_pane_chrome(chrome, &surface, active);
            }
        }
        self.refresh_tab_strips(&tab_updates);
        // Push the latest url into the live webview on each refresh tick, and
        // drop panes for surfaces that no longer exist to avoid leaking them.
        #[cfg(feature = "browser")]
        {
            let stale = self
                .browser_panes
                .borrow()
                .keys()
                .filter(|surface_id| !browser_targets.contains_key(*surface_id))
                .cloned()
                .collect::<Vec<_>>();
            for surface_id in stale {
                if let Some(pane) = self.browser_panes.borrow_mut().remove(&surface_id) {
                    pane.prepare_close();
                }
            }
            for (surface_id, pane) in self.browser_panes.borrow().iter() {
                if let Some((url, active)) = browser_targets.get(surface_id) {
                    // Safe to call every tick: BrowserPaneWidget edge-triggers on the
                    // last *requested* url, so an unchanged url is a no-op.
                    pane.load_uri(url);
                    pane.set_active(*active);
                }
            }
        }
    }

    fn refresh_tab_strips(&self, updates: &[TabStripRefresh]) {
        for (strip, update) in self.pane_tab_strips.borrow().iter().zip(updates.iter()) {
            let Some(active_id) = update.active_id.as_ref() else {
                continue;
            };
            let previous_visible = strip
                .stack
                .visible_child_name()
                .map(|name| name.to_string());
            let active_changed = previous_visible.as_deref() != Some(active_id.as_str());
            if active_changed {
                strip.stack.set_visible_child_name(active_id.as_str());
                self.queue_focus_for_surface(active_id);
            }
            for (idx, tab_update) in update.tabs.iter().enumerate() {
                if let Some(tab) = strip.tab_widgets.get(idx) {
                    if tab_update.active {
                        tab.add_css_class("active");
                    } else {
                        tab.remove_css_class("active");
                    }
                    update_tab_tooltip(tab, Some(tab_update.title.clone()));
                }
                if let Some(label) = strip.labels.get(idx) {
                    label.set_label(&tab_update.title);
                    if let Some(select) = strip.select_areas.get(idx) {
                        update_tab_tooltip(select, Some(tab_update.title.clone()));
                        select.update_property(&[gtk::accessible::Property::Label(&format!(
                            "Select tab {}",
                            tab_update.title
                        ))]);
                        select.update_state(&[gtk::accessible::State::Selected(Some(
                            tab_update.active,
                        ))]);
                    }
                }
            }
            if active_changed {
                if let Some(active_index) = update.tabs.iter().position(|tab| tab.active) {
                    if let Some(tab) = strip.tab_widgets.get(active_index) {
                        queue_reveal_tab(&strip.scroller, &strip.tabstrip, tab);
                    }
                }
            }
        }
    }

    fn widget_for_pane(&self, node: &PaneNode, workspace_id: &str) -> gtk::Widget {
        let model = self.model.clone();
        let state_for_resize = self.state.clone();
        let workspace_id_for_resize = workspace_id.to_string();
        let pending_resize_save = Rc::new(RefCell::new(None::<glib::SourceId>));
        let on_resize: SplitResizeCallback =
            Rc::new(move |left: &[String], right: &[String], ratio: f64| {
                let changed = if let Ok(mut model) = model.lock() {
                    model.update_split_partition_ratio(&workspace_id_for_resize, left, right, ratio)
                } else {
                    false
                };
                if !changed {
                    return;
                }
                let Some(state) = state_for_resize.clone() else {
                    return;
                };
                if let Some(source_id) = pending_resize_save.borrow_mut().take() {
                    source_id.remove();
                }
                let state_for_timeout = state.clone();
                let pending_resize_save_for_timeout = pending_resize_save.clone();
                let source_id =
                    glib::timeout_add_local_once(SESSION_RESIZE_SAVE_DEBOUNCE, move || {
                        *pending_resize_save_for_timeout.borrow_mut() = None;
                        save_session_from_state(&state_for_timeout);
                    });
                *pending_resize_save.borrow_mut() = Some(source_id);
            });
        self.widget_for_pane_with_resize(node, on_resize)
    }

    fn widget_for_pane_with_resize(
        &self,
        node: &PaneNode,
        on_resize: SplitResizeCallback,
    ) -> gtk::Widget {
        match node {
            PaneNode::Leaf { tabs, active } => {
                let Some(active_index) = active_tab_index_for_leaf(tabs, *active) else {
                    return missing_surface_placeholder(
                        "empty-leaf",
                        self.state.as_ref(),
                        Some(&self.model),
                    )
                    .upcast();
                };
                let active_id = &tabs[active_index];
                if tabs.len() == 1 {
                    self.pane_widget_for(active_id)
                } else {
                    self.leaf_widget_with_tabstrip(tabs, active_index)
                }
            }
            PaneNode::Split {
                axis,
                children,
                sizes,
            } => {
                let orientation = match axis {
                    SplitAxis::Horizontal => gtk::Orientation::Horizontal,
                    SplitAxis::Vertical => gtk::Orientation::Vertical,
                };
                let on_resize_inner = on_resize.clone();
                build_split_widget(orientation, children, sizes, on_resize, move |child| {
                    self.widget_for_pane_with_resize(child, on_resize_inner.clone())
                })
            }
        }
    }

    fn pane_widget_for(&self, surface_id: &str) -> gtk::Widget {
        let kind = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.surface(surface_id).map(|s| s.kind.clone()));
        match kind {
            #[cfg(feature = "browser")]
            Some(forktty_core::SurfaceKind::Browser { url, profile }) => {
                self.browser_pane_widget(surface_id, &url, &profile.to_string())
            }
            #[cfg(not(feature = "browser"))]
            Some(forktty_core::SurfaceKind::Browser { .. }) => {
                browser_unavailable_placeholder(surface_id).upcast()
            }
            _ => self.terminal_pane_widget(surface_id),
        }
    }

    /// Build a compact per-pane tab strip for leaves with >1 tab.
    fn leaf_widget_with_tabstrip(&self, tabs: &[String], active: usize) -> gtk::Widget {
        let Some(active) = active_tab_index_for_leaf(tabs, active) else {
            return missing_surface_placeholder(
                "empty-tabs",
                self.state.as_ref(),
                Some(&self.model),
            )
            .upcast();
        };
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.set_hexpand(true);
        outer.set_vexpand(true);

        // Collect surface titles from model.
        let titles: Vec<String> = {
            let model = self.model.lock().ok();
            tabs.iter()
                .map(|id| {
                    model
                        .as_ref()
                        .and_then(|m| m.surface(id))
                        .map(surface_title)
                        .unwrap_or_else(|| "Terminal".to_string())
                })
                .collect()
        };

        let tabstrip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .accessible_role(gtk::AccessibleRole::TabList)
            .build();
        tabstrip.add_css_class("pane-tabstrip");
        tabstrip.set_hexpand(true);
        let tabstrip_scroller = gtk::ScrolledWindow::new();
        tabstrip_scroller.add_css_class("pane-tabstrip-scroll");
        tabstrip_scroller.set_hexpand(true);
        tabstrip_scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        tabstrip_scroller.set_overlay_scrolling(true);
        tabstrip_scroller.set_propagate_natural_height(true);
        tabstrip_scroller.set_child(Some(&tabstrip));
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::None);
        let mut tab_widgets = Vec::with_capacity(tabs.len());
        let mut labels = Vec::with_capacity(tabs.len());
        let mut select_areas = Vec::with_capacity(tabs.len());

        for (idx, (surface_id, title)) in tabs.iter().zip(titles.iter()).enumerate() {
            let tab = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            tab.add_css_class("pane-tab");
            // Stop child expansion from propagating into the strip: tabs stay
            // compact while the select area can still fill each tab hit target.
            tab.set_hexpand(false);
            tab.set_valign(gtk::Align::Center);
            update_tab_tooltip(&tab, Some(title.clone()));
            if idx == active {
                tab.add_css_class("active");
            }

            let label = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(18)
                .single_line_mode(true)
                .build();
            label.add_css_class("pane-tab-label");

            let select_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let select = gtk::Button::builder()
                .child(&select_content)
                .has_frame(false)
                .accessible_role(gtk::AccessibleRole::Tab)
                .build();
            select.add_css_class("flat");
            select.set_hexpand(true);
            select.set_valign(gtk::Align::Center);
            select.add_css_class("pane-tab-select");
            let grip = gtk::Image::from_icon_name("forktty-menu-symbolic");
            grip.add_css_class("pane-tab-grip");
            grip.set_tooltip_text(Some("Drag to move tab"));
            install_tab_drag_source(&grip, surface_id);
            select_content.append(&grip);
            select_content.append(&label);
            update_tab_tooltip(&select, Some(title.clone()));
            select.update_property(&[gtk::accessible::Property::Label(&format!(
                "Select tab {title}"
            ))]);
            select.update_state(&[gtk::accessible::State::Selected(Some(idx == active))]);

            let close = gtk::Button::builder()
                .icon_name("forktty-close-symbolic")
                .has_frame(false)
                .build();
            close.add_css_class("flat");
            close.add_css_class("pane-tab-close");
            close.set_tooltip_text(Some("Close Tab"));
            set_accessible_button_text(&close, "Close Tab", None);

            let state_for_close = self.state.clone();
            let parent_for_close = self.parent_window.clone();
            let close_id = surface_id.clone();
            let is_last_tab = tabs.len() == 1;
            close.connect_clicked(move |_| {
                if is_last_tab {
                    if let Some(state) = &state_for_close {
                        show_close_pane_confirmation(&parent_for_close, state, &close_id);
                    }
                    return;
                }
                if let Some(state) = &state_for_close {
                    close_tab_surface(state, &close_id);
                }
            });

            if let Some(state) = &self.state {
                install_tab_context_menu(&tab, surface_id, state, &self.parent_window);
            }

            tab.append(&select);
            tab.append(&close);
            tabstrip.append(&tab);
            tab_widgets.push(tab);
            labels.push(label);
            select_areas.push(select);
        }
        install_tabstrip_drop_target(
            &tabstrip,
            &tab_widgets,
            tabs,
            self.model.clone(),
            self.state.clone(),
        );
        outer.append(&tabstrip_scroller);
        if let Some(active_tab) = tab_widgets.get(active) {
            queue_reveal_tab(&tabstrip_scroller, &tabstrip, active_tab);
        }

        // Keep tab children mounted and switch the visible child in-place so a
        // tab change does not rebuild the surrounding split tree.
        for (index, surface_id) in tabs.iter().enumerate() {
            let pane_widget = self.pane_widget_for(surface_id);
            stack.add_named(&pane_widget, Some(surface_id.as_str()));
            if let Some(select) = select_areas.get(index) {
                let page = stack.page(&pane_widget);
                select
                    .update_relation(&[gtk::accessible::Relation::Controls(&[page.upcast_ref()])]);
                let model_for_select = self.model.clone();
                let state_for_select = self.state.clone();
                let stack_for_select = stack.downgrade();
                let select_id = surface_id.clone();
                let tab_widgets_for_select = tab_widgets
                    .iter()
                    .map(gtk::prelude::ObjectExt::downgrade)
                    .collect::<Vec<_>>();
                let select_areas_for_select = select_areas
                    .iter()
                    .map(gtk::prelude::ObjectExt::downgrade)
                    .collect::<Vec<_>>();
                let focus_target = self
                    .tab_focus_target_for_surface(surface_id)
                    .map(|target| target.downgrade());
                select.connect_clicked(move |_| {
                    if !select_tab_from_tabstrip(&state_for_select, &model_for_select, &select_id) {
                        return;
                    }
                    let Some(stack_for_select) = stack_for_select.upgrade() else {
                        return;
                    };
                    stack_for_select.set_visible_child_name(select_id.as_str());
                    for (tab_index, (tab, select)) in tab_widgets_for_select
                        .iter()
                        .zip(select_areas_for_select.iter())
                        .enumerate()
                    {
                        let (Some(tab), Some(select)) = (tab.upgrade(), select.upgrade()) else {
                            continue;
                        };
                        let active = tab_index == index;
                        if active {
                            tab.add_css_class("active");
                        } else {
                            tab.remove_css_class("active");
                        }
                        select.update_state(&[gtk::accessible::State::Selected(Some(active))]);
                    }
                    if let Some(focus_target) =
                        focus_target.as_ref().and_then(|target| target.upgrade())
                    {
                        queue_tab_focus_after_selection(
                            focus_target,
                            model_for_select.clone(),
                            select_id.clone(),
                        );
                    }
                });
            }
        }
        let active_id = &tabs[active.min(tabs.len().saturating_sub(1))];
        stack.set_visible_child_name(active_id.as_str());
        outer.append(&stack);

        self.pane_tab_strips.borrow_mut().push(PaneTabStrip {
            tabs: tabs.to_vec(),
            scroller: tabstrip_scroller,
            tabstrip,
            stack,
            tab_widgets,
            labels,
            select_areas,
        });

        outer.upcast()
    }

    #[cfg(feature = "browser")]
    fn browser_pane_widget(&self, surface_id: &str, url: &str, profile_id: &str) -> gtk::Widget {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
            // Widget self-guards on the last requested url; calling unconditionally
            // is harmless and avoids the committed-uri divergence problem.
            pane.load_uri(url);
            pane.set_active(self.is_surface_focused(surface_id));
            return pane.widget();
        }
        let pane = Rc::new(crate::browser_pane::BrowserPaneWidget::new(profile_id, url));
        pane.set_active(self.is_surface_focused(surface_id));
        if let Some(state) = &self.state {
            install_pane_reorder_dnd(&pane.drag_handle(), surface_id, state);
        }
        // Address-bar Enter navigates via the model so socket + manual share one path.
        let model = self.model.clone();
        let id = surface_id.to_string();
        pane.connect_address_activate(move |text| {
            let Some(normalized) = forktty_core::normalize_browser_url(&text) else {
                return;
            };
            if let Ok(mut m) = model.lock() {
                m.set_surface_url(&id, &normalized);
            }
        });
        // Keep model focus in sync when the browser pane gains focus, mirroring
        // the terminal focus handler, so focus-driven split/close target this pane.
        {
            let focus_model = self.model.clone();
            let focus_id = surface_id.to_string();
            pane.connect_focus_in(move || {
                if let Ok(mut m) = focus_model.lock() {
                    let _ = m.focus_surface(&focus_id);
                    let _ = m.mark_surface_unread(&focus_id, false);
                }
            });
        }
        // Keep the model's browser URL in sync with WebKit commits caused by
        // redirects, history buttons, or user link clicks. Without this, the
        // periodic refresh path sees the old model URL and navigates back.
        {
            let url_model = self.model.clone();
            let url_id = surface_id.to_string();
            pane.connect_uri_committed(move |url| {
                if let Ok(mut m) = url_model.lock() {
                    let _ = m.set_surface_url(&url_id, &url);
                }
            });
        }
        // Wire the × button to the same confirmation flow terminal panes use.
        if let Some(state) = self.state.clone() {
            let parent = self.parent_window.clone();
            let sid_close = surface_id.to_string();
            pane.connect_close(move || {
                show_close_pane_confirmation(&parent, &state, &sid_close);
            });
        }
        let widget = pane.widget();
        self.browser_panes
            .borrow_mut()
            .insert(surface_id.to_string(), pane);
        widget
    }

    #[cfg(feature = "browser")]
    fn is_surface_focused(&self, surface_id: &str) -> bool {
        self.model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .is_some_and(|workspace| workspace.focused_surface_id == surface_id)
    }

    #[cfg(feature = "browser")]
    pub(super) fn browser_pane(
        &self,
        surface_id: &str,
    ) -> Option<Rc<crate::browser_pane::BrowserPaneWidget>> {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id).cloned() {
            return Some(pane);
        }
        let (url, profile_id) = self.model.lock().ok().and_then(|model| {
            model
                .surface(surface_id)
                .and_then(|surface| match &surface.kind {
                    forktty_core::SurfaceKind::Browser { url, profile } => {
                        Some((url.clone(), profile.to_string()))
                    }
                    _ => None,
                })
        })?;
        let _ = self.browser_pane_widget(surface_id, &url, &profile_id);
        self.browser_panes.borrow().get(surface_id).cloned()
    }

    fn terminal_pane_widget(&self, surface_id: &str) -> gtk::Widget {
        let Some(chrome) = self.chromes.get(surface_id) else {
            return missing_surface_placeholder(surface_id, self.state.as_ref(), Some(&self.model))
                .upcast();
        };
        let (surface, active) = self
            .model
            .lock()
            .ok()
            .and_then(|model| {
                let surface = model.surface(surface_id)?.clone();
                let active = model
                    .list_workspaces()
                    .into_iter()
                    .any(|workspace| workspace.focused_surface_id == surface_id);
                Some((surface, active))
            })
            .unwrap_or_else(|| {
                (
                    Surface {
                        id: surface_id.to_string(),
                        workspace_id: String::new(),
                        cwd: PathBuf::from("/"),
                        title: "Terminal".to_string(),
                        unread: false,
                        needs_attention: false,
                        kind: forktty_core::SurfaceKind::Terminal,
                        agent_session: None,
                        persisted_scrollback: None,
                    },
                    false,
                )
            });

        update_pane_chrome(chrome, &surface, active);
        chrome.pane.clone().upcast()
    }
}

/// Backend terminal surfaces that no longer map to any model surface are orphans
/// that must be torn down: this happens when a surface is restarted or closed on
/// the GTK thread while a socket client concurrently removes the same surface
/// from the model, so the GTK path re-spawns (or pre-spawns a replacement) into
/// the backend after the model entry is gone, stranding a hidden PTY and widget.
/// Surfaces with a freshly in-flight spawn are excluded because their backend
/// entry exists before the matching `Spawn` command commits the widget. A
/// pending spawn is only protected for one reconciliation after its backend
/// appears while absent from the model; if the model commit is lost, the next
/// reconciliation drops the pending marker and reaps the orphan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PendingSpawn {
    observed_unmodeled_backend: bool,
}

pub(super) fn orphaned_backend_surfaces(
    backend_ids: &BTreeSet<String>,
    model_ids: &BTreeSet<String>,
    pending: &BTreeMap<String, PendingSpawn>,
) -> Vec<String> {
    backend_ids
        .iter()
        .filter(|id| !model_ids.contains(*id) && !pending.contains_key(*id))
        .cloned()
        .collect()
}

pub(super) fn mark_spawn_command_pending(
    pending: &mut BTreeMap<String, PendingSpawn>,
    surface_id: &str,
) {
    pending.insert(surface_id.to_string(), PendingSpawn::default());
}

pub(super) fn clear_modeled_pending_spawns(
    pending: &mut BTreeMap<String, PendingSpawn>,
    model_ids: &BTreeSet<String>,
    backend_ids: &BTreeSet<String>,
) {
    pending.retain(|id, spawn| {
        if model_ids.contains(id) {
            return false;
        }
        if backend_ids.contains(id) {
            let keep = !spawn.observed_unmodeled_backend;
            spawn.observed_unmodeled_backend = true;
            return keep;
        }
        true
    });
}

fn install_tab_context_menu(
    tab: &gtk::Box,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tab_for_menu = tab.downgrade();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();
    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        let Some(tab_for_menu) = tab_for_menu.upgrade() else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        // Drop the RefMut before popdown(): it emits `closed` synchronously and
        // its handler re-borrows the same cell.
        let previous_popover = current_popover_for_menu.borrow_mut().take();
        if let Some(popover) = previous_popover {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
        let popover = build_tab_context_menu(&state_for_menu, &surface_id_for_menu);
        let current_popover_for_closed = current_popover_for_menu.clone();
        popover.connect_closed(move |popover| {
            let should_clear = current_popover_for_closed
                .borrow()
                .as_ref()
                .is_some_and(|current| current == popover);
            if should_clear {
                current_popover_for_closed.borrow_mut().take();
            }
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        let (popover_x, popover_y) = tab_for_menu
            .translate_coordinates(&parent_for_menu, x, y)
            .unwrap_or((x, y));
        popover.set_parent(&parent_for_menu);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            popover_x.round() as i32,
            popover_y.round() as i32,
            1,
            1,
        )));
        *current_popover_for_menu.borrow_mut() = Some(popover.clone());
        popover.popup();
    });
    tab.add_controller(gesture);
}

fn build_tab_context_menu(state: &SocketAppState, surface_id: &str) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");
    if let Some((workspace, surface)) = terminal_context_snapshot(state, surface_id) {
        add_terminal_context_menu_header(&menu, &workspace, &surface);
        add_context_menu_separator(&menu);
    }

    let state_for_new_tab = state.clone();
    let new_tab_id = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-add-symbolic",
        "New Tab",
        Some("Ctrl+Shift+T"),
        false,
        move || add_new_tab_surface(&state_for_new_tab, &new_tab_id),
    );

    let state_for_split = state.clone();
    let split_id = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
        "Split Right",
        Some("Ctrl+Shift+H"),
        false,
        move || {
            request_split_surface_by_id(&state_for_split, &split_id, SplitAxis::Horizontal);
        },
    );

    let state_for_split = state.clone();
    let split_id = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-split-vertical-symbolic",
        "Split Down",
        Some(SPLIT_VERTICAL_SHORTCUT),
        false,
        move || {
            request_split_surface_by_id(&state_for_split, &split_id, SplitAxis::Vertical);
        },
    );

    add_context_menu_separator(&menu);

    let state_for_close = state.clone();
    let close_id = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-close-symbolic",
        "Close Tab",
        Some("Ctrl+Shift+W"),
        true,
        move || {
            close_tab_surface(&state_for_close, &close_id);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_title_filter_rejects_launcher_wrappers() {
        assert!(embedded_title_is_launcher_wrapper("/usr/bin/env"));
        assert!(embedded_title_is_launcher_wrapper(" /bin/env "));
        assert!(embedded_title_is_launcher_wrapper("env"));
        assert!(!embedded_title_is_launcher_wrapper("worker:codex-smoke"));
        assert!(!embedded_title_is_launcher_wrapper("Codex"));
    }

    #[test]
    fn tab_scroll_target_reveals_hidden_edges_and_clamps_to_range() {
        assert_eq!(tab_scroll_target(0.0, 100.0, 0.0, 300.0, 20.0, 40.0), None);
        assert_eq!(
            tab_scroll_target(80.0, 100.0, 0.0, 300.0, 20.0, 40.0),
            Some(20.0)
        );
        assert_eq!(
            tab_scroll_target(0.0, 100.0, 0.0, 300.0, 140.0, 40.0),
            Some(80.0)
        );
        assert_eq!(
            tab_scroll_target(100.0, 100.0, 0.0, 220.0, 200.0, 40.0),
            Some(120.0)
        );
    }

    #[test]
    fn pane_tab_selection_callbacks_do_not_retain_closed_tabstrip() {
        let _ = crate::test_env::with_gtk_test(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let (first_surface_id, second_surface_id) = {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", Path::new("/tmp"));
                let first_surface_id = workspace.focused_surface_id;
                let second_surface_id = model.add_tab(&first_surface_id).expect("second tab").id;
                (first_surface_id, second_surface_id)
            };
            let (tx, _rx) = mpsc::channel();
            let backend = Arc::new(GtkTerminalBackend::new(tx));
            let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let parent = adw::ApplicationWindow::builder().build();
            let mut controller = TerminalController::new(container, parent.clone(), model, backend);

            let tabs = vec![first_surface_id, second_surface_id];
            for surface_id in &tabs {
                let content = gtk::Label::new(Some(surface_id)).upcast::<gtk::Widget>();
                controller.chromes.insert(
                    surface_id.clone(),
                    build_embedded_ghostty_pane_chrome(surface_id, &content, None, &parent),
                );
            }

            let tabstrip = controller.leaf_widget_with_tabstrip(&tabs, 0);
            let (stack, tab, selector) = {
                let strips = controller.pane_tab_strips.borrow();
                let strip = strips.first().expect("pane tab strip");
                (
                    strip.stack.downgrade(),
                    strip.tab_widgets[0].downgrade(),
                    strip.select_areas[0].downgrade(),
                )
            };

            controller.pane_tab_strips.borrow_mut().clear();
            drop(tabstrip);
            while glib::MainContext::default().iteration(false) {}

            assert!(stack.upgrade().is_none(), "tab callback retained stack");
            assert!(tab.upgrade().is_none(), "tab callback retained tab widget");
            assert!(
                selector.upgrade().is_none(),
                "tab callback retained selector button"
            );
            parent.close();
        });
    }

    #[test]
    fn controller_surface_reconciliation_defers_while_shared_surface_guard_is_held() {
        let _ = crate::test_env::with_gtk_test(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let workspace = model
                .lock()
                .unwrap()
                .create_workspace("main", Path::new("/tmp"));
            let (tx, rx) = mpsc::channel();
            let backend = Arc::new(GtkTerminalBackend::new(tx));
            let state = SocketAppState::new(
                model.clone(),
                backend.clone(),
                "/bin/sh",
                PathBuf::from("/tmp/forktty.sock"),
            )
            .with_notification_dispatch(false);
            let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let parent = adw::ApplicationWindow::builder().build();
            let mut controller =
                TerminalController::new(container, parent.clone(), model, backend.clone());
            controller.attach_state(state.clone());

            let surface_guard = glib::MainContext::new().block_on(state.surface_set_guard());
            controller.spawn_active_surfaces_if_needed();
            assert!(
                matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
                "controller reconciliation must not spawn while another topology transaction holds the surface guard"
            );

            drop(surface_guard);
            controller.spawn_active_surfaces_if_needed();
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                GtkTerminalCommand::Spawn { .. }
            ));

            backend
                .spawn(SpawnRequest {
                    surface_id: "orphan-surface".to_string(),
                    workspace_id: workspace.id,
                    shell: "/bin/sh".to_string(),
                    args: Vec::new(),
                    cwd: PathBuf::from("/tmp"),
                    socket_path: PathBuf::from("/tmp/forktty.sock"),
                    extra_env: Vec::new(),
                    eligible_for_pty_persistence: false,
                })
                .unwrap();
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                GtkTerminalCommand::Spawn { .. }
            ));

            let surface_guard = glib::MainContext::new().block_on(state.surface_set_guard());
            controller.spawn_active_surfaces_if_needed();
            assert!(
                matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
                "controller reconciliation must not reap while another topology transaction holds the surface guard"
            );

            drop(surface_guard);
            controller.spawn_active_surfaces_if_needed();
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                GtkTerminalCommand::Close { surface_id, .. } if surface_id == "orphan-surface"
            ));
            parent.close();
        });
    }

    #[test]
    fn embedded_ghostty_view_wraps_surface_in_vertical_scroller() {
        let _ = crate::test_env::with_gtk_test(|| {
            let surface = gtk::TextView::new().upcast::<gtk::Widget>();
            let view = build_embedded_ghostty_scroll_view(&surface, GhosttyScrollbarPolicy::System);
            let scroller = view
                .downcast::<gtk::ScrolledWindow>()
                .expect("embedded view should be a scrolled window");

            assert_eq!(
                scroller.policy(),
                (gtk::PolicyType::Never, gtk::PolicyType::Automatic)
            );
            assert!(scroller.property::<bool>("overlay-scrolling"));
            assert_eq!(scroller.child().as_ref(), Some(&surface));
            assert!(surface.property::<bool>("hexpand"));
            assert!(surface.property::<bool>("vexpand"));

            let hidden_surface = gtk::TextView::new().upcast::<gtk::Widget>();
            let hidden =
                build_embedded_ghostty_scroll_view(&hidden_surface, GhosttyScrollbarPolicy::Never)
                    .downcast::<gtk::ScrolledWindow>()
                    .expect("hidden policy still uses a scrolled window");
            assert_eq!(
                hidden.policy(),
                (gtk::PolicyType::Never, gtk::PolicyType::Never)
            );
        });
    }
}
