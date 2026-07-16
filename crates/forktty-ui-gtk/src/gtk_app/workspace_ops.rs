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

pub(super) fn spawn_surface_gtk(
    state: &SocketAppState,
    surface: &Surface,
) -> Result<(), TerminalError> {
    let base = SpawnRequest::for_surface(surface, state.shell.clone(), state.socket_path.clone());
    let Some(request) = forktty_socket::spawn_request_for_surface(base, surface) else {
        return Ok(());
    };
    state.terminal.spawn(request)
}

pub(super) fn add_new_tab_surface(state: &SocketAppState, near_surface_id: &str) {
    let _ = forktty_socket::sync_live_surface_cwds(state);
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
    if let Err(err) = spawn_surface_gtk(state, &surface) {
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
    let _ = forktty_socket::sync_live_surface_cwds(state);
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
    if let Err(err) = spawn_surface_gtk(state, &surface) {
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
    let selected = {
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
    };
    if selected {
        save_session_from_state(state);
    }
    selected
}

pub(super) fn select_tab_surface(state: &SocketAppState, surface_id: &str) -> bool {
    let selected = match state.model.lock() {
        Ok(mut model) => model.select_tab(surface_id),
        Err(_) => false,
    };
    if selected {
        save_session_from_state(state);
    }
    selected
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
    let _ = forktty_socket::sync_live_surface_cwds(state);
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

    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            eprintln!("Failed to restart terminal surface {surface_id}: {err}");
            if let Ok(mut model) = state.model.lock() {
                let _ = model.set_status(
                    &surface.workspace_id,
                    surface_status_key(surface_id),
                    "Terminal",
                    format!(
                        "Restart failed: {}",
                        truncate_single_line(&err.to_string(), 140)
                    ),
                    Some("red".to_string()),
                );
                let _ = model.append_log(
                    &surface.workspace_id,
                    LogLevel::Error,
                    format!("Terminal {surface_id} restart failed: {err}"),
                );
            }
            create_global_notification(
                state,
                "Restart Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return false;
        }
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

    // Route through spawn_request_for_surface so an agent terminal (a Terminal
    // surface carrying an agent_session) is rewritten to the provider resume
    // argv + resume cwd instead of relaunching a plain shell, matching every
    // other (re)spawn path (controller.rs, worktree_dialog.rs).
    let base = SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
    let Some(request) = forktty_socket::spawn_request_for_surface(base, &surface) else {
        return false;
    };
    if let Err(err) = state.terminal.spawn(request) {
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
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let Some(workspace_id) = model
            .surface(surface_id)
            .map(|surface| surface.workspace_id.clone())
        else {
            return false;
        };
        let Some(workspace) = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        if !surface_is_in_multi_tab_leaf(&workspace.pane_tree, surface_id) {
            return false;
        }

        match state.terminal.close(surface_id) {
            Ok(()) | Err(TerminalError::NotFound(_)) => {}
            Err(err) => {
                let message = err.to_string();
                drop(model);
                eprintln!("Failed to close tab terminal: {message}");
                create_global_notification(
                    state,
                    "Close Tab Failed",
                    &message,
                    NotificationKind::Error,
                );
                return false;
            }
        }
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

// Production close paths resolve a concrete surface id before closing (the
// active workspace can change while a confirmation dialog is open); this
// focused-surface wrapper remains for tests pinning its semantics.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn close_active_surface(state: &SocketAppState) {
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
    close_surface_by_id(state, &focused);
}

pub(super) fn close_surface_by_id(state: &SocketAppState, surface_id: &str) {
    let (focused, root_replacement) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        if model.surface(surface_id).is_none() {
            return;
        }
        let root_replacement = model.prepare_root_surface_replacement(surface_id);
        (surface_id.to_string(), root_replacement)
    };

    if let Some(replacement) = root_replacement {
        if let Err(err) = spawn_surface_gtk(state, &replacement) {
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
    // to launch ssh <host>, and agent terminals resume with their provider argv.
    let base = SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
    let Some(request) = forktty_socket::spawn_request_for_surface(base, &surface) else {
        return Ok(());
    };
    state.terminal.spawn(request)
}
