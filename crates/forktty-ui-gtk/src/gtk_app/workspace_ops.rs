use super::*;

pub(super) fn add_new_tab_surface(state: &SocketAppState, near_surface_id: &str) {
    let surface = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to add tab: workspace model lock poisoned");
                return;
            }
        };
        model.add_tab(near_surface_id)
    };

    let Some(surface) = surface else {
        return;
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
        &surface,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        if let Ok(mut model) = state.model.lock() {
            let _ = model.close_surface(&surface.id);
        }
        eprintln!("Failed to spawn new tab terminal: {err}");
        create_global_notification(
            state,
            "New Tab Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
    } else {
        save_session_from_state(state);
    }
}

pub(super) fn split_active_surface(state: &SocketAppState, axis: SplitAxis) {
    let surface = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to split pane: workspace model lock poisoned");
                return;
            }
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        model.split_surface(&workspace.focused_surface_id, axis)
    };

    let Some(surface) = surface else {
        return;
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
        &surface,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        if let Ok(mut model) = state.model.lock() {
            let _ = model.close_surface(&surface.id);
        }
        eprintln!("Failed to spawn split terminal: {err}");
        create_global_notification(
            state,
            "Split Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
    } else {
        save_session_from_state(state);
    }
}

#[cfg(feature = "browser")]
pub(super) fn open_browser_active(state: &SocketAppState, axis: SplitAxis) {
    let opened = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to open browser pane: workspace model lock poisoned");
                return;
            }
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        let workspace_id = workspace.id.clone();
        model.open_browser(
            &workspace_id,
            "about:blank",
            forktty_core::ProfileId::default(),
            axis,
        )
    };
    if opened.is_some() {
        save_session_from_state(state);
    }
}

pub(super) fn restart_active_surface(state: &SocketAppState) {
    let focused = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        model
            .active_workspace()
            .map(|workspace| workspace.focused_surface_id)
    };
    let Some(focused) = focused else {
        return;
    };
    restart_surface(state, &focused);
}

pub(super) fn restart_surface(state: &SocketAppState, surface_id: &str) -> bool {
    let surface = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        model.surface(surface_id).cloned()
    };
    let Some(surface) = surface else {
        return false;
    };
    if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
        return false;
    }

    if let Ok(mut model) = state.model.lock() {
        let _ = model.set_status(
            &surface.workspace_id,
            surface_status_key(surface_id),
            "Terminal",
            "Restarting",
            Some("blue".to_string()),
        );
        let _ = model.focus_surface(surface_id);
        let _ = model.mark_surface_unread(surface_id, false);
    }

    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            eprintln!("Failed to restart terminal surface {surface_id}: {err}");
            create_global_notification(
                state,
                "Restart Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return false;
        }
    }

    if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
        &surface,
        state.shell.clone(),
        state.socket_path.clone(),
    )) {
        record_terminal_spawn_failure(
            &state.model,
            &surface.workspace_id,
            &surface.id,
            &err.to_string(),
            state.notification_dispatch,
        );
        return false;
    }
    true
}

pub(super) fn close_active_surface(state: &SocketAppState) {
    let (focused, root_replacement) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let focused = model
            .active_workspace()
            .map(|workspace| workspace.focused_surface_id);
        let Some(focused) = focused else {
            return;
        };
        if model.surface(&focused).is_none() {
            return;
        }
        let root_replacement = model.prepare_root_surface_replacement(&focused);
        (focused, root_replacement)
    };

    if let Some(replacement) = root_replacement {
        if let Err(err) = state.terminal.spawn(SpawnRequest::for_surface(
            &replacement,
            state.shell.clone(),
            state.socket_path.clone(),
        )) {
            eprintln!("Failed to spawn replacement terminal surface: {err}");
            create_global_notification(
                state,
                "Close Pane Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return;
        }
        match state.terminal.close(&focused) {
            Ok(()) | Err(TerminalError::NotFound(_)) => {}
            Err(err) => {
                let mut message = err.to_string();
                if let Err(cleanup_err) = forget_terminal_surface_gtk(state, &replacement.id) {
                    message = format!("{message}; replacement cleanup failed: {cleanup_err}");
                }
                eprintln!("Failed to close terminal surface: {message}");
                create_global_notification(
                    state,
                    "Close Pane Failed",
                    &message,
                    NotificationKind::Error,
                );
                return;
            }
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return,
            };
            let _ = model.close_surface_with_replacement(&focused, Some(replacement));
        }
        save_session_from_state(state);
        return;
    }

    match state.terminal.close(&focused) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            eprintln!("Failed to close terminal surface: {err}");
            create_global_notification(
                state,
                "Close Pane Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return;
        }
    }
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.close_surface(&focused);
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep focused terminal alive: {err}");
    }
    save_session_from_state(state);
}

pub(super) fn spawn_focused_surface_if_needed(state: &SocketAppState) -> Result<(), TerminalError> {
    let surface = {
        let model = state
            .model
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let Some(workspace) = model.active_workspace() else {
            return Ok(());
        };
        model.surface(&workspace.focused_surface_id).cloned()
    };
    let Some(surface) = surface else {
        return Ok(());
    };
    if state
        .terminal
        .surfaces()?
        .iter()
        .any(|terminal_surface| terminal_surface.surface_id == surface.id)
    {
        return Ok(());
    }
    // Browser surfaces return None (no PTY backend); Ssh surfaces are rewritten
    // to launch ssh <host> so reselecting a restored remote workspace respawns.
    let base = SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
    let Some(request) = forktty_socket::spawn_request_for_surface_kind(base, &surface.kind) else {
        return Ok(());
    };
    state.terminal.spawn(request)
}
