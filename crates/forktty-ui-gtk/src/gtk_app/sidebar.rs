use super::*;

pub(super) struct SidebarWorkspaceRow {
    workspace: forktty_core::Workspace,
    meta: String,
    summary: String,
    status: Option<WorkspaceStatusBadge>,
    surface_count: usize,
}

#[derive(Clone)]
pub(super) struct WorkspaceStatusBadge {
    pub(super) label: &'static str,
    pub(super) tooltip: &'static str,
    pub(super) class_name: &'static str,
}

pub(super) struct SidebarSnapshot {
    rows: Vec<SidebarWorkspaceRow>,
    active_workspace_name: Option<String>,
    active_status_label: Option<String>,
    active_full_path: Option<String>,
    active_pane_label: Option<String>,
    signature: String,
}

#[derive(Clone)]
pub(super) struct SidebarUi {
    pub(super) sidebar: gtk::ListBox,
    pub(super) parent_window: adw::ApplicationWindow,
    pub(super) workspace_title: gtk::Button,
    pub(super) workspace_title_label: gtk::Label,
    pub(super) status_location: gtk::Button,
    pub(super) status_location_label: gtk::Label,
    pub(super) pane_status: gtk::Label,
    pub(super) last_signature: Rc<RefCell<Option<String>>>,
    pub(super) context_menu_open: Rc<Cell<bool>>,
    pub(super) context_popover: Rc<RefCell<Option<gtk::Popover>>>,
}

pub(super) fn schedule_sidebar_refresh(
    ui: SidebarUi,
    state: SocketAppState,
    controller: Rc<RefCell<VteController>>,
) {
    glib::idle_add_local_once(move || {
        refresh_sidebar(&ui, &state, &controller, true);
    });
}

fn install_workspace_reorder_dnd<W>(
    handle: &W,
    workspace_id: &str,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
    ui: &SidebarUi,
) where
    W: IsA<gtk::Widget>,
{
    install_internal_drag_source(
        handle,
        prefixed_dnd_payload(DND_WORKSPACE_PREFIX, workspace_id),
    );
    let target_workspace_id = workspace_id.to_string();
    let state_for_drop = state.clone();
    let controller_for_drop = controller.clone();
    let ui_for_drop = ui.clone();
    let handle_for_drop = handle.as_ref().clone();
    handle.add_controller(internal_drop_target(
        DND_WORKSPACE_PREFIX,
        move |payload, _x, y| {
            let Some(source_workspace_id) = dnd_payload_id(&payload, DND_WORKSPACE_PREFIX) else {
                return false;
            };
            if source_workspace_id == target_workspace_id {
                return false;
            }
            let position = drop_position(y, handle_for_drop.allocated_height());
            let moved = state_for_drop.model.lock().ok().is_some_and(|mut model| {
                model.move_workspace(&source_workspace_id, &target_workspace_id, position)
            });
            if moved {
                close_sidebar_context_menu(&ui_for_drop);
                save_session_from_state(&state_for_drop);
                schedule_sidebar_refresh(
                    ui_for_drop.clone(),
                    state_for_drop.clone(),
                    controller_for_drop.clone(),
                );
            }
            moved
        },
    ));
}

pub(super) fn refresh_sidebar(
    ui: &SidebarUi,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
    force: bool,
) {
    let snapshot = sidebar_snapshot(state);
    if let Some(name) = snapshot.active_workspace_name.as_deref() {
        ui.workspace_title_label.set_label(name);
        ui.workspace_title.set_tooltip_text(None);
        set_accessible_button_text(
            &ui.workspace_title,
            &format!("Active workspace: {name}"),
            None,
        );
        ui.workspace_title.set_sensitive(true);
    } else {
        ui.workspace_title_label.set_label("No workspace");
        ui.workspace_title
            .set_tooltip_text(Some("No active workspace"));
        set_accessible_button_text(&ui.workspace_title, "No active workspace", None);
        ui.workspace_title.set_sensitive(false);
    }
    if let Some(label) = snapshot.active_status_label.as_deref() {
        ui.status_location_label.set_label(label);
        if let Some(path) = snapshot.active_full_path.as_deref() {
            ui.status_location
                .set_tooltip_text(Some(&format!("Workspace location: {path}")));
        } else {
            ui.status_location
                .set_tooltip_text(Some("Workspace location"));
        }
        set_accessible_button_text(
            &ui.status_location,
            &format!("Workspace location: {label}"),
            None,
        );
        ui.status_location.set_sensitive(true);
    } else {
        ui.status_location_label.set_label("");
        ui.status_location
            .set_tooltip_text(Some("No active workspace location"));
        ui.status_location
            .update_property(&[gtk::accessible::Property::Label(
                "No active workspace location",
            )]);
        ui.status_location.set_sensitive(false);
    }
    if let Some(label) = snapshot.active_pane_label.as_deref() {
        ui.pane_status.set_label(label);
        ui.pane_status.set_tooltip_text(Some(label));
        ui.pane_status.set_visible(true);
    } else {
        ui.pane_status.set_label("");
        ui.pane_status.set_tooltip_text(None);
        ui.pane_status.set_visible(false);
    }

    if !force {
        if ui.context_menu_open.get() {
            return;
        }
        if ui.last_signature.borrow().as_deref() == Some(snapshot.signature.as_str()) {
            return;
        }
    }
    *ui.last_signature.borrow_mut() = Some(snapshot.signature.clone());

    while let Some(child) = ui.sidebar.first_child() {
        ui.sidebar.remove(&child);
    }
    if snapshot.rows.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
        empty.add_css_class("sidebar-empty");
        empty.set_halign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("forktty-folder-symbolic");
        icon.set_pixel_size(24);
        let title = gtk::Label::builder().label("No Workspaces").build();
        title.add_css_class("sidebar-empty-title");
        let body = gtk::Label::builder()
            .label("Create a workspace to start a terminal.")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        body.add_css_class("sidebar-empty-body");
        let create = gtk::Button::builder()
            .label("New Workspace")
            .has_frame(true)
            .build();
        create.add_css_class("suggested-action");
        create.set_action_name(Some("app.new-workspace"));
        set_accessible_button_text(&create, "New Workspace", Some("Ctrl+Shift+N"));
        empty.append(&icon);
        empty.append(&title);
        empty.append(&body);
        empty.append(&create);
        row.set_child(Some(&empty));
        ui.sidebar.append(&row);
        return;
    }
    for row_data in snapshot.rows {
        let SidebarWorkspaceRow {
            workspace,
            meta,
            summary,
            status,
            surface_count,
        } = row_data;
        let row = gtk::ListBoxRow::new();
        row.set_selectable(true);
        row.set_activatable(true);
        row.add_css_class("workspace-row");
        let mut accessible_label = if workspace.active {
            format!("Current workspace {}. {}", workspace.name, meta)
        } else {
            format!("Workspace {}. {}", workspace.name, meta)
        };
        if surface_count > 1 {
            accessible_label.push_str(&format!(". {surface_count} panes"));
        }
        if let Some(status) = status.as_ref() {
            accessible_label.push_str(&format!(". {}", status.tooltip));
        }
        if !summary.is_empty() {
            accessible_label.push_str(&format!(". {summary}"));
        }
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        if workspace.active {
            row.add_css_class("active");
        }
        if workspace.needs_attention {
            row.add_css_class("needs-attention");
        }

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .hexpand(true)
            .build();
        card.add_css_class("workspace-card");
        install_workspace_reorder_dnd(&card, &workspace.id, state, controller, ui);

        let grip = gtk::Image::from_icon_name("forktty-menu-symbolic");
        grip.add_css_class("workspace-drag-grip");
        grip.set_tooltip_text(Some("Drag to reorder"));
        card.append(&grip);

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        let name = gtk::Label::builder()
            .label(&workspace.name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("workspace-name");

        let name_line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .hexpand(true)
            .build();
        name_line.add_css_class("workspace-name-line");
        name_line.append(&name);

        if let Some(status) = status.as_ref() {
            let status_badge = gtk::Label::builder()
                .label(status.label)
                .xalign(0.5)
                .build();
            status_badge.add_css_class("workspace-status-badge");
            status_badge.add_css_class(status.class_name);
            status_badge.set_tooltip_text(Some(status.tooltip));
            name_line.append(&status_badge);
        }

        let meta_label = gtk::Label::builder()
            .label(&meta)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        meta_label.add_css_class("workspace-meta");

        text.append(&name_line);
        text.append(&meta_label);

        if !summary.is_empty() {
            let summary_label = gtk::Label::builder()
                .label(&summary)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            summary_label.add_css_class("workspace-summary");
            text.append(&summary_label);
        }

        card.append(&text);

        if surface_count > 1 {
            let count_badge = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            count_badge.add_css_class("workspace-count-badge");
            count_badge.set_valign(gtk::Align::Center);
            let count_icon = gtk::Image::from_icon_name("forktty-grid-symbolic");
            count_icon.add_css_class("workspace-count-icon");
            count_icon.set_tooltip_text(Some(&format!("{surface_count} panes")));
            let count_label = gtk::Label::new(Some(&surface_count.to_string()));
            count_label.add_css_class("workspace-count");
            count_label.set_tooltip_text(Some(&format!("{surface_count} panes")));
            count_badge.append(&count_icon);
            count_badge.append(&count_label);
            card.append(&count_badge);
        }

        let mut tooltip = format!("{}\n{}", workspace.name, meta);
        if let Some(status) = status.as_ref() {
            tooltip.push('\n');
            tooltip.push_str(status.tooltip);
        }
        if surface_count > 1 {
            tooltip.push('\n');
            tooltip.push_str(&format!("{surface_count} panes"));
        }
        if !summary.is_empty() {
            tooltip.push('\n');
            tooltip.push_str(&summary);
        }
        row.set_tooltip_text(Some(&tooltip));

        row.set_child(Some(&card));

        let primary_click = gtk::GestureClick::new();
        primary_click.set_button(gtk::gdk::BUTTON_PRIMARY);
        primary_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let workspace_id_for_click = workspace.id.clone();
        let state_for_click = state.clone();
        let controller_for_click = controller.clone();
        let ui_for_click = ui.clone();
        let row_for_click = row.clone();
        primary_click.connect_released(move |_, _n_press, _x, _y| {
            close_sidebar_context_menu(&ui_for_click);
            ui_for_click.sidebar.select_row(Some(&row_for_click));
            select_sidebar_workspace(
                &state_for_click,
                &workspace_id_for_click,
                &controller_for_click,
            );
            schedule_sidebar_refresh(
                ui_for_click.clone(),
                state_for_click.clone(),
                controller_for_click.clone(),
            );
        });
        row.add_controller(primary_click);

        let key = gtk::EventControllerKey::new();
        let workspace_id_for_key = workspace.id.clone();
        let state_for_key = state.clone();
        let controller_for_key = controller.clone();
        let ui_for_key = ui.clone();
        let row_for_key = row.clone();
        key.connect_key_pressed(move |_, key, _, _| {
            let should_activate = matches!(
                key,
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter | gtk::gdk::Key::space
            );
            if !should_activate {
                return glib::Propagation::Proceed;
            }
            close_sidebar_context_menu(&ui_for_key);
            ui_for_key.sidebar.select_row(Some(&row_for_key));
            select_sidebar_workspace(&state_for_key, &workspace_id_for_key, &controller_for_key);
            schedule_sidebar_refresh(
                ui_for_key.clone(),
                state_for_key.clone(),
                controller_for_key.clone(),
            );
            glib::Propagation::Stop
        });
        row.add_controller(key);

        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let state_for_menu = state.clone();
        let controller_for_menu = controller.clone();
        let parent_for_menu = ui.parent_window.clone();
        let workspace_for_menu = workspace.clone();
        let row_for_menu = row.clone();
        let ui_for_menu = ui.clone();
        gesture.connect_pressed(move |gesture, _n_press, x, y| {
            // Claim the sequence so ListBox doesn't also treat right-click as
            // row activation (would switch workspace under the menu).
            gesture.set_state(gtk::EventSequenceState::Claimed);
            close_sidebar_context_menu(&ui_for_menu);
            ui_for_menu.context_menu_open.set(true);
            let popover = build_workspace_context_menu(
                &parent_for_menu,
                &state_for_menu,
                &controller_for_menu,
                &workspace_for_menu,
            );
            let ui_for_closed = ui_for_menu.clone();
            let state_for_closed = state_for_menu.clone();
            let controller_for_closed = controller_for_menu.clone();
            popover.connect_closed(move |popover| {
                let is_current = ui_for_closed
                    .context_popover
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| current == popover);
                if is_current {
                    ui_for_closed.context_menu_open.set(false);
                    ui_for_closed.context_popover.borrow_mut().take();
                    schedule_sidebar_refresh(
                        ui_for_closed.clone(),
                        state_for_closed.clone(),
                        controller_for_closed.clone(),
                    );
                }
                if popover.parent().is_some() {
                    popover.unparent();
                }
            });
            popover.set_parent(&row_for_menu);
            popover.set_position(gtk::PositionType::Bottom);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            *ui_for_menu.context_popover.borrow_mut() = Some(popover.clone());
            popover.popup();
        });
        row.add_controller(gesture);

        let select_row = workspace.active;
        ui.sidebar.append(&row);
        if select_row {
            ui.sidebar.select_row(Some(&row));
        }
    }
}

pub(super) fn sidebar_snapshot(state: &SocketAppState) -> SidebarSnapshot {
    let Ok(model) = state.model.lock() else {
        return SidebarSnapshot {
            rows: Vec::new(),
            active_workspace_name: None,
            active_status_label: None,
            active_full_path: None,
            active_pane_label: None,
            signature: "lock-poisoned".to_string(),
        };
    };
    let active_workspace_id = model.active_workspace_id();
    let notifications = model.list_notifications();
    let mut rows = Vec::new();
    for workspace in model.list_workspaces() {
        let statuses = model.list_status(&workspace.id);
        let progress = model.list_progress(&workspace.id);
        let logs = model.list_logs(&workspace.id);
        let latest_attention_notification = workspace
            .needs_attention
            .then(|| {
                notifications
                    .iter()
                    .rev()
                    .find(|notification| {
                        notification_targets_workspace(&model, notification, &workspace.id)
                    })
                    .cloned()
            })
            .flatten();
        let status = workspace_status_badge(
            &workspace,
            &statuses,
            &progress,
            latest_attention_notification.as_ref(),
        );
        let summary = format_workspace_activity_summary(
            &statuses,
            &progress,
            logs.first(),
            latest_attention_notification.as_ref(),
        );
        let surfaces = model.list_surfaces(Some(&workspace.id));
        let surface_count = surfaces.len();
        let ssh_host = surfaces.iter().find_map(|s| {
            if let forktty_core::SurfaceKind::Ssh { host } = &s.kind {
                Some(host.clone())
            } else {
                None
            }
        });
        let meta = workspace_meta_line(&workspace, ssh_host.as_deref());
        rows.push(SidebarWorkspaceRow {
            workspace,
            meta,
            summary,
            status,
            surface_count,
        });
    }
    let active_workspace = rows
        .iter()
        .map(|row| &row.workspace)
        .find(|workspace| active_workspace_id.as_deref() == Some(workspace.id.as_str()));
    let active_workspace_name = active_workspace.map(|workspace| workspace.name.clone());
    let active_status_label = active_workspace.map(|workspace| {
        let cwd = model
            .surface(&workspace.focused_surface_id)
            .map(|surface| compact_path(&surface.cwd))
            .unwrap_or_else(|| compact_path(&workspace.working_dir));
        format!("{} · {}", workspace.name, cwd)
    });
    let active_full_path = active_workspace.map(|workspace| {
        model
            .surface(&workspace.focused_surface_id)
            .map(|surface| surface.cwd.to_string_lossy().to_string())
            .unwrap_or_else(|| workspace.working_dir.to_string_lossy().to_string())
    });
    let active_pane_label = active_workspace.and_then(|workspace| {
        let panes = collect_panes(&workspace.pane_tree);
        let pane_count = panes.len();
        let index = panes
            .iter()
            .position(|surface_id| surface_id == &workspace.focused_surface_id)?;
        let surface = model.surface(&workspace.focused_surface_id);
        let title = surface
            .map(surface_title)
            .unwrap_or_else(|| "Terminal".to_string());
        let compact_cwd = surface
            .map(|surface| compact_path(&surface.cwd))
            .unwrap_or_else(|| compact_path(&workspace.working_dir));
        let full_cwd = surface
            .map(|surface| surface.cwd.to_string_lossy().to_string())
            .unwrap_or_else(|| workspace.working_dir.to_string_lossy().to_string());
        let title_is_cwd_echo = title == compact_cwd.as_str()
            || title == full_cwd.as_str()
            || full_cwd.ends_with(&format!("/{title}"));
        if pane_count <= 1 {
            // With a single pane the "Pane 1/1" prefix is noise; the cwd is
            // already shown in status_location. Only surface a distinct title.
            if title == "Terminal" || title_is_cwd_echo {
                return None;
            }
            return Some(title);
        }
        if title == "Terminal" || title_is_cwd_echo {
            return Some(format!("Pane {}/{}", index + 1, pane_count));
        }
        Some(format!("Pane {}/{} · {}", index + 1, pane_count, title))
    });
    let mut signature = format!(
        "active={:?};status={:?};path={:?};pane={:?};rows={};",
        active_workspace_id,
        active_status_label,
        active_full_path,
        active_pane_label,
        rows.len()
    );
    for row in &rows {
        signature.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?};",
            row.workspace.id,
            row.workspace.name,
            row.workspace.active,
            row.workspace.needs_attention,
            row.workspace.working_dir.to_string_lossy(),
            row.workspace.worktree_name.as_deref().unwrap_or(""),
            row.surface_count,
            row.meta,
            row.summary,
            row.status
                .as_ref()
                .map(|status| (status.label, status.class_name))
        ));
    }
    SidebarSnapshot {
        rows,
        active_workspace_name,
        active_status_label,
        active_full_path,
        active_pane_label,
        signature,
    }
}

pub(super) fn workspace_meta_line(
    workspace: &forktty_core::Workspace,
    ssh_host: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if let Some(host) = ssh_host {
        parts.push(format!("ssh:{host}"));
    }
    if !workspace.git_branch.trim().is_empty() {
        parts.push(workspace.git_branch.clone());
    }
    if let Some(worktree) = workspace.worktree_name.as_deref() {
        if !worktree.trim().is_empty() {
            parts.push(format!("wt:{worktree}"));
        }
    }
    if let Some(pr) = workspace.pr.as_ref() {
        parts.push(pr.summary());
    }
    parts.push(compact_path(&workspace.working_dir));
    if !workspace.listening_ports.is_empty() {
        let ports = workspace
            .listening_ports
            .iter()
            .map(|port| format!(":{port}"))
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(ports);
    }
    parts.join(" · ")
}

pub(super) fn select_sidebar_workspace(
    state: &SocketAppState,
    workspace_id: &str,
    controller: &Rc<RefCell<VteController>>,
) {
    if let Err(err) = select_workspace_with_terminal(state, workspace_id) {
        eprintln!("Failed to spawn selected workspace terminal: {err}");
        create_global_notification(
            state,
            "Workspace Switch Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
    }
    controller.borrow_mut().rebuild_layout();
}

pub(super) fn close_sidebar_context_menu(ui: &SidebarUi) {
    let popover = ui.context_popover.borrow_mut().take();
    if let Some(popover) = popover {
        popover.popdown();
        if popover.parent().is_some() {
            popover.unparent();
        }
    }
    ui.context_menu_open.set(false);
}

pub(super) fn focus_relative_pane(state: &SocketAppState, delta: isize) -> bool {
    let mut model = match state.model.lock() {
        Ok(model) => model,
        Err(_) => return false,
    };
    let Some(workspace) = model.active_workspace() else {
        return false;
    };
    let panes = collect_panes(&workspace.pane_tree);
    if panes.len() < 2 {
        return false;
    }
    let current = panes
        .iter()
        .position(|surface_id| surface_id == &workspace.focused_surface_id)
        .unwrap_or(0);
    let len = panes.len() as isize;
    let next = (current as isize + delta).rem_euclid(len) as usize;
    model.focus_surface(&panes[next])
}

pub(super) fn notification_targets_workspace(
    model: &WorkspaceModel,
    notification: &NotificationItem,
    workspace_id: &str,
) -> bool {
    if notification.workspace_id.as_deref() == Some(workspace_id) {
        return true;
    }
    notification
        .surface_id
        .as_deref()
        .and_then(|surface_id| model.surface(surface_id))
        .map(|surface| surface.workspace_id == workspace_id)
        .unwrap_or(false)
}

pub(super) fn workspace_status_badge(
    workspace: &forktty_core::Workspace,
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
    latest_attention_notification: Option<&NotificationItem>,
) -> Option<WorkspaceStatusBadge> {
    if let Some(notification) = latest_attention_notification {
        if notification.kind == NotificationKind::Prompt {
            return Some(WorkspaceStatusBadge {
                label: "Input",
                tooltip: "Needs input",
                class_name: "needs-input",
            });
        }
        if notification.kind == NotificationKind::Error {
            return Some(WorkspaceStatusBadge {
                label: "Error",
                tooltip: "Error reported in this workspace",
                class_name: "error",
            });
        }
    }

    if statuses.iter().any(status_entry_suggests_error) {
        return Some(WorkspaceStatusBadge {
            label: "Error",
            tooltip: "Error reported in this workspace",
            class_name: "error",
        });
    }

    if statuses.iter().any(status_entry_suggests_exited) {
        return Some(WorkspaceStatusBadge {
            label: "Exited",
            tooltip: "Process exited",
            class_name: "exited",
        });
    }

    if latest_attention_notification.is_some() || workspace.needs_attention {
        return Some(WorkspaceStatusBadge {
            label: "Alert",
            tooltip: "Attention",
            class_name: "attention",
        });
    }

    if statuses.iter().any(status_entry_suggests_running) {
        return Some(WorkspaceStatusBadge {
            label: "Running",
            tooltip: "Process running",
            class_name: "running",
        });
    }

    if !progress.is_empty() {
        return Some(WorkspaceStatusBadge {
            label: "Working",
            tooltip: "Work in progress",
            class_name: "working",
        });
    }

    None
}

pub(super) fn surface_status_blocks_auto_spawn(statuses: &[StatusEntry], surface_id: &str) -> bool {
    let key = surface_status_key(surface_id);
    statuses.iter().any(|status| {
        status.key == key
            && (status_entry_suggests_error(status) || status_entry_suggests_exited(status))
    })
}

pub(super) fn status_entry_suggests_running(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    let color = status
        .color
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    value.contains("running")
        || value.contains("working")
        || value.contains("busy")
        || color == "blue"
}

pub(super) fn status_entry_suggests_error(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    let color = status
        .color
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    value.contains("error")
        || value.contains("failed")
        || value.contains("failure")
        || color == "red"
}

pub(super) fn status_entry_suggests_exited(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    let color = status
        .color
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    value.contains("exited") || value.contains("stopped") || color == "yellow" || color == "warning"
}

pub(super) fn format_workspace_activity_summary(
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
    latest_log: Option<&forktty_core::LogEntry>,
    latest_attention_notification: Option<&NotificationItem>,
) -> String {
    if let Some(notification) = latest_attention_notification {
        return format_notification_preview(notification);
    }
    format_metadata_summary(statuses, progress, latest_log)
}

pub(super) fn format_notification_preview(notification: &NotificationItem) -> String {
    let body = notification.body.trim();
    let text = if body.is_empty() {
        notification.title.trim().to_string()
    } else {
        format!("{}: {body}", notification.title.trim())
    };
    truncate_single_line(&text, 120)
}

pub(super) fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut truncated = collapsed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

pub(super) fn format_metadata_summary(
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
    latest_log: Option<&forktty_core::LogEntry>,
) -> String {
    if statuses.is_empty() && progress.is_empty() && latest_log.is_none() {
        return String::new();
    }
    let mut parts = statuses
        .iter()
        .map(|status| format!("{}: {}", status.label, status.value))
        .collect::<Vec<_>>();
    parts.extend(progress.iter().map(|progress| {
        let total = progress.total.unwrap_or(100.0);
        let percent = if total > 0.0 {
            (progress.value / total * 100.0).round().clamp(0.0, 100.0)
        } else {
            0.0
        };
        format!("{}: {percent:.0}%", progress.label)
    }));
    if let Some(log) = latest_log {
        parts.push(format!("{:?}: {}", log.level, log.message));
    }
    parts.join("  ·  ")
}
