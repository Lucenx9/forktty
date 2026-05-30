use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabNavigation {
    Previous,
    Next,
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabMoveDirection {
    Left,
    Right,
}

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

pub(super) fn select_tab_in_focused_pane(
    state: &SocketAppState,
    navigation: TabNavigation,
) -> bool {
    let mut model = match state.model.lock() {
        Ok(model) => model,
        Err(_) => return false,
    };
    let Some(workspace) = model.active_workspace() else {
        return false;
    };
    let Some(target) = tab_navigation_target(
        &workspace.pane_tree,
        &workspace.focused_surface_id,
        navigation,
    ) else {
        return false;
    };
    model.select_tab(&target)
}

pub(super) fn move_focused_tab(state: &SocketAppState, direction: TabMoveDirection) -> bool {
    let moved = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let Some(workspace) = model.active_workspace() else {
            return false;
        };
        let source = workspace.focused_surface_id.clone();
        let Some((target, position)) = tab_move_target(&workspace.pane_tree, &source, direction)
        else {
            return false;
        };
        model.move_tab(&source, &target, position)
    };
    if moved {
        save_session_from_state(state);
    }
    moved
}

pub(super) fn tab_navigation_target(
    node: &PaneNode,
    focused_surface_id: &str,
    navigation: TabNavigation,
) -> Option<String> {
    match node {
        PaneNode::Leaf { tabs, .. } if tabs.iter().any(|id| id == focused_surface_id) => {
            if tabs.len() < 2 {
                return None;
            }
            let current = tabs.iter().position(|id| id == focused_surface_id)?;
            let target = match navigation {
                TabNavigation::Previous => (current + tabs.len() - 1) % tabs.len(),
                TabNavigation::Next => (current + 1) % tabs.len(),
                TabNavigation::First => 0,
                TabNavigation::Last => tabs.len() - 1,
            };
            tabs.get(target).cloned()
        }
        PaneNode::Leaf { .. } => None,
        PaneNode::Split { children, .. } => children
            .iter()
            .find_map(|child| tab_navigation_target(child, focused_surface_id, navigation)),
    }
}

pub(super) fn tab_move_target(
    node: &PaneNode,
    focused_surface_id: &str,
    direction: TabMoveDirection,
) -> Option<(String, forktty_core::MovePosition)> {
    match node {
        PaneNode::Leaf { tabs, .. } if tabs.iter().any(|id| id == focused_surface_id) => {
            let current = tabs.iter().position(|id| id == focused_surface_id)?;
            match direction {
                TabMoveDirection::Left if current > 0 => Some((
                    tabs[current - 1].clone(),
                    forktty_core::MovePosition::Before,
                )),
                TabMoveDirection::Right if current + 1 < tabs.len() => {
                    Some((tabs[current + 1].clone(), forktty_core::MovePosition::After))
                }
                _ => None,
            }
        }
        PaneNode::Leaf { .. } => None,
        PaneNode::Split { children, .. } => children
            .iter()
            .find_map(|child| tab_move_target(child, focused_surface_id, direction)),
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

pub(super) fn close_tab_surface(state: &SocketAppState, surface_id: &str) -> bool {
    let is_multi_tab = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let Some(surface) = model.surface(surface_id) else {
            return false;
        };
        let Some(workspace) = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == surface.workspace_id)
        else {
            return false;
        };
        surface_is_in_multi_tab_leaf(&workspace.pane_tree, surface_id)
    };
    if !is_multi_tab {
        return false;
    }

    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            eprintln!("Failed to close tab terminal: {err}");
            create_global_notification(
                state,
                "Close Tab Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return false;
        }
    }
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        if model.close_surface(surface_id).is_none() {
            return false;
        }
    }
    save_session_from_state(state);
    true
}

pub(super) fn surface_is_in_multi_tab_leaf(node: &PaneNode, surface_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => tabs.len() > 1 && tabs.iter().any(|id| id == surface_id),
        PaneNode::Split { children, .. } => children
            .iter()
            .any(|child| surface_is_in_multi_tab_leaf(child, surface_id)),
    }
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
