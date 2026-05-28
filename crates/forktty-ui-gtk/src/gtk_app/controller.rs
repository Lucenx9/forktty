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
    select_buttons: Vec<gtk::Button>,
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
                self.browser_panes.borrow_mut().remove(&surface_id);
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
            self.browser_panes
                .borrow_mut()
                .retain(|surface_id, _| live_surface_ids.contains(surface_id));
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
        let surfaces = {
            let Ok(model) = self.model.lock() else {
                return;
            };
            let Some(workspace) = model.active_workspace() else {
                return;
            };
            let statuses = model.list_status(&workspace.id);
            model
                .list_surfaces(Some(&workspace.id))
                .into_iter()
                .map(|surface| {
                    let blocked = surface_status_blocks_auto_spawn(&statuses, &surface.id);
                    (surface, blocked)
                })
                .collect::<Vec<_>>()
        };
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
        for (surface_id, chrome) in &self.chromes {
            if let Some(surface) = model.surface(surface_id) {
                update_pane_chrome(
                    chrome,
                    surface,
                    focused_surface_id.as_deref() == Some(surface_id.as_str()),
                );
            }
        }
        self.refresh_tab_strips(&model);
        // Browser navigation only mutates a surface's url (same layout structure),
        // so the layout signature is unchanged and rebuild_layout does not fire.
        // Clone browser targets before touching live WebViews, then release the
        // model lock so WebKit load callbacks can safely sync redirects/clicks
        // back into the model.
        #[cfg(feature = "browser")]
        let browser_targets = model
            .list_surfaces(None)
            .into_iter()
            .filter_map(|surface| match surface.kind {
                forktty_core::SurfaceKind::Browser { url, .. } => Some((surface.id, url)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        drop(model);
        // Push the latest url into the live webview on each refresh tick, and
        // drop panes for surfaces that no longer exist to avoid leaking them.
        #[cfg(feature = "browser")]
        self.browser_panes.borrow_mut().retain(|surface_id, pane| {
            if let Some(url) = browser_targets.get(surface_id) {
                // Safe to call every tick: BrowserPaneWidget edge-triggers on the
                // last *requested* url, so an unchanged url is a no-op.
                pane.load_uri(url);
                true
            } else {
                false
            }
        });
    }

    fn refresh_tab_strips(&self, model: &WorkspaceModel) {
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        for strip in self.pane_tab_strips.borrow().iter() {
            let Some(active_id) = active_tab_for_tabs(&workspace.pane_tree, &strip.tabs) else {
                continue;
            };
            let previous_visible = strip
                .stack
                .visible_child_name()
                .map(|name| name.to_string());
            if previous_visible.as_deref() != Some(active_id.as_str()) {
                strip.stack.set_visible_child_name(active_id.as_str());
                self.queue_focus_for_surface(&active_id);
            }
            for (idx, surface_id) in strip.tabs.iter().enumerate() {
                if let Some(tab) = strip.tab_widgets.get(idx) {
                    if surface_id == &active_id {
                        tab.add_css_class("active");
                    } else {
                        tab.remove_css_class("active");
                    }
                    update_tab_tooltip(tab, model.surface(surface_id).map(surface_title));
                }
                if let Some(label) = strip.labels.get(idx) {
                    let title = model
                        .surface(surface_id)
                        .map(surface_title)
                        .unwrap_or_else(|| "Terminal".to_string());
                    label.set_label(&title);
                    if let Some(select) = strip.select_buttons.get(idx) {
                        update_tab_tooltip(select, Some(title.clone()));
                        set_accessible_button_text(select, &format!("Select tab {title}"), None);
                    }
                }
            }
        }
    }

    fn widget_for_pane(&self, node: &PaneNode, workspace_id: &str) -> gtk::Widget {
        let model = self.model.clone();
        let workspace_id_for_resize = workspace_id.to_string();
        let on_resize: SplitResizeCallback =
            Rc::new(move |left: &[String], right: &[String], ratio: f64| {
                if let Ok(mut model) = model.lock() {
                    let _ = model.update_split_partition_ratio(
                        &workspace_id_for_resize,
                        left,
                        right,
                        ratio,
                    );
                }
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
        let mut tab_widgets = Vec::with_capacity(tabs.len());
        let mut labels = Vec::with_capacity(tabs.len());
        let mut select_buttons = Vec::with_capacity(tabs.len());

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

            let select = gtk::Button::builder()
                .has_frame(false)
                .hexpand(true)
                .build();
            select.add_css_class("flat");
            select.add_css_class("pane-tab-select");
            select.set_child(Some(&label));
            update_tab_tooltip(&select, Some(title.clone()));
            set_accessible_button_text(&select, &format!("Select tab {title}"), None);

            let model_for_select = self.model.clone();
            let select_id = surface_id.clone();
            select.connect_clicked(move |_| {
                if let Ok(mut m) = model_for_select.lock() {
                    let _ = m.select_tab(&select_id);
                }
            });

            let close = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .has_frame(false)
                .build();
            close.add_css_class("flat");
            close.add_css_class("pane-tab-close");
            close.set_tooltip_text(Some("Close Tab"));
            set_accessible_button_text(&close, "Close Tab", None);

            let model_for_close = self.model.clone();
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
                    match state.terminal.close(&close_id) {
                        Ok(()) | Err(TerminalError::NotFound(_)) => {}
                        Err(err) => {
                            eprintln!("Failed to close tab terminal: {err}");
                        }
                    }
                }
                if let Ok(mut m) = model_for_close.lock() {
                    let _ = m.close_surface(&close_id);
                }
                if let Some(state) = &state_for_close {
                    save_session_from_state(state);
                }
            });

            tab.append(&select);
            tab.append(&close);
            tabstrip.append(&tab);
            tab_widgets.push(tab);
            labels.push(label);
            select_buttons.push(select);
        }
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
            select_buttons,
        });

        outer.upcast()
    }

    #[cfg(feature = "browser")]
    fn browser_pane_widget(&self, surface_id: &str, url: &str, profile_id: &str) -> gtk::Widget {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
            // Widget self-guards on the last requested url; calling unconditionally
            // is harmless and avoids the committed-uri divergence problem.
            pane.load_uri(url);
            return pane.widget();
        }
        let pane = Rc::new(crate::browser_pane::BrowserPaneWidget::new(profile_id, url));
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
