//! Workspace sidebar state, rows, popovers, and status/location summaries.

use super::*;
use std::collections::BTreeSet;
use std::path::Path;

pub(super) struct SidebarWorkspaceRow {
    workspace: forktty_core::Workspace,
    pub(super) meta: String,
    summary: String,
    status: Option<WorkspaceStatusBadge>,
    pub(super) pane_count: usize,
}

#[derive(Clone)]
pub(super) struct WorkspaceStatusBadge {
    pub(super) label: &'static str,
    pub(super) tooltip: &'static str,
    pub(super) class_name: &'static str,
}

pub(super) struct SidebarSnapshot {
    pub(super) rows: Vec<SidebarWorkspaceRow>,
    active_workspace_name: Option<String>,
    active_status_label: Option<String>,
    pub(super) active_full_path: Option<String>,
    pub(super) active_pane_label: Option<String>,
    pub(super) signature: String,
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
    controller: Rc<RefCell<TerminalController>>,
) {
    glib::idle_add_local_once(move || {
        refresh_sidebar(&ui, &state, &controller, true);
    });
}

fn install_workspace_reorder_dnd<W>(
    handle: &W,
    workspace_id: &str,
    state: &SocketAppState,
    controller: &Rc<RefCell<TerminalController>>,
    ui: &SidebarUi,
) where
    W: IsA<gtk::Widget>,
{
    install_workspace_drag_source(handle, workspace_id);
    let target_workspace_id = workspace_id.to_string();
    let state_for_drop = state.clone();
    let controller_for_drop = controller.clone();
    let ui_for_drop = ui.clone();
    let handle_for_drop = handle.as_ref().clone();
    let target = workspace_drop_target(move |source_workspace_id, _x, y| {
        clear_workspace_drop_indicator(&handle_for_drop);
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
    });
    target.set_preload(true);
    let target_workspace_id_for_motion = workspace_id.to_string();
    let state_for_motion = state.clone();
    let handle_for_motion = handle.as_ref().clone();
    target.connect_motion(move |target, _x, y| {
        let Some(source_workspace_id) = target
            .value()
            .and_then(|value| workspace_dnd_id_from_value(&value))
        else {
            clear_workspace_drop_indicator(&handle_for_motion);
            return gdk::DragAction::MOVE;
        };
        if source_workspace_id == target_workspace_id_for_motion {
            clear_workspace_drop_indicator(&handle_for_motion);
            return gdk::DragAction::empty();
        }
        let position = drop_position(y, handle_for_motion.allocated_height());
        let order = state_for_motion
            .model
            .lock()
            .ok()
            .map(|model| {
                model
                    .list_workspaces()
                    .into_iter()
                    .map(|workspace| workspace.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if workspace_move_would_keep_order(
            &order,
            &source_workspace_id,
            &target_workspace_id_for_motion,
            position,
        ) {
            clear_workspace_drop_indicator(&handle_for_motion);
            return gdk::DragAction::empty();
        }
        set_workspace_drop_indicator(&handle_for_motion, position);
        gdk::DragAction::MOVE
    });
    let handle_for_leave = handle.as_ref().clone();
    target.connect_leave(move |_| {
        clear_workspace_drop_indicator(&handle_for_leave);
    });
    handle.add_controller(target);
}

fn clear_workspace_drop_indicator(handle: &gtk::Widget) {
    handle.remove_css_class("drop-before");
    handle.remove_css_class("drop-after");
}

fn set_workspace_drop_indicator(handle: &gtk::Widget, position: forktty_core::MovePosition) {
    clear_workspace_drop_indicator(handle);
    match position {
        forktty_core::MovePosition::Before => handle.add_css_class("drop-before"),
        forktty_core::MovePosition::After => handle.add_css_class("drop-after"),
    }
}

pub(super) fn workspace_move_would_keep_order(
    workspace_order: &[String],
    source_id: &str,
    target_id: &str,
    position: forktty_core::MovePosition,
) -> bool {
    if source_id == target_id {
        return true;
    }
    let source_index = workspace_order
        .iter()
        .position(|workspace_id| workspace_id == source_id);
    let target_index = workspace_order
        .iter()
        .position(|workspace_id| workspace_id == target_id);
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

pub(super) fn relative_pane_target(
    panes: &[String],
    focused_surface_id: &str,
    delta: isize,
) -> Option<String> {
    if panes.len() < 2 {
        return None;
    }
    let current = panes
        .iter()
        .position(|surface_id| surface_id == focused_surface_id)?;
    let len = panes.len() as isize;
    let target = (current as isize + delta).rem_euclid(len) as usize;
    panes.get(target).cloned()
}

pub(super) fn refresh_sidebar(
    ui: &SidebarUi,
    state: &SocketAppState,
    controller: &Rc<RefCell<TerminalController>>,
    force: bool,
) {
    let snapshot = sidebar_snapshot(state);
    if let Some(name) = snapshot.active_workspace_name.as_deref() {
        ui.workspace_title_label.set_label(name);
        ui.workspace_title
            .set_tooltip_text(Some(&format!("Active workspace: {name}")));
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
            pane_count,
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
        if pane_count > 1 {
            accessible_label.push_str(&format!(". {pane_count} panes"));
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

        if pane_count > 1 {
            let count_badge = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            count_badge.add_css_class("workspace-count-badge");
            count_badge.set_valign(gtk::Align::Center);
            let count_icon = gtk::Image::from_icon_name("forktty-grid-symbolic");
            count_icon.add_css_class("workspace-count-icon");
            count_icon.set_tooltip_text(Some(&format!("{pane_count} panes")));
            let count_label = gtk::Label::new(Some(&pane_count.to_string()));
            count_label.add_css_class("workspace-count");
            count_label.set_tooltip_text(Some(&format!("{pane_count} panes")));
            count_badge.append(&count_icon);
            count_badge.append(&count_label);
            card.append(&count_badge);
        }

        let mut tooltip = format!("{}\n{}", workspace.name, meta);
        if let Some(status) = status.as_ref() {
            tooltip.push('\n');
            tooltip.push_str(status.tooltip);
        }
        if pane_count > 1 {
            tooltip.push('\n');
            tooltip.push_str(&format!("{pane_count} panes"));
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
            let state = state_for_click.clone();
            let workspace_id = workspace_id_for_click.clone();
            let controller = controller_for_click.clone();
            let ui = ui_for_click.clone();
            glib::spawn_future_local(async move {
                select_sidebar_workspace(&state, &workspace_id, &controller).await;
                schedule_sidebar_refresh(ui, state, controller);
            });
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
            let state = state_for_key.clone();
            let workspace_id = workspace_id_for_key.clone();
            let controller = controller_for_key.clone();
            let ui = ui_for_key.clone();
            glib::spawn_future_local(async move {
                select_sidebar_workspace(&state, &workspace_id, &controller).await;
                schedule_sidebar_refresh(ui, state, controller);
            });
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
    let terminal_surfaces = state.terminal.surfaces().unwrap_or_default();
    let ready_surface_ids = ready_surface_ids(state, &terminal_surfaces).unwrap_or_default();
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
        let (statuses, progress) =
            sidebar_visible_metadata(&model, &workspace, &statuses, &progress);
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
        let pane_count = collect_panes(&workspace.pane_tree).len();
        let ssh_state = surfaces.iter().find_map(|surface| {
            if let forktty_core::SurfaceKind::Ssh { host } = &surface.kind {
                Some((
                    host.clone(),
                    ready_surface_ids.contains(surface.id.as_str()),
                ))
            } else {
                None
            }
        });
        let agent_project_cwd = model
            .surface(&workspace.focused_surface_id)
            .and_then(surface_agent_resume_cwd);
        let meta = workspace_meta_line(
            &workspace,
            ssh_state
                .as_ref()
                .map(|(host, connected)| (host.as_str(), *connected)),
            agent_project_cwd,
        );
        rows.push(SidebarWorkspaceRow {
            workspace,
            meta,
            summary,
            status,
            pane_count,
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
            .map(|surface| compact_path(surface_effective_project_cwd(surface)))
            .unwrap_or_else(|| compact_path(&workspace.working_dir));
        format!("{} · {}", workspace.name, cwd)
    });
    let active_full_path = active_workspace.map(|workspace| {
        model
            .surface(&workspace.focused_surface_id)
            .map(|surface| {
                surface_effective_project_cwd(surface)
                    .to_string_lossy()
                    .to_string()
            })
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
        let title_is_cwd_echo = surface
            .map(|surface| {
                title_matches_path_echo(&title, surface_effective_project_cwd(surface))
                    || title_matches_path_echo(&title, &surface.cwd)
            })
            .unwrap_or_else(|| title_matches_path_echo(&title, &workspace.working_dir));
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
            row.pane_count,
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
    ssh_state: Option<(&str, bool)>,
    effective_project_cwd: Option<&Path>,
) -> String {
    let mut parts = Vec::new();
    if let Some((host, connected)) = ssh_state {
        parts.push(format!("ssh:{host}"));
        parts.push(if connected {
            "connected".to_string()
        } else {
            "disconnected".to_string()
        });
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
    parts.push(compact_path(
        effective_project_cwd.unwrap_or(workspace.working_dir.as_path()),
    ));
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

pub(super) fn surface_effective_project_cwd(surface: &forktty_core::Surface) -> &Path {
    surface_agent_resume_cwd(surface).unwrap_or(surface.cwd.as_path())
}

pub(super) fn surface_agent_resume_cwd(surface: &forktty_core::Surface) -> Option<&Path> {
    surface.agent_session.as_ref()?.resume_cwd.as_deref()
}

fn title_matches_path_echo(title: &str, path: &Path) -> bool {
    let compact = compact_path(path);
    let full = path.to_string_lossy();
    title == compact.as_str() || title == full.as_ref() || full.ends_with(&format!("/{title}"))
}

pub(super) async fn select_sidebar_workspace(
    state: &SocketAppState,
    workspace_id: &str,
    controller: &Rc<RefCell<TerminalController>>,
) {
    if let Err(err) = select_workspace_with_terminal(state, workspace_id).await {
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
    let Some(target) = relative_pane_target(&panes, &workspace.focused_surface_id, delta) else {
        return false;
    };
    model.focus_surface(&target)
}

pub(super) fn swap_focused_pane_relative(state: &SocketAppState, delta: isize) -> bool {
    let swapped = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let Some(workspace) = model.active_workspace() else {
            return false;
        };
        let source = workspace.focused_surface_id.clone();
        let panes = collect_panes(&workspace.pane_tree);
        let Some(target) = relative_pane_target(&panes, &source, delta) else {
            return false;
        };
        model.swap_panes(&source, &target)
    };
    if swapped {
        save_session_from_state(state);
    }
    swapped
}

pub(super) fn move_active_workspace_relative(state: &SocketAppState, delta: isize) -> bool {
    let moved = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let Some((source_id, target_id, position)) =
            active_workspace_relative_target(&model, delta)
        else {
            return false;
        };
        model.move_workspace(&source_id, &target_id, position)
    };
    if moved {
        save_session_from_state(state);
    }
    moved
}

pub(super) fn active_workspace_can_move_relative(state: &SocketAppState, delta: isize) -> bool {
    let model = match state.model.lock() {
        Ok(model) => model,
        Err(_) => return false,
    };
    active_workspace_relative_target(&model, delta).is_some()
}

fn active_workspace_relative_target(
    model: &WorkspaceModel,
    delta: isize,
) -> Option<(String, String, forktty_core::MovePosition)> {
    let workspaces = model.list_workspaces();
    let current = workspaces.iter().position(|workspace| workspace.active)?;
    let target = current as isize + delta;
    if target < 0 || target >= workspaces.len() as isize {
        return None;
    }
    let position = if delta < 0 {
        forktty_core::MovePosition::Before
    } else {
        forktty_core::MovePosition::After
    };
    Some((
        workspaces[current].id.clone(),
        workspaces[target as usize].id.clone(),
        position,
    ))
}

pub(super) fn notification_targets_workspace(
    model: &WorkspaceModel,
    notification: &NotificationItem,
    workspace_id: &str,
) -> bool {
    if let Some(surface_id) = notification.surface_id.as_deref() {
        let Some(surface) = model.surface(surface_id) else {
            return false;
        };
        return surface.workspace_id == workspace_id
            && notification
                .workspace_id
                .as_deref()
                .is_none_or(|id| id == surface.workspace_id);
    }
    notification.workspace_id.as_deref() == Some(workspace_id)
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

    // Mode indicators (e.g. "Claude mode: bypassPermissions", deliberately
    // colored red so the risky mode is visible on its own pill) describe a
    // standing configuration, not an event: they must not keep the whole
    // workspace badged as Error/Exited for as long as the mode is active.
    let workspace_surface_ids = collect_leaves(&workspace.pane_tree);
    let event_statuses = || {
        statuses.iter().filter(|status| {
            !status_entry_is_mode_indicator(status)
                && status_entry_targets_workspace_surface(status, &workspace_surface_ids)
        })
    };

    if event_statuses().any(status_entry_suggests_needs_input) {
        return Some(WorkspaceStatusBadge {
            label: "Input",
            tooltip: "Needs input",
            class_name: "needs-input",
        });
    }

    if event_statuses().any(status_entry_suggests_error) {
        return Some(WorkspaceStatusBadge {
            label: "Error",
            tooltip: "Error reported in this workspace",
            class_name: "error",
        });
    }

    if event_statuses().any(status_entry_suggests_surface_missing) {
        return Some(WorkspaceStatusBadge {
            label: "Missing",
            tooltip: "Surface missing",
            class_name: "exited",
        });
    }

    if event_statuses().any(status_entry_suggests_exited) {
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

    if statuses.iter().any(status_entry_suggests_starting) {
        return Some(WorkspaceStatusBadge {
            label: "Starting",
            tooltip: "Process starting",
            class_name: "working",
        });
    }

    if statuses.iter().any(status_entry_suggests_running) {
        return Some(WorkspaceStatusBadge {
            label: "Working",
            tooltip: "Process working",
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

pub(super) fn sidebar_visible_metadata(
    model: &forktty_core::WorkspaceModel,
    workspace: &forktty_core::Workspace,
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
) -> (Vec<StatusEntry>, Vec<ProgressEntry>) {
    let workspace_surface_ids = collect_leaves(&workspace.pane_tree);
    let active_agent_aliases = active_agent_metadata_aliases(model, &workspace.id);
    let mut statuses = statuses
        .iter()
        .filter(|status| {
            sidebar_metadata_key_is_visible(
                &status.key,
                &workspace_surface_ids,
                &active_agent_aliases,
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    append_agent_lifecycle_statuses(model, &workspace.id, &mut statuses);
    let progress = progress
        .iter()
        .filter(|progress| {
            sidebar_metadata_key_is_visible(
                &progress.key,
                &workspace_surface_ids,
                &active_agent_aliases,
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    (statuses, progress)
}

fn append_agent_lifecycle_statuses(
    model: &forktty_core::WorkspaceModel,
    workspace_id: &str,
    statuses: &mut Vec<StatusEntry>,
) {
    let sessions = model
        .list_surfaces(Some(workspace_id))
        .into_iter()
        .filter_map(|surface| surface.agent_session)
        .collect::<Vec<_>>();

    for agent in [
        forktty_core::AgentKind::Codex,
        forktty_core::AgentKind::ClaudeCode,
        forktty_core::AgentKind::Pi,
        forktty_core::AgentKind::OpenCode,
        forktty_core::AgentKind::Antigravity,
        forktty_core::AgentKind::Grok,
        forktty_core::AgentKind::Custom,
    ] {
        let aliases = forktty_core::agents::agent_metadata_aliases(agent);
        let has_primary_status = statuses.iter().any(|status| {
            status
                .key
                .strip_prefix("agent:")
                .is_some_and(|key| aliases.contains(&key))
        });
        if has_primary_status {
            continue;
        }

        let lifecycle = sessions
            .iter()
            .filter(|session| session.agent == agent)
            .map(|session| session.lifecycle)
            .min_by_key(|lifecycle| match lifecycle {
                forktty_core::AgentSessionLifecycle::NeedsInput => 0,
                forktty_core::AgentSessionLifecycle::Running => 1,
                _ => 2,
            });
        let Some((value, color)) = lifecycle.and_then(agent_lifecycle_sidebar_status) else {
            continue;
        };
        let (key, label) = sidebar_agent_identity(agent);
        statuses.push(StatusEntry {
            key: format!("agent:{key}"),
            label: label.to_string(),
            value: value.to_string(),
            color: Some(color.to_string()),
        });
    }
}

fn agent_lifecycle_sidebar_status(
    lifecycle: forktty_core::AgentSessionLifecycle,
) -> Option<(&'static str, &'static str)> {
    match lifecycle {
        forktty_core::AgentSessionLifecycle::Running => Some(("Running", "blue")),
        forktty_core::AgentSessionLifecycle::NeedsInput => Some(("Needs input", "yellow")),
        forktty_core::AgentSessionLifecycle::Idle
        | forktty_core::AgentSessionLifecycle::Suspended
        | forktty_core::AgentSessionLifecycle::Ended
        | forktty_core::AgentSessionLifecycle::Unknown => None,
    }
}

fn sidebar_agent_identity(agent: forktty_core::AgentKind) -> (&'static str, &'static str) {
    match agent {
        forktty_core::AgentKind::ClaudeCode => ("claude", "Claude"),
        forktty_core::AgentKind::Codex => ("codex", "Codex"),
        forktty_core::AgentKind::Antigravity => ("antigravity", "Antigravity"),
        forktty_core::AgentKind::Grok => ("grok", "Grok"),
        forktty_core::AgentKind::Pi => ("pi", "Pi"),
        forktty_core::AgentKind::OpenCode => ("opencode", "OpenCode"),
        forktty_core::AgentKind::Custom => ("custom", "Agent"),
    }
}

fn active_agent_metadata_aliases(
    model: &forktty_core::WorkspaceModel,
    workspace_id: &str,
) -> BTreeSet<&'static str> {
    model
        .list_surfaces(Some(workspace_id))
        .into_iter()
        .filter_map(|surface| surface.agent_session)
        .filter(|session| agent_lifecycle_keeps_sidebar_metadata(session.lifecycle))
        .flat_map(|session| forktty_core::agents::agent_metadata_aliases(session.agent).iter())
        .copied()
        .collect()
}

fn agent_lifecycle_keeps_sidebar_metadata(lifecycle: forktty_core::AgentSessionLifecycle) -> bool {
    matches!(
        lifecycle,
        forktty_core::AgentSessionLifecycle::Running
            | forktty_core::AgentSessionLifecycle::Idle
            | forktty_core::AgentSessionLifecycle::NeedsInput
            | forktty_core::AgentSessionLifecycle::Unknown
    )
}

fn sidebar_metadata_key_is_visible(
    key: &str,
    workspace_surface_ids: &[String],
    active_agent_aliases: &BTreeSet<&'static str>,
) -> bool {
    if let Some(surface_id) = status_surface_id(key) {
        return workspace_surface_ids.iter().any(|id| id == surface_id);
    }
    if let Some(agent_alias) = managed_agent_alias_from_metadata_key(key) {
        return active_agent_aliases.contains(agent_alias);
    }
    true
}

fn managed_agent_alias_from_metadata_key(key: &str) -> Option<&'static str> {
    let rest = key.strip_prefix("agent:")?;
    [
        forktty_core::AgentKind::ClaudeCode,
        forktty_core::AgentKind::Codex,
        forktty_core::AgentKind::Antigravity,
        forktty_core::AgentKind::Grok,
        forktty_core::AgentKind::Pi,
        forktty_core::AgentKind::OpenCode,
        forktty_core::AgentKind::Custom,
    ]
    .into_iter()
    .flat_map(|agent| {
        forktty_core::agents::agent_metadata_aliases(agent)
            .iter()
            .copied()
    })
    .find(|alias| {
        rest == *alias
            || rest
                .strip_prefix(*alias)
                .is_some_and(|suffix| suffix.starts_with(':'))
    })
}

pub(super) fn surface_status_blocks_auto_spawn(statuses: &[StatusEntry], surface_id: &str) -> bool {
    let key = surface_status_key(surface_id);
    statuses.iter().any(|status| {
        status.key == key
            && (status_entry_suggests_error(status) || status_entry_suggests_exited(status))
    })
}

fn status_entry_targets_workspace_surface(
    status: &StatusEntry,
    workspace_surface_ids: &[String],
) -> bool {
    let Some(surface_id) = status_surface_id(&status.key) else {
        return true;
    };
    workspace_surface_ids.iter().any(|id| id == surface_id)
}

fn status_surface_id(key: &str) -> Option<&str> {
    key.strip_prefix("surface:")?
        .split_once(':')
        .map(|(id, _)| id)
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

pub(super) fn status_entry_suggests_starting(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    value == "starting" || value.contains("starting ")
}

pub(super) fn status_entry_suggests_needs_input(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    value.contains("needs_input") || value.contains("needs input")
}

pub(super) fn status_entry_suggests_surface_missing(status: &StatusEntry) -> bool {
    let value = status.value.to_ascii_lowercase();
    value == "surface_missing" || value.contains("surface missing")
}

/// Status entries that describe a standing mode rather than an event — today
/// the agent permission-mode pills (`agent:<key>:permission`), whose warning
/// colors must not be read as workspace failures.
pub(super) fn status_entry_is_mode_indicator(status: &StatusEntry) -> bool {
    status.key.starts_with("agent:") && status.key.ends_with(":permission")
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
    value == "closed"
        || value.contains("exited")
        || value.contains("stopped")
        || color == "yellow"
        || color == "warning"
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
        .filter(|status| !status_entry_is_mode_indicator(status))
        .map(format_status_summary_part)
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

fn format_status_summary_part(status: &StatusEntry) -> String {
    let value = if managed_agent_alias_from_metadata_key(&status.key).is_some() {
        if status_entry_suggests_needs_input(status) {
            "Needs input".to_string()
        } else if status_entry_suggests_starting(status) {
            "Starting".to_string()
        } else if status_entry_suggests_error(status) {
            "Problem".to_string()
        } else if status_entry_suggests_surface_missing(status) {
            "Missing".to_string()
        } else if status_entry_suggests_exited(status) {
            "Done".to_string()
        } else if status_entry_suggests_running(status) {
            "Working".to_string()
        } else {
            status.value.clone()
        }
    } else {
        status.value.clone()
    };
    format!("{}: {}", status.label, value)
}

/// Fixed sidebar sections below the workspace list: project shortcuts and the
/// Settings/About footer.
#[derive(Clone)]
pub(super) struct SidebarSectionsUi {
    pub(super) resources_shell: gtk::Box,
    pub(super) git_repos_row: gtk::Button,
    pub(super) footer_shell: gtk::Box,
    pub(super) about_row: gtk::Button,
}

pub(super) fn build_sidebar_sections() -> SidebarSectionsUi {
    let resources_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    resources_shell.add_css_class("sidebar-fixed-section");
    resources_shell.append(&sidebar_section_label("Resources"));
    let git_repos_row = sidebar_nav_row("forktty-merge-symbolic", "Worktrees");
    resources_shell.append(&git_repos_row);

    let footer_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    footer_shell.add_css_class("sidebar-footer");
    let settings_row = sidebar_nav_row("forktty-settings-symbolic", "Settings");
    settings_row.set_action_name(Some("app.settings"));
    footer_shell.append(&settings_row);
    let about_row = sidebar_nav_row("forktty-info-symbolic", "About ForkTTY");
    footer_shell.append(&about_row);

    SidebarSectionsUi {
        resources_shell,
        git_repos_row,
        footer_shell,
        about_row,
    }
}

fn sidebar_section_label(title: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    label.add_css_class("sidebar-section-label");
    label.add_css_class("sidebar-fixed-section-label");
    label
}

fn sidebar_nav_row(icon: &str, label: &str) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Image::from_icon_name(icon);
    icon.add_css_class("sidebar-nav-icon");
    content.append(&icon);
    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    text.add_css_class("sidebar-nav-label");
    content.append(&text);
    let button = gtk::Button::builder()
        .child(&content)
        .has_frame(false)
        .build();
    button.add_css_class("flat");
    button.add_css_class("sidebar-nav-row");
    set_accessible_button_text(&button, label, None);
    button
}
