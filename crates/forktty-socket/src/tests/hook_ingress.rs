//! Serialized hook ingress ordering, ownership, and atomicity regressions.

use super::*;
use crate::hook_session::{hook_target_gate_for_surface, run_serialized_hook_ingress};
use forktty_core::LogLevel;

mod hook_prompt_lifecycle;

fn target(state: &SocketAppState) -> (String, String) {
    let model = state.model.lock().unwrap();
    let workspace = model.list_workspaces().into_iter().next().unwrap();
    (workspace.id, workspace.focused_surface_id)
}

fn hook_target(
    workspace_id: &str,
    surface_id: &str,
    session_id: &str,
    event: &str,
    clock: Option<&str>,
    order: Option<u64>,
) -> Value {
    let mut params = json!({
        "workspace_id": workspace_id,
        "surface_id": surface_id,
        "hook_session_id": session_id,
        "hook_event_name": event,
    });
    if let Some(clock) = clock {
        params["hook_event_clock"] = json!(clock);
    }
    if let Some(order) = order {
        params["hook_event_order"] = json!(order);
    }
    params
}

#[allow(clippy::too_many_arguments)]
async fn set_agent_status(
    state: &SocketAppState,
    workspace_id: &str,
    surface_id: &str,
    session_id: &str,
    event: &str,
    clock: Option<&str>,
    order: Option<u64>,
    value: &str,
    turn_id: Option<&str>,
) -> Value {
    let mut params = hook_target(workspace_id, surface_id, session_id, event, clock, order);
    params["key"] = json!("agent:codex");
    params["label"] = json!("Codex");
    params["value"] = json!(value);
    if let Some(turn_id) = turn_id {
        params["hook_turn_id"] = json!(turn_id);
    }
    dispatch(state, "metadata.set_status", params)
        .await
        .unwrap()
}

#[tokio::test]
async fn serialized_hook_ingress_ignores_lower_order_without_any_state_change() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "ordered-session",
        "stop",
        Some("boottime-ns"),
        Some(200),
        "Ready",
        None,
    )
    .await;

    let mut progress = hook_target(
        &workspace_id,
        &surface_id,
        "ordered-session",
        "post-tool",
        Some("boottime-ns"),
        Some(100),
    );
    progress["key"] = json!("task");
    progress["label"] = json!("Task");
    progress["value"] = json!(50.0);
    assert_eq!(
        dispatch(&state, "metadata.set_progress", progress)
            .await
            .unwrap(),
        json!({"ignored": true})
    );

    let mut log = hook_target(
        &workspace_id,
        &surface_id,
        "ordered-session",
        "post-tool",
        Some("boottime-ns"),
        Some(100),
    );
    log["level"] = json!("info");
    log["message"] = json!("stale");
    assert_eq!(
        dispatch(&state, "metadata.log", log).await.unwrap(),
        json!({"ignored": true})
    );

    let mut notification = hook_target(
        &workspace_id,
        &surface_id,
        "ordered-session",
        "notification",
        Some("boottime-ns"),
        Some(100),
    );
    notification["title"] = json!("Stale");
    notification["body"] = json!("must not appear");
    notification["kind"] = json!("info");
    assert_eq!(
        dispatch(&state, "notification.create", notification)
            .await
            .unwrap(),
        json!({"ignored": true})
    );

    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "ordered-session",
        "prompt-submit",
        Some("boottime-ns"),
        Some(100),
        "Running",
        None,
    )
    .await;
    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses[0]["value"], "Ready");
    assert!(dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
    assert!(dispatch(
        &state,
        "metadata.list_logs",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
    assert!(dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn serialized_hook_ingress_applies_equal_actions_and_advances_on_greater_order() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "multi-action-session",
        "prompt-submit",
        Some("boottime-ns"),
        Some(100),
        "Running",
        None,
    )
    .await;

    let mut log = hook_target(
        &workspace_id,
        &surface_id,
        "multi-action-session",
        "prompt-submit",
        Some("boottime-ns"),
        Some(100),
    );
    log["level"] = json!("info");
    log["message"] = json!("equal-order action");
    dispatch(&state, "metadata.log", log).await.unwrap();

    let mut progress = hook_target(
        &workspace_id,
        &surface_id,
        "multi-action-session",
        "prompt-submit",
        Some("boottime-ns"),
        Some(100),
    );
    progress["key"] = json!("task");
    progress["label"] = json!("Task");
    progress["value"] = json!(25.0);
    dispatch(&state, "metadata.set_progress", progress)
        .await
        .unwrap();

    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "multi-action-session",
        "stop",
        Some("boottime-ns"),
        Some(200),
        "Ready",
        None,
    )
    .await;
    let logs = dispatch(
        &state,
        "metadata.list_logs",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let progress = dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(logs[0]["message"], "equal-order action");
    assert_eq!(progress[0]["value"], 25.0);
    assert_eq!(statuses[0]["value"], "Ready");
}

#[tokio::test]
async fn serialized_hook_ingress_replaces_clock_and_missing_order_preserves_watermark() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    for (clock, order, value) in [
        (Some("clock-a"), Some(100), 10.0),
        (Some("clock-b"), Some(5), 20.0),
        (Some("clock-b"), None, 30.0),
        (Some("clock-b"), Some(4), 40.0),
    ] {
        let mut params = hook_target(
            &workspace_id,
            &surface_id,
            "clock-session",
            "post-tool",
            clock,
            order,
        );
        params["key"] = json!("task");
        params["label"] = json!("Task");
        params["value"] = json!(value);
        dispatch(&state, "metadata.set_progress", params)
            .await
            .unwrap();
    }
    let progress = dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(progress[0]["value"], 30.0);
}

#[tokio::test]
async fn serialized_hook_ingress_watermark_is_scoped_to_provider_session() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-a",
        "stop",
        Some("boottime-ns"),
        Some(500),
        "Ready",
        None,
    )
    .await;
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-b",
        "session-start",
        Some("boottime-ns"),
        Some(10),
        "Running",
        None,
    )
    .await;
    let model = state.model.lock().unwrap();
    let session = model
        .surface(&surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(session.session_id, "session-b");
    assert_eq!(session.lifecycle, AgentSessionLifecycle::Running);
}

#[test]
fn serialized_hook_ingress_keeps_decision_mutation_and_watermark_atomic() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id, other_workspace_id, other_surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.list_workspaces().into_iter().next().unwrap();
        let other = model.create_workspace("other", "/tmp/other");
        (
            workspace.id,
            workspace.focused_surface_id,
            other.id,
            other.focused_surface_id,
        )
    };
    let mut initial = hook_target(
        &workspace_id,
        &surface_id,
        "atomic-session",
        "post-tool",
        Some("boottime-ns"),
        Some(100),
    );
    hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut initial, |_| {
        Ok(json!({"applied": true}))
    })
    .unwrap();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let first_state = state.clone();
    let mut first_params = hook_target(
        &workspace_id,
        &surface_id,
        "atomic-session",
        "post-tool",
        Some("boottime-ns"),
        Some(300),
    );
    let first = std::thread::spawn(move || {
        hook_session::run_serialized_hook_ingress(
            &first_state,
            "metadata.log",
            &mut first_params,
            |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(json!({"applied": "first"}))
            },
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let (stale_done_tx, stale_done_rx) = std::sync::mpsc::channel();
    let stale_state = state.clone();
    let mut stale_params = hook_target(
        &workspace_id,
        &surface_id,
        "atomic-session",
        "post-tool",
        Some("boottime-ns"),
        Some(200),
    );
    let stale = std::thread::spawn(move || {
        stale_done_tx
            .send(hook_session::run_serialized_hook_ingress(
                &stale_state,
                "metadata.log",
                &mut stale_params,
                |_| Ok(json!({"applied": "stale"})),
            ))
            .unwrap();
    });

    let (other_done_tx, other_done_rx) = std::sync::mpsc::channel();
    let other_state = state.clone();
    let mut other_params = hook_target(
        &other_workspace_id,
        &other_surface_id,
        "independent-session",
        "post-tool",
        Some("boottime-ns"),
        Some(50),
    );
    let other = std::thread::spawn(move || {
        other_done_tx
            .send(hook_session::run_serialized_hook_ingress(
                &other_state,
                "metadata.log",
                &mut other_params,
                |_| Ok(json!({"applied": "other"})),
            ))
            .unwrap();
    });

    assert_eq!(
        other_done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        Some(json!({"applied": "other"}))
    );
    assert!(stale_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        first.join().unwrap().unwrap(),
        Some(json!({"applied": "first"}))
    );
    assert_eq!(
        stale_done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        Some(json!({"ignored": true}))
    );
    stale.join().unwrap();
    other.join().unwrap();
}

#[test]
fn serialized_hook_ingress_does_not_advance_watermark_when_mutation_fails() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    let mut initial = hook_target(
        &workspace_id,
        &surface_id,
        "failure-session",
        "post-tool",
        Some("boottime-ns"),
        Some(100),
    );
    hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut initial, |_| {
        Ok(json!({"applied": true}))
    })
    .unwrap();
    let mut failed = hook_target(
        &workspace_id,
        &surface_id,
        "failure-session",
        "post-tool",
        Some("boottime-ns"),
        Some(300),
    );
    assert!(
        hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut failed, |_| Err(
            DispatchError::InvalidParam("forced".to_string())
        ),)
        .is_err()
    );
    let mut after = hook_target(
        &workspace_id,
        &surface_id,
        "failure-session",
        "post-tool",
        Some("boottime-ns"),
        Some(200),
    );
    assert_eq!(
        hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut after, |_| Ok(
            json!({"applied": true})
        ),)
        .unwrap(),
        Some(json!({"applied": true}))
    );
}

#[tokio::test]
async fn serialized_hook_ingress_rejects_late_events_from_a_superseded_session() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-a",
        "session-start",
        Some("boottime-ns"),
        Some(500),
        "Running",
        None,
    )
    .await;

    let mut replacement_log = hook_target(
        &workspace_id,
        &surface_id,
        "session-b",
        "session-start",
        Some("boottime-ns"),
        Some(10),
    );
    replacement_log["level"] = json!("info");
    replacement_log["message"] = json!("replacement started");
    assert_ne!(
        dispatch(&state, "metadata.log", replacement_log)
            .await
            .unwrap(),
        json!({"ignored": true})
    );
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-b",
        "session-start",
        Some("boottime-ns"),
        Some(10),
        "Running",
        None,
    )
    .await;
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-b",
        "stop",
        Some("boottime-ns"),
        Some(20),
        "Ready",
        None,
    )
    .await;

    assert_eq!(
        set_agent_status(
            &state,
            &workspace_id,
            &surface_id,
            "session-a",
            "session-end",
            Some("boottime-ns"),
            Some(600),
            "Ended by stale A",
            None,
        )
        .await,
        json!({"ignored": true})
    );
    let mut clear = hook_target(
        &workspace_id,
        &surface_id,
        "session-a",
        "session-end",
        Some("boottime-ns"),
        Some(700),
    );
    clear["key"] = json!("agent:codex");
    assert_eq!(
        dispatch(&state, "metadata.clear_status", clear)
            .await
            .unwrap(),
        json!({"ignored": true})
    );
    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses[0]["value"], "Ready");
    let model = state.model.lock().unwrap();
    let session = model
        .surface(&surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(session.session_id, "session-b");
    assert_eq!(session.lifecycle, AgentSessionLifecycle::Idle);
}

#[tokio::test]
async fn serialized_hook_ingress_keeps_same_turn_terminal_prompt_guard() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    for (event, value, order, turn_id) in [
        ("prompt-submit", "Running", 100, Some("prompt:one")),
        ("stop", "Ready", 200, None),
        ("prompt-submit", "Running", 300, Some("prompt:one")),
    ] {
        set_agent_status(
            &state,
            &workspace_id,
            &surface_id,
            "same-turn-session",
            event,
            Some("boottime-ns"),
            Some(order),
            value,
            turn_id,
        )
        .await;
    }
    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses[0]["value"], "Ready");
}

#[test]
fn serialized_hook_ingress_serializes_ownership_check_and_mutation_per_target() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.list_workspaces().into_iter().next().unwrap();
        assert!(model.set_surface_agent_session(
            &workspace.focused_surface_id,
            AgentKind::Codex,
            "session-a",
        ));
        (workspace.id, workspace.focused_surface_id)
    };
    let mut initial = hook_target(
        &workspace_id,
        &surface_id,
        "session-a",
        "prompt-submit",
        Some("boottime-ns"),
        Some(100),
    );
    hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut initial, |_| {
        Ok(json!({"applied": true}))
    })
    .unwrap();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let replacement_state = state.clone();
    let replacement_surface = surface_id.clone();
    let mut replacement_params = hook_target(
        &workspace_id,
        &surface_id,
        "session-b",
        "session-start",
        Some("boottime-ns"),
        Some(10),
    );
    let replacement = std::thread::spawn(move || {
        hook_session::run_serialized_hook_ingress(
            &replacement_state,
            "metadata.log",
            &mut replacement_params,
            |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let mut model = replacement_state.model.lock().unwrap();
                assert!(model.set_surface_agent_session(
                    &replacement_surface,
                    AgentKind::Codex,
                    "session-b",
                ));
                Ok(json!({"applied": "replacement"}))
            },
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let (late_mutated_tx, late_mutated_rx) = std::sync::mpsc::channel();
    let (late_done_tx, late_done_rx) = std::sync::mpsc::channel();
    let late_state = state.clone();
    let mut late_params = hook_target(
        &workspace_id,
        &surface_id,
        "session-a",
        "stop",
        Some("boottime-ns"),
        Some(200),
    );
    let late = std::thread::spawn(move || {
        let result = hook_session::run_serialized_hook_ingress(
            &late_state,
            "metadata.clear_status",
            &mut late_params,
            |_| {
                late_mutated_tx.send(()).unwrap();
                Ok(json!({"applied": "late"}))
            },
        );
        late_done_tx.send(result).unwrap();
    });

    let early_late_result = late_done_rx.recv_timeout(Duration::from_millis(100));
    let late_was_blocked = matches!(
        early_late_result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
    release_tx.send(()).unwrap();
    assert_eq!(
        replacement.join().unwrap().unwrap(),
        Some(json!({"applied": "replacement"}))
    );
    let late_result = match early_late_result {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            late_done_rx.recv_timeout(Duration::from_secs(1)).unwrap()
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("late mutation thread disconnected")
        }
    }
    .unwrap();
    late.join().unwrap();
    assert!(
        late_was_blocked,
        "late A must wait for B's ownership mutation"
    );
    assert_eq!(late_result, Some(json!({"ignored": true})));
    assert!(late_mutated_rx.try_recv().is_err());
}

#[test]
fn serialized_hook_ingress_missing_order_limits_takeover_to_the_same_event() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.list_workspaces().into_iter().next().unwrap();
        assert!(model.set_surface_agent_session(
            &workspace.focused_surface_id,
            AgentKind::Codex,
            "session-a",
        ));
        (workspace.id, workspace.focused_surface_id)
    };
    for event in ["prompt-submit", "prompt-submit"] {
        let mut action = hook_target(
            &workspace_id,
            &surface_id,
            "session-b",
            event,
            Some("boottime-ns"),
            None,
        );
        assert_eq!(
            hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut action, |_| Ok(
                json!({"applied": true})
            ),)
            .unwrap(),
            Some(json!({"applied": true}))
        );
    }
    let (mutated_tx, mutated_rx) = std::sync::mpsc::channel();
    let mut later = hook_target(
        &workspace_id,
        &surface_id,
        "session-b",
        "post-tool",
        Some("boottime-ns"),
        None,
    );
    let result =
        hook_session::run_serialized_hook_ingress(&state, "metadata.log", &mut later, |_| {
            mutated_tx.send(()).unwrap();
            Ok(json!({"applied": true}))
        })
        .unwrap();
    assert_eq!(result, Some(json!({"ignored": true})));
    assert!(mutated_rx.try_recv().is_err());
}

#[tokio::test]
async fn serialized_hook_ingress_scopes_same_turn_guard_to_provider_session() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-a",
        "prompt-submit",
        Some("boottime-ns"),
        Some(100),
        "Running",
        Some("shared-turn"),
    )
    .await;
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-a",
        "stop",
        Some("boottime-ns"),
        Some(200),
        "Ready",
        Some("shared-turn"),
    )
    .await;
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "session-b",
        "prompt-submit",
        Some("boottime-ns"),
        Some(10),
        "Running",
        Some("shared-turn"),
    )
    .await;

    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses[0]["value"], "Running");
    let model = state.model.lock().unwrap();
    assert_eq!(
        model
            .surface(&surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap()
            .session_id,
        "session-b"
    );
}

#[tokio::test]
async fn serialized_hook_ingress_canonicalizes_every_surface_workspace_selector() {
    for selector_key in [
        Some("workspace_name"),
        Some("id"),
        Some("worktree_name"),
        None,
    ] {
        let (state, _backend) = test_state();
        let (workspace_id, workspace_name, worktree_name, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = if selector_key == Some("worktree_name") {
                model.create_worktree_workspace(
                    "hook-worktree",
                    "/tmp/hook-worktree",
                    "hook-branch",
                    "hook-worktree",
                )
            } else {
                model.list_workspaces().into_iter().next().unwrap()
            };
            (
                workspace.id,
                workspace.name,
                workspace.worktree_name,
                workspace.focused_surface_id,
            )
        };
        let selector_value = match selector_key {
            Some("workspace_name") => Some(workspace_name.as_str()),
            Some("id") => Some(workspace_id.as_str()),
            Some("worktree_name") => worktree_name.as_deref(),
            None => None,
            Some(unexpected) => panic!("unexpected selector {unexpected}"),
        };
        let status_params = |session_id: &str, event: &str, order: u64, value: &str| {
            let mut params = json!({
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": value,
                "hook_session_id": session_id,
                "hook_event_name": event,
                "hook_event_clock": "boottime-ns",
                "hook_event_order": order,
            });
            if let (Some(key), Some(value)) = (selector_key, selector_value) {
                params[key] = json!(value);
            }
            params
        };
        dispatch(
            &state,
            "metadata.set_status",
            status_params("session-a", "session-start", 100, "Running A"),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            status_params("session-b", "session-start", 10, "Running B"),
        )
        .await
        .unwrap();
        assert_eq!(
            dispatch(
                &state,
                "metadata.set_status",
                status_params("session-a", "post-tool", 200, "Late A"),
            )
            .await
            .unwrap(),
            json!({"ignored": true}),
            "selector {selector_key:?} bypassed hook target ownership",
        );

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(statuses[0]["value"], "Running B");
        let model = state.model.lock().unwrap();
        assert_eq!(
            model
                .surface(&surface_id)
                .unwrap()
                .agent_session
                .as_ref()
                .unwrap()
                .session_id,
            "session-b"
        );
    }
}

#[test]
#[serial_test::serial]
fn agent_hibernate_waits_for_inflight_hook_mutation_on_the_same_target() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &surface_id,
            AgentKind::Codex,
            "hibernate-race-session",
        ));
        assert!(
            model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,)
        );
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
        (workspace.id, surface_id)
    };
    let target_gate = hook_target_gate_for_surface(&state, &surface_id).unwrap();
    let params = json!({
        "workspace_id": workspace_id,
        "surface_id": surface_id,
        "level": "info",
        "message": "linearized before hibernate",
        "hook_session_id": "hibernate-race-session",
        "hook_event_name": "post-tool",
        "hook_event_clock": "boottime-ns",
        "hook_event_order": 100,
    });
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (hook_done_tx, hook_done_rx) = std::sync::mpsc::channel();
    let hook_state = state.clone();
    let hook_workspace_id = workspace_id.clone();
    let hook = std::thread::spawn(move || {
        let mut params = params;
        let result = run_serialized_hook_ingress(&hook_state, "metadata.log", &mut params, |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let entry = hook_state
                .model
                .lock()
                .unwrap()
                .append_log(
                    &hook_workspace_id,
                    LogLevel::Info,
                    "linearized before hibernate",
                )
                .unwrap();
            Ok(json!({"id": entry.id}))
        })
        .unwrap();
        hook_done_tx.send(()).unwrap();
        result
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();

    let (hibernate_done_tx, hibernate_done_rx) = std::sync::mpsc::channel();
    let hibernate_state = state.clone();
    let hibernate_surface_id = surface_id.clone();
    let hibernate = std::thread::spawn(move || {
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(dispatch(
            &hibernate_state,
            "agent.hibernate",
            json!({"surface_id": hibernate_surface_id, "min_idle_ms": 0}),
        ));
        hibernate_done_tx.send(result).unwrap();
    });
    let contended = target_gate.wait_until_contended(std::time::Duration::from_secs(2));
    release_tx.send(()).unwrap();
    hook_done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let hibernated = hibernate_done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(hibernated["lifecycle"], "suspended");
    assert!(hook.join().unwrap().is_some());
    hibernate.join().unwrap();
    assert!(
        contended,
        "hibernate never contended on the in-flight hook mutation's target gate",
    );
    let model = state.model.lock().unwrap();
    assert_eq!(
        model
            .surface(&surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap()
            .lifecycle,
        AgentSessionLifecycle::Suspended,
    );
}

#[test]
fn surface_close_waits_for_inflight_hook_mutation_before_removing_metadata() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    let target_gate = hook_target_gate_for_surface(&state, &surface_id).unwrap();
    let status_key = format!("surface:{surface_id}:hook-race");
    let params = json!({
        "workspace_id": workspace_id,
        "surface_id": surface_id,
        "key": status_key,
        "label": "Agent",
        "value": "Running",
        "hook_session_id": "surface-close-race-session",
        "hook_event_name": "prompt-submit",
        "hook_event_clock": "boottime-ns",
        "hook_event_order": 100,
    });
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (hook_done_tx, hook_done_rx) = std::sync::mpsc::channel();
    let hook_state = state.clone();
    let hook_workspace_id = workspace_id.clone();
    let hook_status_key = status_key.clone();
    let hook = std::thread::spawn(move || {
        let mut params = params;
        let result =
            run_serialized_hook_ingress(&hook_state, "metadata.set_status", &mut params, |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let status = hook_state
                    .model
                    .lock()
                    .unwrap()
                    .set_status(
                        &hook_workspace_id,
                        hook_status_key,
                        "Agent",
                        "Running",
                        Some("blue".to_string()),
                    )
                    .unwrap();
                Ok(json!(status))
            })
            .unwrap();
        hook_done_tx.send(()).unwrap();
        result
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();

    let (close_done_tx, close_done_rx) = std::sync::mpsc::channel();
    let close_state = state.clone();
    let close_surface_id = surface_id.clone();
    let close = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(dispatch(
            &close_state,
            "surface.close",
            json!({"surface_id": close_surface_id}),
        ));
        close_done_tx.send(result).unwrap();
    });
    let contended = target_gate.wait_until_contended(std::time::Duration::from_secs(2));
    release_tx.send(()).unwrap();
    hook_done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    close_done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(hook.join().unwrap().is_some());
    close.join().unwrap();
    assert!(
        contended,
        "surface.close never contended on the in-flight hook mutation's target gate",
    );
    let model = state.model.lock().unwrap();
    assert!(model.surface(&surface_id).is_none());
    assert!(
        model
            .list_status(&workspace_id)
            .iter()
            .all(|status| status.key != status_key),
        "surface.close left hook metadata behind",
    );
}
