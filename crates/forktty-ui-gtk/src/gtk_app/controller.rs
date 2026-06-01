use super::*;

pub(super) struct PaneChrome {
    pub(super) pane: gtk::Box,
    pub(super) header: gtk::Box,
    pub(super) single_pane_actions: gtk::Box,
    pub(super) focus_marker: gtk::Box,
    pub(super) title: gtk::Label,
    pub(super) cwd: gtk::Label,
    pub(super) attention_dot: gtk::Box,
}

pub(super) type SplitResizeCallback = Rc<dyn Fn(&[String], &[String], f64)>;
pub(super) type SettingsApplyCallback = Rc<dyn Fn(&config::AppConfig)>;

pub(super) struct VteController {
    container: gtk::Box,
    parent_window: adw::ApplicationWindow,
    pub(super) model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
    pub(super) widgets: BTreeMap<String, VteTerminalWidget>,
    chromes: BTreeMap<String, PaneChrome>,
    pending_spawns: BTreeSet<String>,
    last_layout_signature: Option<String>,
    maximized_pane: bool,
    /// Spawned child PID per surface, used to discover listening ports.
    pub(super) surface_pids: Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    next_spawn_token: u64,
    pane_tab_strips: Rc<RefCell<Vec<PaneTabStrip>>>,
    #[cfg(feature = "browser")]
    browser_panes: Rc<RefCell<BTreeMap<String, Rc<crate::browser_pane::BrowserPaneWidget>>>>,
}

#[derive(Clone)]
pub(super) struct PaneTabStrip {
    tabs: Vec<String>,
    stack: gtk::Stack,
    tab_widgets: Vec<gtk::Box>,
    labels: Vec<gtk::Label>,
    select_areas: Vec<gtk::Box>,
}

struct TabStripRefresh {
    active_id: Option<String>,
    tabs: Vec<TabRefresh>,
}

struct TabRefresh {
    title: String,
    active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SurfacePid {
    pub(super) pid: i32,
    pub(super) spawn_token: u64,
}

pub(super) fn remove_surface_pid_for_spawn(
    pids: &mut BTreeMap<String, SurfacePid>,
    surface_id: &str,
    spawn_token: u64,
) -> bool {
    if !matches!(
        pids.get(surface_id),
        Some(entry) if entry.spawn_token == spawn_token
    ) {
        return false;
    }
    pids.remove(surface_id);
    true
}

impl VteController {
    pub(super) fn new(
        container: gtk::Box,
        parent_window: adw::ApplicationWindow,
        model: Arc<Mutex<WorkspaceModel>>,
    ) -> Self {
        Self {
            container,
            parent_window,
            model,
            state: None,
            widgets: BTreeMap::new(),
            chromes: BTreeMap::new(),
            pending_spawns: BTreeSet::new(),
            last_layout_signature: None,
            maximized_pane: false,
            surface_pids: Rc::new(RefCell::new(BTreeMap::new())),
            next_spawn_token: 0,
            pane_tab_strips: Rc::new(RefCell::new(Vec::new())),
            #[cfg(feature = "browser")]
            browser_panes: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    pub(super) fn attach_state(&mut self, state: SocketAppState) {
        self.state = Some(state);
    }

    pub(super) fn handle(&mut self, command: GtkTerminalCommand) {
        match command {
            GtkTerminalCommand::Spawn(request) => self.spawn(request),
            GtkTerminalCommand::SendText { surface_id, text } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    vte_send_text(widget, &text);
                } else {
                    eprintln!("Dropped send-text for unready terminal surface: {surface_id}");
                }
            }
            GtkTerminalCommand::Resize {
                surface_id,
                cols,
                rows,
            } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    widget.set_size(cols.into(), rows.into());
                }
            }
            GtkTerminalCommand::Close { surface_id } => {
                if let Some(chrome) = self.chromes.remove(&surface_id) {
                    detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
                }
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

    fn spawn(&mut self, request: SpawnRequest) {
        if self.widgets.contains_key(&request.surface_id) {
            return;
        }
        self.pending_spawns.remove(&request.surface_id);
        let spawn_model = self.model.clone();
        let spawn_workspace_id = request.workspace_id.clone();
        let spawn_surface_id = request.surface_id.clone();
        let spawn_state_for_error = self.state.clone();
        let spawn_state_for_ready = self.state.clone();
        let spawn_model_for_error = spawn_model.clone();
        let spawn_pids = self.surface_pids.clone();
        let spawn_pid_surface_id = request.surface_id.clone();
        self.next_spawn_token = self.next_spawn_token.checked_add(1).unwrap_or(1);
        let spawn_token = self.next_spawn_token;
        match spawn_vte_terminal_with_callback(&request, move |result| match result {
            Ok(pid) => {
                spawn_pids.borrow_mut().insert(
                    spawn_pid_surface_id.clone(),
                    SurfacePid {
                        pid: pid.0,
                        spawn_token,
                    },
                );
                if let Some(state) = &spawn_state_for_ready {
                    if let Err(err) = state.terminal.mark_surface_ready(&spawn_surface_id) {
                        eprintln!(
                            "Failed to mark terminal surface ready {}: {err}",
                            spawn_surface_id
                        );
                    }
                }
            }
            Err(err) => {
                record_terminal_spawn_failure(
                    &spawn_model,
                    &spawn_workspace_id,
                    &spawn_surface_id,
                    &err.to_string(),
                    spawn_state_for_error
                        .as_ref()
                        .is_none_or(|state| state.notification_dispatch),
                );
                if let Some(state) = &spawn_state_for_error {
                    let _ = state.terminal.close(&spawn_surface_id);
                }
            }
        }) {
            Ok(widget) => {
                if let Ok(mut model) = self.model.lock() {
                    let _ = model.clear_status(
                        &request.workspace_id,
                        Some(&surface_status_key(&request.surface_id)),
                    );
                }
                apply_vte_appearance(&widget);
                attach_vte_signal_handlers(
                    &widget,
                    &self.model,
                    &request,
                    &self.surface_pids,
                    self.state.clone(),
                    spawn_token,
                );
                let chrome = build_pane_chrome(
                    &request.surface_id,
                    &widget,
                    self.state.as_ref(),
                    &self.parent_window,
                );
                self.chromes.insert(request.surface_id.clone(), chrome);
                self.widgets.insert(request.surface_id, widget);
                self.rebuild_layout();
            }
            Err(err) => {
                record_terminal_spawn_failure(
                    &spawn_model_for_error,
                    &request.workspace_id,
                    &request.surface_id,
                    &err.to_string(),
                    self.state
                        .as_ref()
                        .is_none_or(|state| state.notification_dispatch),
                );
                if let Some(state) = &self.state {
                    let _ = state.terminal.close(&request.surface_id);
                }
                self.last_layout_signature = None;
                self.rebuild_layout();
            }
        }
    }

    pub(super) fn rebuild_layout(&mut self) {
        self.spawn_active_surfaces_if_needed();
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
            self.last_layout_signature = Some(EMPTY_LAYOUT_SIGNATURE.to_string());
            self.container.append(&empty_terminal_stage(
                self.state.as_ref(),
                Some(&self.parent_window),
            ));
            return;
        };
        let visible_tree = if self.maximized_pane {
            PaneNode::single_leaf(focused_surface_id.clone())
        } else {
            pane_tree
        };
        let widget = self.widget_for_pane(&visible_tree, &workspace_id);
        let single_pane = collect_panes(&visible_tree).len() == 1;
        for chrome in self.chromes.values() {
            chrome.header.set_visible(!single_pane);
            chrome.single_pane_actions.set_visible(single_pane);
            chrome.single_pane_actions.set_sensitive(single_pane);
        }
        self.container.append(&widget);
        self.queue_focus_for_surface(&focused_surface_id);
        self.last_layout_signature = Some(signature);
    }

    pub(super) fn toggle_maximized_pane(&mut self) {
        self.maximized_pane = !self.maximized_pane;
        self.last_layout_signature = None;
        self.rebuild_layout();
    }

    fn model_focused_widget(&self) -> Option<VteTerminalWidget> {
        let surface_id = {
            let model = self.model.lock().ok()?;
            model.active_workspace()?.focused_surface_id
        };
        self.widgets.get(&surface_id).cloned()
    }

    fn gtk_focused_widget(&self) -> Option<VteTerminalWidget> {
        self.widgets
            .values()
            .find(|widget| widget.has_focus())
            .cloned()
    }

    // App-wide clipboard accelerators must only affect a terminal that currently
    // owns GTK focus; the model focus can legitimately be stale while dialogs or
    // search entries are active.
    pub(super) fn copy_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.copy_clipboard_format(Format::Text);
        true
    }

    pub(super) fn paste_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.paste_clipboard();
        true
    }

    pub(super) fn select_all_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.select_all();
        true
    }

    pub(super) fn reset_focused_terminal(&self) -> bool {
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        reset_and_redraw_terminal(&widget);
        true
    }

    // Explicit commands from the command palette intentionally target the active
    // terminal, because the palette itself owns GTK focus while the user chooses.
    pub(super) fn copy_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.copy_clipboard_format(Format::Text);
        true
    }

    pub(super) fn paste_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.paste_clipboard();
        true
    }

    pub(super) fn select_all_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.select_all();
        true
    }

    pub(super) fn reset_active_terminal(&self) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        reset_and_redraw_terminal(&widget);
        true
    }

    fn queue_focus_for_surface(&self, surface_id: &str) {
        if let Some(widget) = self.widgets.get(surface_id) {
            queue_widget_focus(widget.clone().upcast());
        } else {
            // Browser panes are not in self.widgets; hand keyboard focus to the
            // pane's focus target so keyboard-only nav reaches the browser.
            #[cfg(feature = "browser")]
            if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
                queue_widget_focus(pane.focus_target());
            }
        }
    }

    pub(super) fn sync_model_focus_to_ui(&mut self) {
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

    pub(super) fn ensure_layout_current(&mut self) {
        self.spawn_active_surfaces_if_needed();
        let Some((signature, _, _, _)) = active_layout_snapshot(&self.model) else {
            if self.last_layout_signature.as_deref() != Some(EMPTY_LAYOUT_SIGNATURE) {
                self.rebuild_layout();
            }
            return;
        };
        if self.last_layout_signature.as_deref() != Some(signature.as_str()) {
            self.rebuild_layout();
        } else {
            self.refresh_chromes();
        }
    }

    fn spawn_active_surfaces_if_needed(&mut self) {
        let Some(state) = self.state.clone() else {
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
                    let blocked = surface_status_blocks_auto_spawn(&statuses, &surface.id);
                    (surface, blocked)
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
        // A surface restarted or closed on the GTK thread while a socket client
        // concurrently removed it from the model can leave a backend terminal
        // (PTY + widget) with no model counterpart. Tear those orphans down;
        // the queued Close event drops the widget on the next rebuild.
        for surface_id in
            orphaned_backend_surfaces(&backend_surface_ids, &model_surface_ids, &self.pending_spawns)
        {
            match state.terminal.close(&surface_id) {
                Ok(()) | Err(TerminalError::NotFound(_)) => {}
                Err(err) => {
                    eprintln!("Failed to reap orphaned terminal surface {surface_id}: {err}");
                }
            }
        }
        for (surface, auto_spawn_blocked) in surfaces {
            if auto_spawn_blocked {
                continue;
            }
            if self.widgets.contains_key(&surface.id)
                || self.pending_spawns.contains(&surface.id)
                || backend_surface_ids.contains(&surface.id)
            {
                continue;
            }
            // Browser surfaces are rendered by browser_panes and never get a
            // terminal backend; spawning a PTY for them would leak a hidden
            // shell process and emit bogus terminal status/port/close events.
            // Ssh surfaces rewrite the request to launch ssh <host>; this is
            // what respawns restored remote workspaces on session restore.
            let base =
                SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
            let Some(request) = forktty_socket::spawn_request_for_surface_kind(base, &surface.kind)
            else {
                continue;
            };
            let surface_id = surface.id.clone();
            let workspace_id = surface.workspace_id.clone();
            self.pending_spawns.insert(surface_id.clone());
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

    fn refresh_chromes(&self) {
        let Ok(model) = self.model.lock() else {
            return;
        };
        let focused_surface_id = model
            .active_workspace()
            .map(|workspace| workspace.focused_surface_id);
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
        let tab_strip_tabs = self
            .pane_tab_strips
            .borrow()
            .iter()
            .map(|strip| strip.tabs.clone())
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
            if let Some(chrome) = self.chromes.get(&surface_id) {
                update_pane_chrome(chrome, &surface, active);
            }
        }
        self.refresh_tab_strips(&tab_updates);
        // Push the latest url into the live webview on each refresh tick, and
        // drop panes for surfaces that no longer exist to avoid leaking them.
        #[cfg(feature = "browser")]
        self.browser_panes.borrow_mut().retain(|surface_id, pane| {
            if let Some((url, active)) = browser_targets.get(surface_id) {
                // Safe to call every tick: BrowserPaneWidget edge-triggers on the
                // last *requested* url, so an unchanged url is a no-op.
                pane.load_uri(url);
                pane.set_active(*active);
                true
            } else {
                false
            }
        });
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
            if previous_visible.as_deref() != Some(active_id.as_str()) {
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
                let active_id = &tabs[*active];
                if tabs.len() == 1 {
                    self.pane_widget_for(active_id)
                } else {
                    self.leaf_widget_with_tabstrip(tabs, *active)
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

        let tabstrip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabstrip.add_css_class("pane-tabstrip");
        tabstrip.set_hexpand(true);
        let mut tab_widgets = Vec::with_capacity(tabs.len());
        let mut labels = Vec::with_capacity(tabs.len());
        let mut select_areas = Vec::with_capacity(tabs.len());

        for (idx, (surface_id, title)) in tabs.iter().zip(titles.iter()).enumerate() {
            let tab = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            tab.add_css_class("pane-tab");
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

            let select = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            select.set_hexpand(true);
            select.set_valign(gtk::Align::Center);
            select.set_focusable(true);
            select.add_css_class("pane-tab-select");
            let grip = gtk::Image::from_icon_name("forktty-menu-symbolic");
            grip.add_css_class("pane-tab-grip");
            grip.set_tooltip_text(Some("Drag Tab"));
            select.append(&grip);
            select.append(&label);
            update_tab_tooltip(&select, Some(title.clone()));
            select.update_property(&[gtk::accessible::Property::Label(&format!(
                "Select tab {title}"
            ))]);

            let model_for_select = self.model.clone();
            let select_id = surface_id.clone();
            let primary_click = gtk::GestureClick::new();
            primary_click.set_button(gtk::gdk::BUTTON_PRIMARY);
            primary_click.connect_released(move |_, _n_press, _x, _y| {
                if let Ok(mut m) = model_for_select.lock() {
                    let _ = m.select_tab(&select_id);
                }
            });
            select.add_controller(primary_click);

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
            install_tab_drag_source(&tab, surface_id);
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
        outer.append(&tabstrip);

        // Keep tab children mounted and switch the visible child in-place so a
        // tab change does not rebuild the surrounding split tree.
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::None);
        for surface_id in tabs {
            let pane_widget = self.pane_widget_for(surface_id);
            stack.add_named(&pane_widget, Some(surface_id.as_str()));
        }
        let active_id = &tabs[active.min(tabs.len().saturating_sub(1))];
        stack.set_visible_child_name(active_id.as_str());
        outer.append(&stack);

        self.pane_tab_strips.borrow_mut().push(PaneTabStrip {
            tabs: tabs.to_vec(),
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
        // the VTE has-focus handler, so focus-driven split/close target this pane.
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
/// Surfaces with an in-flight spawn are excluded because their backend entry
/// exists before the matching `Spawn` command commits the widget.
pub(super) fn orphaned_backend_surfaces(
    backend_ids: &BTreeSet<String>,
    model_ids: &BTreeSet<String>,
    pending: &BTreeSet<String>,
) -> Vec<String> {
    backend_ids
        .iter()
        .filter(|id| !model_ids.contains(*id) && !pending.contains(*id))
        .cloned()
        .collect()
}

fn tab_strip_refreshes(model: &WorkspaceModel, strip_tabs: &[Vec<String>]) -> Vec<TabStripRefresh> {
    let workspace = model.active_workspace();
    strip_tabs
        .iter()
        .map(|tabs| {
            let active_id = workspace
                .as_ref()
                .and_then(|workspace| active_tab_for_tabs(&workspace.pane_tree, tabs));
            let tabs = tabs
                .iter()
                .map(|surface_id| {
                    let title = model
                        .surface(surface_id)
                        .map(surface_title)
                        .unwrap_or_else(|| "Terminal".to_string());
                    let active = active_id.as_deref() == Some(surface_id.as_str());
                    TabRefresh { title, active }
                })
                .collect();
            TabStripRefresh { active_id, tabs }
        })
        .collect()
}

fn install_tabstrip_drop_target(
    tabstrip: &gtk::Box,
    tab_widgets: &[gtk::Box],
    surface_ids: &[String],
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
) {
    let strip_for_drop = tabstrip.clone().upcast::<gtk::Widget>();
    let tab_targets = surface_ids
        .iter()
        .cloned()
        .zip(
            tab_widgets
                .iter()
                .map(|tab| tab.clone().upcast::<gtk::Widget>()),
        )
        .collect::<Vec<_>>();

    install_tab_drop_target_on(&strip_for_drop, &strip_for_drop, &tab_targets, model, state);
}

fn install_tab_drop_target_on(
    handle: &gtk::Widget,
    tabstrip: &gtk::Widget,
    tab_targets: &[(String, gtk::Widget)],
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
) {
    let handle_for_drop = handle.clone();
    let strip_for_drop = tabstrip.clone();
    let tab_targets = tab_targets.to_vec();
    let tab_order = tab_targets
        .iter()
        .map(|(surface_id, _)| surface_id.clone())
        .collect::<Vec<_>>();
    let tab_targets_for_drop = tab_targets.clone();
    let tab_order_for_drop = tab_order.clone();
    let tab_targets_for_motion = tab_targets.clone();
    let tab_order_for_motion = tab_order.clone();
    let handle_for_motion = handle.clone();
    let strip_for_motion = tabstrip.clone();
    let target = tab_drop_target(move |source_id, x, y| {
        clear_tab_drop_indicators(&tab_targets_for_drop);
        let Some((strip_x, _)) = handle_for_drop.translate_coordinates(&strip_for_drop, x, y)
        else {
            return false;
        };
        let midpoints = tab_drop_midpoints(&strip_for_drop, &tab_targets_for_drop);
        let Some((target_id, position)) =
            tab_drop_target_at_x(&midpoints, strip_x).filter(|(target_id, position)| {
                !tab_move_would_keep_order(&tab_order_for_drop, &source_id, target_id, *position)
            })
        else {
            return false;
        };
        let moved = model
            .lock()
            .ok()
            .is_some_and(|mut model| model.move_tab(&source_id, target_id, position));
        if moved {
            if let Some(state) = state.as_ref() {
                save_session_from_state(state);
            }
        }
        moved
    });
    target.set_preload(true);
    target.connect_motion(move |target, x, y| {
        let Some(source_id) = target
            .value()
            .and_then(|value| tab_dnd_id_from_value(&value))
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::MOVE;
        };
        let Some((strip_x, _)) = handle_for_motion.translate_coordinates(&strip_for_motion, x, y)
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::empty();
        };
        let midpoints = tab_drop_midpoints(&strip_for_motion, &tab_targets_for_motion);
        let Some((target_id, position)) =
            tab_drop_target_at_x(&midpoints, strip_x).filter(|(target_id, position)| {
                !tab_move_would_keep_order(&tab_order_for_motion, &source_id, target_id, *position)
            })
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::empty();
        };
        set_tab_drop_indicator(&tab_targets_for_motion, target_id, position);
        gdk::DragAction::MOVE
    });
    let tab_targets_for_leave = tab_targets.clone();
    target.connect_leave(move |_| {
        clear_tab_drop_indicators(&tab_targets_for_leave);
    });
    handle.add_controller(target);
}

fn tab_drop_midpoints(
    tabstrip: &gtk::Widget,
    tab_targets: &[(String, gtk::Widget)],
) -> Vec<(String, f64)> {
    tab_targets
        .iter()
        .filter_map(|(surface_id, tab)| {
            let (tab_x, _) = tab.translate_coordinates(tabstrip, 0.0, 0.0)?;
            Some((
                surface_id.clone(),
                tab_x + f64::from(tab.allocated_width()) / 2.0,
            ))
        })
        .collect()
}

pub(super) fn tab_drop_target_at_x(
    tab_midpoints: &[(String, f64)],
    x: f64,
) -> Option<(&str, forktty_core::MovePosition)> {
    for (surface_id, midpoint) in tab_midpoints {
        if x < *midpoint {
            return Some((surface_id.as_str(), forktty_core::MovePosition::Before));
        }
    }
    tab_midpoints
        .last()
        .map(|(surface_id, _)| (surface_id.as_str(), forktty_core::MovePosition::After))
}

pub(super) fn tab_move_would_keep_order(
    tab_order: &[String],
    source_id: &str,
    target_id: &str,
    position: forktty_core::MovePosition,
) -> bool {
    if source_id == target_id {
        return true;
    }
    let source_index = tab_order
        .iter()
        .position(|surface_id| surface_id == source_id);
    let target_index = tab_order
        .iter()
        .position(|surface_id| surface_id == target_id);
    matches!(
        (source_index, target_index, position),
        (Some(source), Some(target), forktty_core::MovePosition::Before)
            if source + 1 == target
    ) || matches!(
        (source_index, target_index, position),
        (Some(source), Some(target), forktty_core::MovePosition::After)
            if target + 1 == source
    )
}

fn clear_tab_drop_indicators(tab_targets: &[(String, gtk::Widget)]) {
    for (_, tab) in tab_targets {
        tab.remove_css_class("drop-before");
        tab.remove_css_class("drop-after");
    }
}

fn set_tab_drop_indicator(
    tab_targets: &[(String, gtk::Widget)],
    target_id: &str,
    position: forktty_core::MovePosition,
) {
    clear_tab_drop_indicators(tab_targets);
    let Some((_, tab)) = tab_targets
        .iter()
        .find(|(surface_id, _)| surface_id == target_id)
    else {
        return;
    };
    match position {
        forktty_core::MovePosition::Before => tab.add_css_class("drop-before"),
        forktty_core::MovePosition::After => tab.add_css_class("drop-after"),
    }
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
    let tab_for_menu = tab.clone();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();
    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(popover) = current_popover_for_menu.borrow_mut().take() {
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
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-add-symbolic",
        "New Tab",
        false,
        move || add_new_tab_surface(&state_for_new_tab, &new_tab_id),
    );

    let state_for_split = state.clone();
    let split_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
        "Split Right",
        false,
        move || {
            focus_surface_and(&state_for_split, &split_id, |state| {
                split_active_surface(state, SplitAxis::Horizontal)
            });
        },
    );

    let state_for_split = state.clone();
    let split_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-vertical-symbolic",
        "Split Down",
        false,
        move || {
            focus_surface_and(&state_for_split, &split_id, |state| {
                split_active_surface(state, SplitAxis::Vertical)
            });
        },
    );

    add_context_menu_separator(&menu);

    let state_for_close = state.clone();
    let close_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-close-symbolic",
        "Close Tab",
        true,
        move || {
            close_tab_surface(&state_for_close, &close_id);
        },
    );

    popover.set_child(Some(&menu));
    popover
}
