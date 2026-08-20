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
    spawn_surface_gtk_with_failure_handler(state, surface, None)
}

pub(super) fn spawn_surface_gtk_with_failure_handler(
    state: &SocketAppState,
    surface: &Surface,
    failure_handler: Option<DeferredSpawnFailureHandler>,
) -> Result<(), TerminalError> {
    let failure_lease = failure_handler.clone();
    let base = SpawnRequest::for_surface(surface, state.shell.clone(), state.socket_path.clone());
    let request = match forktty_socket::spawn_request_for_surface(base, surface) {
        Ok(Some(request)) => request,
        Ok(None) => {
            if let Some(failure_handler) = failure_handler {
                failure_handler.disarm();
            }
            return Ok(());
        }
        Err(error) => {
            record_terminal_spawn_failure(
                &state.model,
                &surface.workspace_id,
                &surface.id,
                &error.to_string(),
                state.notification_dispatch,
            );
            return Err(TerminalError::Backend(error.to_string()));
        }
    };
    let result = match failure_handler {
        Some(failure_handler) => state
            .terminal
            .spawn_with_failure_handler(request, failure_handler),
        None => state.terminal.spawn(request),
    };
    drop(failure_lease);
    result
}

pub(super) fn add_new_tab_surface(state: &SocketAppState, near_surface_id: &str) {
    let state = state.clone();
    let near_surface_id = near_surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let _ = add_new_tab_surface_transaction(&state, &near_surface_id).await;
    });
}

pub(super) async fn add_new_tab_surface_transaction(
    state: &SocketAppState,
    near_surface_id: &str,
) -> bool {
    let surface_set_guard = state.surface_set_guard().await;
    let _ = forktty_socket::sync_live_surface_cwds(state);
    let added = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to add tab: workspace model lock poisoned");
                return false;
            }
        };
        let Some(snapshot) = SurfaceCreationLayoutSnapshot::capture(&model, near_surface_id) else {
            return false;
        };
        model
            .add_tab(near_surface_id)
            .map(|surface| (surface, snapshot))
    };

    let Some((surface, snapshot)) = added else {
        return false;
    };
    let failure_handler = deferred_surface_creation_failure_handler(
        state,
        &surface.id,
        snapshot,
        surface_set_guard,
        save_session_from_state,
    );
    if let Err(err) = spawn_surface_gtk_with_failure_handler(state, &surface, Some(failure_handler))
    {
        eprintln!("Failed to spawn new tab terminal: {err}");
        create_global_notification(
            state,
            "New Tab Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
        false
    } else {
        // Synchronous backends disarm before returning, so persist their commit
        // now. GTK still owns the surface guard here; this snapshot is skipped
        // and controller materialization performs the authoritative save.
        save_session_from_state(state);
        true
    }
}

pub(super) fn split_active_surface(state: &SocketAppState, axis: SplitAxis) {
    let source_id = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to split pane: workspace model lock poisoned");
                return;
            }
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        workspace.focused_surface_id
    };

    request_split_surface_by_id(state, &source_id, axis);
}

pub(super) fn request_split_surface_by_id(
    state: &SocketAppState,
    surface_id: &str,
    axis: SplitAxis,
) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let _ = split_surface_by_id(&state, &surface_id, axis).await;
    });
}

pub(super) async fn split_surface_by_id(
    state: &SocketAppState,
    surface_id: &str,
    axis: SplitAxis,
) -> bool {
    let surface_set_guard = state.surface_set_guard().await;
    let target_exists = state
        .model
        .lock()
        .ok()
        .is_some_and(|model| model.surface(surface_id).is_some());
    if !target_exists {
        return false;
    }
    let _ = forktty_socket::sync_live_surface_cwds(state);
    let split = match state.model.lock() {
        Ok(mut model) => {
            let Some(snapshot) = SurfaceCreationLayoutSnapshot::capture(&model, surface_id) else {
                return false;
            };
            model
                .split_surface(surface_id, axis)
                .map(|surface| (surface, snapshot))
        }
        Err(_) => {
            eprintln!("Failed to split pane: workspace model lock poisoned");
            return false;
        }
    };

    let Some((surface, snapshot)) = split else {
        return false;
    };
    let failure_handler = deferred_surface_creation_failure_handler(
        state,
        &surface.id,
        snapshot,
        surface_set_guard,
        save_session_from_state,
    );
    if let Err(err) = spawn_surface_gtk_with_failure_handler(state, &surface, Some(failure_handler))
    {
        eprintln!("Failed to spawn split terminal: {err}");
        create_global_notification(
            state,
            "Split Failed",
            &err.to_string(),
            NotificationKind::Error,
        );
        false
    } else {
        // See add_new_tab_surface_transaction: GTK defers this save until
        // materialization, while synchronous backends can persist immediately.
        save_session_from_state(state);
        true
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
pub(super) fn open_browser_by_surface_id(
    state: &SocketAppState,
    surface_id: &str,
    axis: SplitAxis,
) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let _ = open_browser_by_surface_id_transaction(&state, &surface_id, axis).await;
    });
}

#[cfg(feature = "browser")]
pub(super) async fn open_browser_by_surface_id_transaction(
    state: &SocketAppState,
    surface_id: &str,
    axis: SplitAxis,
) -> bool {
    let surface_set_guard = state.surface_set_guard().await;
    let opened = match state.model.lock() {
        Ok(mut model) => model.open_browser_near(
            surface_id,
            "about:blank",
            forktty_core::ProfileId::default(),
            axis,
        ),
        Err(_) => {
            eprintln!("Failed to open browser pane: workspace model lock poisoned");
            return false;
        }
    };
    if opened.is_some() {
        drop(surface_set_guard);
        save_session_from_state(state);
        true
    } else {
        false
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

pub(super) fn restart_surface(state: &SocketAppState, surface_id: &str) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let _ = restart_surface_transaction(&state, &surface_id).await;
    });
}

pub(super) async fn restart_surface_transaction(state: &SocketAppState, surface_id: &str) -> bool {
    let _surface_set_guard = state.surface_set_guard().await;
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
    if !matches!(
        surface.kind,
        forktty_core::SurfaceKind::Terminal | forktty_core::SurfaceKind::Ssh { .. }
    ) {
        return false;
    }

    // Preflight the complete replacement request before destroying the current
    // runtime generation or publishing a transient Restarting status. Invalid
    // persisted SSH or agent-resume metadata must leave the live pane intact.
    let base = SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
    let request = match forktty_socket::spawn_request_for_surface(base, &surface) {
        Ok(Some(request)) => request,
        Ok(None) => return false,
        Err(error) => {
            record_terminal_spawn_failure(
                &state.model,
                &surface.workspace_id,
                &surface.id,
                &error.to_string(),
                state.notification_dispatch,
            );
            return false;
        }
    };

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

pub(super) fn close_tab_surface(state: &SocketAppState, surface_id: &str) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let _ = close_tab_surface_transaction(&state, &surface_id).await;
    });
}

pub(super) async fn close_tab_surface_transaction(
    state: &SocketAppState,
    surface_id: &str,
) -> bool {
    let surface_set_guard = state.surface_set_guard().await;
    let workspace = {
        let model = match state.model.lock() {
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
        workspace
    };

    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => {}
        Err(err) => {
            let message = err.to_string();
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
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        if !surface_is_in_multi_tab_leaf(&workspace.pane_tree, surface_id) {
            return false;
        }
        if model.close_surface(surface_id).is_none() {
            return false;
        }
    }
    if let Err(err) =
        forktty_socket::evict_hook_session_targets_for_surfaces(state, &[surface_id.to_string()])
    {
        eprintln!("Failed to clean up closed tab surface hook state: {err}");
    }
    drop(surface_set_guard);
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

pub(super) fn close_surface_by_id(state: &SocketAppState, surface_id: &str) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let _ = close_surface_by_id_transaction(&state, &surface_id).await;
    });
}

pub(super) async fn close_surface_by_id_transaction(
    state: &SocketAppState,
    surface_id: &str,
) -> bool {
    let surface_set_guard = state.surface_set_guard().await;
    let closed = close_surface_by_id_with(state, surface_id, &|surface_id| match state
        .terminal
        .close(surface_id)
    {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err),
    });
    drop(surface_set_guard);
    if closed {
        save_session_from_state(state);
    }
    closed
}

pub(super) fn close_surface_by_id_for_generation(
    state: &SocketAppState,
    backend: &Arc<GtkTerminalBackend>,
    surface_id: &str,
    generation: u64,
    on_complete: impl FnOnce(GenerationCloseOutcome) + 'static,
) {
    let state = state.clone();
    let backend = backend.clone();
    let surface_id = surface_id.to_string();
    glib::MainContext::default().spawn_local(async move {
        let outcome = close_surface_by_id_for_generation_transaction(
            &state,
            &backend,
            &surface_id,
            generation,
        )
        .await;
        on_complete(outcome);
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GenerationCloseOutcome {
    Closed,
    Stale,
    Failed,
}

type GenerationClosePreparation = Option<(Surface, Option<Surface>)>;
type GenerationClosePreparationResult = Result<GenerationClosePreparation, String>;

/// Close one embedded terminal incarnation as a single coordinated topology
/// transaction. The process-local surface guard excludes socket/worktree
/// topology changes from validation through runtime/model commit or rollback.
pub(super) async fn close_surface_by_id_for_generation_transaction(
    state: &SocketAppState,
    backend: &GtkTerminalBackend,
    surface_id: &str,
    generation: u64,
) -> GenerationCloseOutcome {
    let surface_guard = state.surface_set_guard().await;
    let prepared: Result<GenerationClosePreparationResult, TerminalError> = backend
        .with_surface_generation(surface_id, generation, || {
            let (surface, root_replacement) = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "workspace model lock poisoned".to_string())?;
                let Some(surface) = model.surface(surface_id).cloned() else {
                    return Ok(None);
                };
                let _ = model.focus_surface(surface_id);
                let root_replacement = model.prepare_root_surface_replacement(surface_id);
                (surface, root_replacement)
            };
            if let Some(replacement) = root_replacement.as_ref() {
                spawn_surface_gtk(state, replacement).map_err(|err| err.to_string())?;
            }
            Ok(Some((surface, root_replacement)))
        });
    let (surface, root_replacement) = match prepared {
        Err(TerminalError::NotFound(_) | TerminalError::NotReady(_)) | Ok(Ok(None)) => {
            return GenerationCloseOutcome::Stale;
        }
        Err(err) => {
            report_generation_close_failure(state, &err.to_string());
            return GenerationCloseOutcome::Failed;
        }
        Ok(Err(err)) => {
            report_generation_close_failure(state, &err);
            return GenerationCloseOutcome::Failed;
        }
        Ok(Ok(Some(prepared))) => prepared,
    };

    match backend.close_for_generation(surface_id, generation) {
        Ok(()) => {}
        Err(TerminalError::NotFound(_) | TerminalError::NotReady(_)) => {
            if let Some(replacement) = root_replacement.as_ref() {
                let _ = forget_terminal_surface_gtk(state, &replacement.id);
            }
            return GenerationCloseOutcome::Stale;
        }
        Err(err) => {
            let mut message = err.to_string();
            if let Some(replacement) = root_replacement.as_ref() {
                if let Err(cleanup_err) = forget_terminal_surface_gtk(state, &replacement.id) {
                    message = format!("{message}; replacement cleanup failed: {cleanup_err}");
                }
            }
            report_generation_close_failure(state, &message);
            return GenerationCloseOutcome::Failed;
        }
    }

    let committed = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                rollback_generation_close_runtime(state, &surface, root_replacement.as_ref());
                report_generation_close_failure(state, "workspace model lock poisoned");
                return GenerationCloseOutcome::Failed;
            }
        };
        match root_replacement.clone() {
            Some(replacement) => {
                model
                    .close_surface_with_replacement(surface_id, Some(replacement.clone()))
                    .is_some()
                    && model.surface(&replacement.id).is_some()
            }
            None => model.close_surface(surface_id).is_some(),
        }
    };
    if !committed {
        rollback_generation_close_runtime(state, &surface, root_replacement.as_ref());
        report_generation_close_failure(state, "workspace model rejected terminal close commit");
        return GenerationCloseOutcome::Failed;
    }

    if let Err(err) = forktty_socket::evict_hook_session_targets_for_surfaces(
        state,
        std::slice::from_ref(&surface.id),
    ) {
        eprintln!("Failed to clean up closed surface hook state: {err}");
    }

    if root_replacement.is_none() {
        if let Err(err) = spawn_focused_surface_if_needed(state) {
            eprintln!("Failed to keep focused terminal alive: {err}");
        }
    }
    drop(surface_guard);
    save_session_from_state(state);
    GenerationCloseOutcome::Closed
}

fn rollback_generation_close_runtime(
    state: &SocketAppState,
    surface: &Surface,
    root_replacement: Option<&Surface>,
) {
    if let Some(replacement) = root_replacement {
        if let Err(err) = forget_terminal_surface_gtk(state, &replacement.id) {
            eprintln!("Failed to clean up replacement terminal after close rollback: {err}");
        }
    }
    if let Err(err) = spawn_surface_gtk(state, surface) {
        eprintln!("Failed to restore terminal after close rollback: {err}");
    }
}

fn report_generation_close_failure(state: &SocketAppState, message: &str) {
    eprintln!("Failed to close terminal surface: {message}");
    create_global_notification(state, "Close Pane Failed", message, NotificationKind::Error);
}

fn close_surface_by_id_with(
    state: &SocketAppState,
    surface_id: &str,
    close_terminal: &impl Fn(&str) -> Result<(), TerminalError>,
) -> bool {
    let (focused, root_replacement) = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        if model.surface(surface_id).is_none() {
            return false;
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
            return false;
        }
        match close_terminal(&focused) {
            Ok(()) => {}
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
                return false;
            }
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return false,
            };
            let _ = model.close_surface_with_replacement(&focused, Some(replacement));
        }
        if let Err(err) = forktty_socket::evict_hook_session_targets_for_surfaces(
            state,
            std::slice::from_ref(&focused),
        ) {
            eprintln!("Failed to clean up closed surface hook state: {err}");
        }
        return true;
    }

    match close_terminal(&focused) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("Failed to close terminal surface: {err}");
            create_global_notification(
                state,
                "Close Pane Failed",
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
        let _ = model.close_surface(&focused);
    }
    if let Err(err) = forktty_socket::evict_hook_session_targets_for_surfaces(
        state,
        std::slice::from_ref(&focused),
    ) {
        eprintln!("Failed to clean up closed surface hook state: {err}");
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep focused terminal alive: {err}");
    }
    true
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
    let request = match forktty_socket::spawn_request_for_surface(base, &surface) {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(error) => {
            record_terminal_spawn_failure(
                &state.model,
                &surface.workspace_id,
                &surface.id,
                &error.to_string(),
                state.notification_dispatch,
            );
            return Err(TerminalError::Backend(error.to_string()));
        }
    };
    state.terminal.spawn(request)
}
