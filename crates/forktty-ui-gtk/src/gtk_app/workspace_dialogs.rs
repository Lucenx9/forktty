use super::*;

pub(super) fn open_workspace_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::FileChooserNative::new(
        Some("Open Workspace"),
        Some(parent),
        gtk::FileChooserAction::SelectFolder,
        Some("Open"),
        Some("Cancel"),
    );
    let state = state.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            match dialog.file().and_then(|file| file.path()) {
                Some(path) => {
                    let state = state.clone();
                    glib::MainContext::default().spawn_local(async move {
                        if let Err(err) = open_workspace_from_path(&state, path).await {
                            eprintln!("Failed to open workspace: {err}");
                            create_global_notification(
                                &state,
                                "Open Workspace Failed",
                                &err,
                                NotificationKind::Error,
                            );
                        }
                    });
                }
                None => create_global_notification(
                    &state,
                    "Open Workspace Failed",
                    "The selected folder does not map to a local path.",
                    NotificationKind::Error,
                ),
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

pub(super) async fn open_workspace_from_path(
    state: &SocketAppState,
    path: PathBuf,
) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("Not a directory: {}", path.display()));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("workspace")
        .to_string();
    let _surface_set_guard = state.surface_set_guard().await;
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (model.create_workspace(name, path), previous_active_id)
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_workspace(
        &workspace,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id)?;
        return Err(err.to_string());
    }
    save_session_from_state(state);
    Ok(())
}

pub(super) fn create_plain_workspace(state: &SocketAppState) {
    let state = state.clone();
    glib::MainContext::default().spawn_local(async move {
        let _ = create_plain_workspace_transaction(&state).await;
    });
}

pub(super) async fn create_plain_workspace_transaction(state: &SocketAppState) -> bool {
    let _surface_set_guard = state.surface_set_guard().await;
    let cwd = default_startup_workspace_dir();
    let (workspace, previous_active_id) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let previous_active_id = model.active_workspace_id();
        (model.create_auto_named_workspace(cwd), previous_active_id)
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_workspace(
        &workspace,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        let _ = rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id);
        eprintln!("Failed to create workspace terminal: {err}");
        create_global_notification(
            state,
            "Workspace Create Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
        false
    } else {
        save_session_from_state(state);
        true
    }
}

pub(super) fn rename_workspace_gtk(
    state: &SocketAppState,
    workspace_id: &str,
    new_name: &str,
) -> Result<(), String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err("Workspace name cannot be empty.".to_string());
    }
    if trimmed.chars().count() > 80 {
        return Err("Workspace name must be 80 characters or fewer.".to_string());
    }
    {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if model
            .list_workspaces()
            .into_iter()
            .any(|workspace| workspace.id != workspace_id && workspace.name == trimmed)
        {
            return Err(format!("A workspace named '{trimmed}' already exists."));
        }
        model
            .rename_workspace(WorkspaceSelector::Id(workspace_id), trimmed)
            .ok_or_else(|| "Workspace no longer exists.".to_string())?;
    }
    save_session_from_state(state);
    Ok(())
}

pub(super) fn close_workspace_by_id(state: &SocketAppState, workspace_id: &str) {
    let state = state.clone();
    let workspace_id = workspace_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        close_workspace_by_id_transaction(&state, &workspace_id).await;
    });
}

pub(super) async fn close_workspace_by_id_transaction(state: &SocketAppState, workspace_id: &str) {
    let _surface_set_guard = state.surface_set_guard().await;
    let (workspace, surfaces, is_last_workspace) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let Some(workspace) = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        let surfaces = model.list_surfaces(Some(&workspace.id));
        let is_last_workspace = model.list_workspaces().len() == 1;
        (workspace, surfaces, is_last_workspace)
    };
    let surface_ids = surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();

    if is_last_workspace {
        let (replacement, previous_active_id) = {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return,
            };
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", workspace.working_dir.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal_gtk(state, &replacement) {
            let mut message = err.to_string();
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                message = format!("{message}; workspace rollback failed: {rollback_err}");
            }
            notify_close_workspace_failed(state, &message);
            return;
        }
        if let Err(err) = close_terminal_surfaces(state, &surfaces) {
            let mut message = err.to_string();
            if let Err(cleanup_err) =
                forget_terminal_surface_gtk(state, &replacement.focused_surface_id)
            {
                message = format!("{message}; replacement cleanup failed: {cleanup_err}");
            }
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                message = format!("{message}; workspace rollback failed: {rollback_err}");
            }
            notify_close_workspace_failed(state, &message);
            return;
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return,
            };
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        if let Err(err) =
            forktty_socket::evict_hook_session_targets_for_surfaces(state, &surface_ids)
        {
            eprintln!("Failed to clean up closed workspace hook state: {err}");
        }
        save_session_from_state(state);
        return;
    }

    if let Err(err) = close_terminal_surfaces(state, &surfaces) {
        notify_close_workspace_failed(state, &err.to_string());
        return;
    }

    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
    }
    if let Err(err) = forktty_socket::evict_hook_session_targets_for_surfaces(state, &surface_ids) {
        eprintln!("Failed to clean up closed workspace hook state: {err}");
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep a workspace terminal alive: {err}");
    }
    save_session_from_state(state);
}

pub(super) fn spawn_workspace_terminal_gtk(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<(), TerminalError> {
    state.terminal.spawn(SpawnRequest::for_workspace(
        workspace,
        state.shell.clone(),
        state.socket_path.clone(),
    ))
}

pub(super) fn close_terminal_surfaces(
    state: &SocketAppState,
    surfaces: &[Surface],
) -> Result<(), TerminalError> {
    let mut closed = Vec::new();
    for surface in surfaces {
        match state.terminal.close(&surface.id) {
            Ok(()) => closed.push(surface.clone()),
            Err(TerminalError::NotFound(_)) => {}
            Err(err) => {
                let restore_failures = closed
                    .iter()
                    .filter_map(|closed_surface| {
                        spawn_surface_gtk(state, closed_surface)
                            .err()
                            .map(|restore_err| format!("{}: {restore_err}", closed_surface.id))
                    })
                    .collect::<Vec<_>>();
                if restore_failures.is_empty() {
                    return Err(err);
                }
                return Err(TerminalError::Backend(format!(
                    "{err}; terminal restore failed: {}",
                    restore_failures.join("; ")
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn forget_terminal_surface_gtk(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), TerminalError> {
    match state.terminal.forget_surface(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err),
    }
}

pub(super) fn notify_close_workspace_failed(state: &SocketAppState, message: &str) {
    eprintln!("Failed to close workspace: {message}");
    create_global_notification(
        state,
        "Close Workspace Failed",
        message,
        NotificationKind::Error,
    );
}

pub(super) fn create_global_notification(
    state: &SocketAppState,
    title: &str,
    body: &str,
    kind: NotificationKind,
) {
    let creation = {
        let Ok(mut model) = state.model.lock() else {
            return;
        };
        model.create_notification_with_evictions(title, body, kind, None, None)
    };
    for notification_id in creation.evicted_desktop_notification_ids {
        forktty_core::close_desktop_notification(&notification_id);
    }
    if state.notification_dispatch {
        dispatch_notification_with_loaded_config(&creation.notification);
    }
}

pub(super) fn create_local_notification(state: &SocketAppState, title: &str, body: &str) {
    create_local_notification_with_kind(state, title, body, NotificationKind::Info);
}

pub(super) fn create_local_notification_with_kind(
    state: &SocketAppState,
    title: &str,
    body: &str,
    kind: NotificationKind,
) {
    // Resolve the target and create the notification under one lock so the
    // focused surface cannot be closed in between, leaving a notification
    // that points at a surface the model no longer knows.
    let Ok(mut model) = state.model.lock() else {
        return;
    };
    let Some((workspace_id, surface_id)) = model
        .active_workspace()
        .map(|workspace| (workspace.id, workspace.focused_surface_id))
    else {
        return;
    };
    let creation = model.create_notification_with_evictions(
        title,
        body,
        kind,
        Some(workspace_id),
        Some(surface_id),
    );
    drop(model);
    for notification_id in creation.evicted_desktop_notification_ids {
        forktty_core::close_desktop_notification(&notification_id);
    }
    if state.notification_dispatch {
        dispatch_notification_with_loaded_config(&creation.notification);
    }
}
