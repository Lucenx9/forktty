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
                    if let Err(err) = open_workspace_from_path(&state, path) {
                        eprintln!("Failed to open workspace: {err}");
                        create_global_notification(
                            &state,
                            "Open Workspace Failed",
                            &err,
                            NotificationKind::Error,
                        );
                    }
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

pub(super) fn open_workspace_from_path(
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
    let cwd = default_startup_workspace_dir();
    let (workspace, previous_active_id) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let count = model.list_workspaces().len() + 1;
        let previous_active_id = model.active_workspace_id();
        (
            model.create_workspace(format!("workspace-{count}"), cwd),
            previous_active_id,
        )
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
    } else {
        save_session_from_state(state);
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

pub(super) fn close_active_workspace(state: &SocketAppState) {
    let (workspace, surface_ids, is_last_workspace) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        let surface_ids = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        let is_last_workspace = model.list_workspaces().len() == 1;
        (workspace, surface_ids, is_last_workspace)
    };

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
        if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
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
        save_session_from_state(state);
        return;
    }

    if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
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
    surface_ids: &[String],
) -> Result<(), TerminalError> {
    for surface_id in surface_ids {
        match state.terminal.close(surface_id) {
            Ok(()) | Err(TerminalError::NotFound(_)) => {}
            Err(err) => return Err(err),
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
    if let Ok(mut model) = state.model.lock() {
        let notification = model.create_notification(title, body, kind, None, None);
        if state.notification_dispatch {
            dispatch_notification_with_loaded_config(&notification);
        }
    }
}

pub(super) fn create_local_notification(state: &SocketAppState, title: &str, body: &str) {
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
    let notification = model.create_notification(
        title,
        body,
        NotificationKind::Info,
        Some(workspace_id),
        Some(surface_id),
    );
    if state.notification_dispatch {
        dispatch_notification_with_loaded_config(&notification);
    }
}
