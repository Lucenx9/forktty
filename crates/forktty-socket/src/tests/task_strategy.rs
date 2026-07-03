//! Task strategy socket method regression tests.

use super::*;
use forktty_core::{
    plan_task_strategy, HarnessHealth, HarnessRole, TaskStrategy, TaskStrategyInput,
};

#[tokio::test]
async fn task_strategy_plan_is_public_capability() {
    let capabilities = system_runtime::capabilities();
    let methods = capabilities["methods"].as_array().unwrap();
    assert!(methods.iter().any(|method| method == "task.strategy.plan"));
    assert!(methods.iter().any(|method| method == "task.strategy.apply"));
}

#[tokio::test]
async fn task_strategy_plan_returns_read_only_strategy() {
    let (state, _backend) = test_state();
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the failing workflow loop test and run verification",
            "repo_dirty": false,
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["strategy"], "solo_with_verify_loop");
    assert_eq!(result["layers"]["workflow"], true);
    assert_eq!(result["layers"]["loop_metadata"], true);
    assert_eq!(result["layers"]["team"], false);
    assert_eq!(
        result["candidate_scores"][0]["strategy"],
        result["strategy"]
    );
    assert!(
        result["candidate_scores"][0]["score"]
            .as_i64()
            .unwrap_or_default()
            > 0
    );
    assert!(result["candidate_scores"][0]["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| factor["name"] == "task_class_fit"));
    assert!(
        result["assignments"][0]["score"]
            .as_i64()
            .unwrap_or_default()
            > 0
    );
    assert!(result["assignments"][0]["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| factor["name"] == "harness_readiness" || factor["name"] == "self_fallback"));
    assert!(result["approvals"]
        .as_array()
        .unwrap()
        .contains(&json!("start_run")));
    assert!(result["safety_notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|note| { note.as_str().unwrap_or_default().contains("read-only") }));
}

#[tokio::test]
async fn task_strategy_plan_response_stays_compact() {
    let (state, _backend) = test_state();
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task strategy router bug, review the result, and run focused verification",
            "task_kind": "focused_bugfix",
            "router_profile": "review_heavy",
            "repo_dirty": true,
            "likely_user_visible_change": true,
            "last_known_good": {
                "strategy": "implementer_plus_reviewer",
                "harness_id": "codex",
                "reason": "recent successful router fix"
            },
            "harness_signals": {
                "codex": {
                    "cooldown": true,
                    "cooldown_reason": "recent quota warning"
                },
                "claude": {
                    "locked_out": true,
                    "lockout_reason": "provider unavailable for this run"
                }
            }
        }),
    )
    .await
    .unwrap();

    let bytes = serde_json::to_vec(&result).unwrap().len();
    assert!(
        bytes <= 16_000,
        "task.strategy.plan response is {bytes} bytes; keep router explanations compact"
    );
    assert!(result["candidate_scores"].as_array().unwrap().len() >= 4);
}

#[tokio::test]
async fn task_strategy_plan_response_stays_compact_with_max_reason_hints() {
    let (state, _backend) = test_state();
    let reason = "r".repeat(forktty_core::protocol_limits::SOCKET_TASK_STRATEGY_REASON_MAX_BYTES);
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task strategy router bug, review the result, and run focused verification",
            "task_kind": "focused_bugfix",
            "router_profile": "review_heavy",
            "repo_dirty": true,
            "likely_user_visible_change": true,
            "last_known_good": {
                "strategy": "implementer_plus_reviewer",
                "harness_id": "codex",
                "reason": reason
            },
            "harness_signals": {
                "codex": {
                    "cooldown": true,
                    "cooldown_reason": reason
                },
                "claude": {
                    "cooldown": true,
                    "cooldown_reason": reason
                },
                "grok": {
                    "cooldown": true,
                    "cooldown_reason": reason
                }
            }
        }),
    )
    .await
    .unwrap();

    let bytes = serde_json::to_vec(&result).unwrap().len();
    assert!(
        bytes <= 16_000,
        "task.strategy.plan response is {bytes} bytes with max-size routing reasons"
    );
}

#[tokio::test]
async fn task_strategy_plan_accepts_explicit_router_profile() {
    let (state, _backend) = test_state();
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Implement the task router",
            "router_profile": "review_heavy",
            "repo_dirty": false,
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["router_profile"], "review_heavy");
    assert!(matches!(
        result["strategy"].as_str(),
        Some("implementer_plus_reviewer" | "solo_with_verify_loop")
    ));
    assert!(result["candidate_scores"][0]["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| factor["name"] == "router_profile_fit"
            && factor["points"].as_i64().unwrap_or_default() > 0));
}

#[tokio::test]
async fn task_strategy_plan_accepts_explicit_task_kind_hint() {
    let (state, _backend) = test_state();
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Bitte diese Änderung umsetzen",
            "task_kind": "feature_implementation",
            "repo_dirty": false,
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["task_class"], "feature_implementation");
    assert_eq!(result["strategy"], "solo_with_verify_loop");
    assert!(result["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason
            .as_str()
            .unwrap_or_default()
            .contains("caller-provided task kind")));
}

#[tokio::test]
async fn task_strategy_plan_rejects_conflicting_task_kind_aliases() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Inspect repo",
            "task_kind": "repo_inspection",
            "task_class_hint": "feature_implementation"
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("task_kind and task_class_hint"));
}

#[tokio::test]
async fn task_strategy_plan_accepts_last_known_good_context() {
    let (state, _backend) = test_state();
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task router bug and run tests",
            "repo_dirty": false,
            "likely_user_visible_change": true,
            "last_known_good": {
                "strategy": "parallel_experiment",
                "reason": "last successful experiment for this repo"
            }
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["strategy"], "solo_with_verify_loop");
    let lkg_candidate = result["candidate_scores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["strategy"] == "parallel_experiment")
        .unwrap();
    assert!(lkg_candidate["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| {
            factor["name"] == "last_known_good_strategy"
                && factor["points"].as_i64().unwrap_or_default() > 0
                && factor["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("last successful experiment")
        }));
}

#[tokio::test]
async fn task_strategy_plan_infers_last_known_good_from_completed_workflow() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "router-success-1",
            "workspace_id": workspace_id,
            "mode": "task_strategy",
            "status": "done",
            "goal": "Implement experimental router",
            "memory": "Task strategy: ParallelExperiment. Run id: router-success-1."
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": "router-success-1",
            "steps": [
                {
                    "id": "implement",
                    "title": "Implementer: claude",
                    "status": "done",
                    "detail": "Goal: Implement experimental router\nRole: implementer\nHarness: claude\nReason: completed successfully"
                }
            ]
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "workspace_id": workspace_id,
            "goal": "Fix the task router bug and run tests",
            "repo_dirty": false,
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    let lkg_candidate = result["candidate_scores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["strategy"] == "parallel_experiment")
        .unwrap();
    assert!(lkg_candidate["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| {
            factor["name"] == "last_known_good_strategy"
                && factor["points"].as_i64().unwrap_or_default() > 0
                && factor["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("router-success-1")
        }));
    assert!(result["reasons"].as_array().unwrap().iter().any(|reason| {
        reason
            .as_str()
            .unwrap_or_default()
            .contains("inferred last-known-good")
    }));
}

#[tokio::test]
async fn task_strategy_plan_ignores_invalid_inferred_last_known_good_harness_id() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let huge_harness_id = format!("legacy-{}", "x".repeat(20_000));

    dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "router-success-oversized-harness",
            "workspace_id": workspace_id,
            "mode": "task_strategy",
            "status": "done",
            "goal": "Inspect router",
            "memory": "Task strategy: SoloTracked. Run id: router-success-oversized-harness."
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": "router-success-oversized-harness",
            "steps": [
                {
                    "id": "implement",
                    "title": "Implementer: legacy",
                    "status": "done",
                    "detail": format!("Goal: Inspect router\nRole: implementer\nHarness: {huge_harness_id}\nReason: completed")
                }
            ]
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "workspace_id": workspace_id,
            "goal": "Inspect router state",
            "repo_dirty": false
        }),
    )
    .await
    .unwrap();

    let bytes = serde_json::to_vec(&result).unwrap().len();
    assert!(
        bytes <= 16_000,
        "task.strategy.plan response is {bytes} bytes after invalid inferred harness id"
    );
    let text = serde_json::to_string(&result).unwrap();
    assert!(!text.contains(&huge_harness_id));
}

#[tokio::test]
async fn task_strategy_plan_prefers_explicit_last_known_good_over_workflow_history() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "router-success-1",
            "workspace_id": workspace_id,
            "mode": "task_strategy",
            "status": "done",
            "goal": "Implement experimental router",
            "memory": "Task strategy: ParallelExperiment. Run id: router-success-1."
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "workspace_id": workspace_id,
            "goal": "Fix the task router bug and run tests",
            "repo_dirty": false,
            "likely_user_visible_change": true,
            "last_known_good": {
                "strategy": "review_only",
                "reason": "manual override"
            }
        }),
    )
    .await
    .unwrap();

    let manual_candidate = result["candidate_scores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["strategy"] == "review_only")
        .unwrap();
    assert!(manual_candidate["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| {
            factor["name"] == "last_known_good_strategy"
                && factor["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("manual override")
        }));
    assert!(!result["reasons"].as_array().unwrap().iter().any(|reason| {
        reason
            .as_str()
            .unwrap_or_default()
            .contains("inferred last-known-good")
    }));
}

#[tokio::test]
async fn task_strategy_plan_rejects_malformed_last_known_good() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Inspect router state",
            "last_known_good": []
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("last_known_good must be an object"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_unknown_router_profile() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Inspect router state",
            "router_profile": "reckless"
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("unsupported router profile"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_oversized_enum_values_with_compact_errors() {
    let (state, _backend) = test_state();
    let oversized = "x".repeat(16_000);

    for (field, expected) in [
        ("strategy", "unsupported task strategy"),
        ("task_kind", "unsupported task kind"),
        ("router_profile", "unsupported router profile"),
    ] {
        let mut params = json!({"goal": "Inspect router state"});
        params[field] = json!(oversized.clone());
        let err = dispatch(&state, "task.strategy.plan", params)
            .await
            .unwrap_err();

        let message = err.to_string();
        assert_eq!(err.code(), "invalid_param");
        assert!(message.contains(expected));
        assert!(
            message.len() < 128,
            "{field} error should stay compact, got {} bytes",
            message.len()
        );
    }
}

#[tokio::test]
async fn task_strategy_plan_rejects_oversized_last_known_good_strategy_with_compact_error() {
    let (state, _backend) = test_state();
    let oversized = "x".repeat(16_000);

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task router bug",
            "last_known_good": {
                "strategy": oversized
            }
        }),
    )
    .await
    .unwrap_err();

    let message = err.to_string();
    assert_eq!(err.code(), "invalid_param");
    assert!(message.contains("unsupported task strategy"));
    assert!(message.len() < 128, "error should stay compact: {message}");
}

#[tokio::test]
async fn task_strategy_apply_rejects_invalid_plan_with_compact_error_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["strategy"] = json!("x".repeat(16_000));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    let message = err.to_string();
    assert_eq!(err.code(), "invalid_param");
    assert_eq!(message, "Invalid parameter plan");
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_oversized_plan_reason_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["assignments"][0]["reason"] =
        json!("r".repeat(forktty_core::protocol_limits::SOCKET_TASK_STRATEGY_REASON_MAX_BYTES + 1));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "payload_too_large");
    assert!(err.to_string().contains("plan.assignments.reason"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_too_many_plan_reasons_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["reasons"] = json!((0..33)
        .map(|index| format!("reason {index}"))
        .collect::<Vec<_>>());

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("plan.reasons has too many items"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_too_many_plan_score_factors_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["assignments"][0]["factors"] = json!((0..33)
        .map(|index| json!({
            "name": format!("factor-{index}"),
            "points": 0,
            "reason": "client supplied factor"
        }))
        .collect::<Vec<_>>());

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("plan.assignments.factors has too many items"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_too_many_candidate_scores_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["candidate_scores"] = json!((0..9)
        .map(|_| json!({
            "strategy": "solo",
            "score": 0,
            "factors": []
        }))
        .collect::<Vec<_>>());

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("plan.candidate_scores has too many items"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_too_many_plan_approvals_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["approvals"] = json!([
        "start_run",
        "create_worktree",
        "launch_parallel_workers",
        "increase_risk",
        "start_run"
    ]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("plan.approvals has too many items"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_too_many_assignments_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    let assignment = plan["assignments"][0].clone();
    plan["assignments"] = Value::Array(
        (0..=forktty_core::protocol_limits::SOCKET_TASK_STRATEGY_ASSIGNMENT_MAX_COUNT)
            .map(|_| assignment.clone())
            .collect(),
    );

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("too many assignments"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_empty_assignments_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = solo_verify_plan_json();
    plan["assignments"] = json!([]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Fix the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("requires at least one assignment"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_team_layer_for_non_team_strategy_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = solo_verify_plan_json();
    plan["layers"]["team"] = json!(true);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Fix the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("does not support a team layer"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_team_layer_without_workflow_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["layers"]["workflow"] = json!(false);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("team layer requires workflow"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_loop_metadata_without_workflow_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = solo_verify_plan_json();
    plan["layers"]["workflow"] = json!(false);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Fix the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("loop metadata requires workflow"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_loop_metadata_for_unsupported_strategy_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = parallel_research_plan_json();
    plan["layers"]["loop_metadata"] = json!(true);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Compare router implementations",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("does not support loop metadata"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_unsupported_parallel_research_role_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = parallel_research_plan_json();
    plan["assignments"][1]["role"] = json!("synthesizer");
    plan["assignments"][1]["reason"] = json!("client supplied eager synthesizer");

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Compare router implementations",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("does not support role synthesizer"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_extra_non_team_assignments_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = solo_verify_plan_json();
    let extra = plan["assignments"][0].clone();
    plan["assignments"].as_array_mut().unwrap().push(extra);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Fix task strategy routing",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("strategy solo_with_verify_loop supports exactly one assignment"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_overwide_parallel_research_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = parallel_research_plan_json();
    while plan["assignments"].as_array().unwrap().len() < 4 {
        let mut extra = plan["assignments"][0].clone();
        extra["reason"] = json!("client supplied extra research lane");
        plan["assignments"].as_array_mut().unwrap().push(extra);
    }

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Compare router implementations",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("strategy parallel_research supports between 2 and 3 assignments"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_plan_infers_dirty_repo_from_active_surface_cwd() {
    let (state, _backend) = test_state();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
        assert!(model.set_surface_cwd(&surface_id, repo_dir.path().to_path_buf()));
    }

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the router bug",
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["layers"]["worktree"], true);
    assert!(result["approvals"]
        .as_array()
        .unwrap()
        .contains(&json!("create_worktree")));
    assert!(result["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str().unwrap_or_default().contains("dirty repo")));
}

#[tokio::test]
async fn task_strategy_plan_uses_explicit_cwd_for_repo_context() {
    let (state, _backend) = test_state();
    let active_dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
        assert!(model.set_surface_cwd(&surface_id, active_dir.path().to_path_buf()));
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(
            model.set_surface_agent_session_resume_cwd(&surface_id, repo_dir.path().to_path_buf())
        );
    }

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Implement the router fix",
            "cwd": repo_dir.path().to_string_lossy(),
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["layers"]["worktree"], true);
    assert!(result["approvals"]
        .as_array()
        .unwrap()
        .contains(&json!("create_worktree")));
    assert!(result["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| { reason.as_str().unwrap_or_default().contains("explicit cwd") }));
}

#[tokio::test]
async fn task_strategy_plan_rejects_relative_explicit_cwd() {
    let (state, _backend) = test_state();

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Inspect router",
            "cwd": "relative/path"
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("cwd"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_explicit_cwd_outside_open_workspace_repo() {
    let (state, _backend) = test_state();
    let active_dir = tempfile::tempdir().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
        assert!(model.set_surface_cwd(&surface_id, active_dir.path().to_path_buf()));
    }
    let hidden_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(hidden_repo.path()).unwrap();
    std::fs::write(hidden_repo.path().join("dirty.txt"), "dirty\n").unwrap();

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Implement the router fix",
            "cwd": hidden_repo.path().to_string_lossy(),
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("open ForkTTY workspace"));
}

#[tokio::test]
async fn task_strategy_apply_rejects_explicit_cwd_outside_open_workspace_repo() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let active_dir = tempfile::tempdir().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
        assert!(model.set_surface_cwd(&surface_id, active_dir.path().to_path_buf()));
    }
    let hidden_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(hidden_repo.path()).unwrap();

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router fix",
            "approved": ["start_run"],
            "cwd": hidden_repo.path().to_string_lossy(),
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("open ForkTTY workspace"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_plan_infers_user_visible_change_from_editing_goal() {
    let (state, _backend) = test_state();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
        assert!(model.set_surface_cwd(&surface_id, repo_dir.path().to_path_buf()));
    }

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the router bug"
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["layers"]["worktree"], true);
    assert!(result["approvals"]
        .as_array()
        .unwrap()
        .contains(&json!("create_worktree")));
    assert!(result["reasons"].as_array().unwrap().iter().any(|reason| {
        reason
            .as_str()
            .unwrap_or_default()
            .contains("inferred likely user-visible change")
    }));
}

#[tokio::test]
async fn task_strategy_plan_explicit_user_visible_false_overrides_inference() {
    let (state, _backend) = test_state();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
        assert!(model.set_surface_cwd(&surface_id, repo_dir.path().to_path_buf()));
    }

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the router bug",
            "likely_user_visible_change": false
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["layers"]["worktree"], false);
    assert!(!result["approvals"]
        .as_array()
        .unwrap()
        .contains(&json!("create_worktree")));
    assert!(!result["reasons"].as_array().unwrap().iter().any(|reason| {
        reason
            .as_str()
            .unwrap_or_default()
            .contains("inferred likely user-visible change")
    }));
}

#[test]
fn task_strategy_harness_registry_uses_real_provider_capability_shape() {
    let registry = crate::task_strategy_runtime::harness_registry_from_capabilities(&json!({
        "provider_capabilities": {
            "codex": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": true,
                "available_on_path": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false
            },
            "claude": {
                "team_worker_launch": true,
                "launchable": false,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": false,
                "executable": null,
                "disabled_by_config": false
            },
            "opencode": {
                "team_worker_launch": true,
                "launchable": false,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": true,
                "executable": "/usr/bin/opencode",
                "disabled_by_config": true
            }
        }
    }));

    let codex = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "codex")
        .unwrap();
    assert!(codex.installed);
    assert_eq!(codex.health, HarnessHealth::Ready);
    assert!(codex.supports_resume);
    assert!(codex.supports_worktree_cwd);
    assert_eq!(codex.max_parallel_sessions, Some(4));

    let claude = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "claude")
        .unwrap();
    assert!(!claude.installed);
    assert_eq!(claude.health, HarnessHealth::Missing);
    assert!(!claude.supports_worktree_cwd);

    let opencode = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "opencode")
        .unwrap();
    assert!(opencode.installed);
    assert_eq!(opencode.health, HarnessHealth::Disabled);
}

#[test]
fn task_strategy_real_provider_shape_reuses_single_launchable_harness_for_review_roles() {
    let registry = crate::task_strategy_runtime::harness_registry_from_capabilities(&json!({
        "provider_capabilities": {
            "codex": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": true,
                "available_on_path": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false
            }
        }
    }));

    let plan = plan_task_strategy(TaskStrategyInput {
        goal: "Implement the task router".to_string(),
        explicit_mode: None,
        router_profile: None,
        task_class_hint: None,
        last_known_good: None,
        repo_dirty: false,
        user_requested_parallelism: false,
        user_requested_review: true,
        likely_user_visible_change: true,
        harness_registry: registry,
    })
    .unwrap();

    assert_eq!(plan.strategy, TaskStrategy::ImplementerPlusReviewer);
    assert!(plan
        .assignments
        .iter()
        .any(|assignment| assignment.role == HarnessRole::Implementer
            && assignment.harness_id == "codex"));
    assert!(plan
        .assignments
        .iter()
        .any(|assignment| assignment.role == HarnessRole::Reviewer
            && assignment.harness_id == "codex"));
}

#[test]
fn task_strategy_harness_registry_maps_provider_plan_mode_capability() {
    let registry = crate::task_strategy_runtime::harness_registry_from_capabilities(&json!({
        "provider_capabilities": {
            "codex": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": true,
                "available_on_path": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false,
                "plan_mode": false
            },
            "claude": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": true,
                "executable": "/usr/bin/claude",
                "disabled_by_config": false,
                "plan_mode": true
            }
        }
    }));

    let codex = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "codex")
        .unwrap();
    let claude = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "claude")
        .unwrap();

    assert!(!codex.supports_plan_mode);
    assert!(claude.supports_plan_mode);
}

#[tokio::test]
async fn task_strategy_apply_forces_dirty_editing_worktree_approval() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };

    let mut plan = staged_team_plan_json();
    plan["layers"]["worktree"] = json!(false);
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router fix",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("create_worktree"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_uses_plan_task_class_for_dirty_isolation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };

    let mut plan = staged_team_plan_json();
    plan["task_class"] = json!("feature_implementation");
    plan["layers"]["worktree"] = json!(false);
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Bitte diese Änderung umsetzen",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("create_worktree"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_review_only_strategy_cannot_bypass_dirty_mutating_task_class() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };

    let plan = json!({
        "task_class": "feature_implementation",
        "strategy": "review_only",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": false,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "reviewer",
                "harness_id": "claude",
                "reason": "client supplied review strategy with mutating task class"
            }
        ],
        "approvals": ["start_run"],
        "reasons": ["client supplied review strategy"],
        "safety_notes": ["visible setup only"]
    });

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Bitte diese Änderung umsetzen",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("create_worktree"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_read_only_review_does_not_force_dirty_worktree_for_fix_words() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };

    let plan = json!({
        "task_class": "review_only",
        "strategy": "review_only",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": false,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "reviewer",
                "harness_id": "claude",
                "reason": "read-only review"
            }
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as ReviewOnly"],
        "safety_notes": ["visible setup only"]
    });

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Review the bug fix in the task router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    assert_eq!(result["blocked_approvals"], json!([]));
}

#[tokio::test]
async fn task_strategy_apply_uses_mutating_strategy_for_dirty_isolation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };
    let plan = json!({
        "task_class": "repo_inspection",
        "strategy": "solo_with_verify_loop",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "implementer",
                "harness_id": "codex",
                "reason": "verify-loop harness"
            }
        ],
        "approvals": ["start_run"],
        "reasons": ["client supplied stale task class"],
        "safety_notes": ["visible setup only"]
    });

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Bitte diese Änderung umsetzen",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("create_worktree"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_forces_dirty_isolation_for_common_editing_verbs() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };
    let goals = [
        "Fixing legacy parser bugs",
        "Fixed bugfix parser path",
        "Adding parser fallback",
        "Delete legacy flag from parser",
        "Replace legacy flag in parser",
        "Rewrite legacy flag parser",
        "Migrate legacy flag parser",
        "Dropping legacy flag parser",
        "Drop legacy flag parser",
        "Convert legacy flag parser",
        "Porting legacy flag parser",
        "Port legacy flag parser",
        "Patch legacy flag parser",
    ];

    for (index, goal) in goals.iter().enumerate() {
        let mut plan = staged_team_plan_json();
        plan["layers"]["worktree"] = json!(false);
        plan["approvals"] = json!(["start_run"]);
        let err = dispatch(
            &state,
            "task.strategy.apply",
            json!({
                "run_id": format!("router-run-{index}"),
                "worktree_name": "feature-x",
                "goal": goal,
                "approved": ["start_run"],
                "plan": plan
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "precondition_failed", "{goal}");
        assert!(err.to_string().contains("create_worktree"), "{goal}");
    }
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_does_not_force_dirty_isolation_for_edit_prefix_false_positives() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    std::fs::write(repo_dir.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", repo_dir.path(), "feature", "feature-x");
    };
    let plan = json!({
        "task_class": "repo_inspection",
        "strategy": "solo_tracked",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": false,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "implementer",
                "harness_id": "codex",
                "reason": "inspection harness"
            }
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as RepoInspection"],
        "safety_notes": ["visible setup only"]
    });

    let applied = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Review dropdown fixture portability",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap();

    assert_eq!(applied["status"], "staged");
    assert_eq!(applied["blocked_approvals"], json!([]));
}

#[tokio::test]
async fn task_strategy_apply_rejects_conflicting_surface_aliases_before_dirty_check() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let dirty_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(dirty_repo.path()).unwrap();
    std::fs::write(dirty_repo.path().join("dirty.txt"), "dirty\n").unwrap();
    let clean_dir = tempfile::tempdir().unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let dirty_surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    let clean_surface = dispatch(
        &state,
        "pane.new_tab",
        json!({"surface_id": dirty_surface_id}),
    )
    .await
    .unwrap();
    let clean_surface_id = clean_surface["id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(dirty_surface_id, dirty_repo.path().to_path_buf()));
        assert!(model.set_surface_cwd(clean_surface_id, clean_dir.path().to_path_buf()));
    }

    let mut plan = staged_team_plan_json();
    plan["layers"]["worktree"] = json!(false);
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "surface_id": clean_surface_id,
            "leader_surface_id": dirty_surface_id,
            "goal": "Implement the router fix",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("surface_id and leader_surface_id"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_uses_worktree_cwd_for_dirty_isolation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let dirty_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(dirty_repo.path()).unwrap();
    std::fs::write(dirty_repo.path().join("dirty.txt"), "dirty\n").unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", dirty_repo.path(), "feature", "feature-x");
    };

    let mut plan = staged_team_plan_json();
    plan["layers"]["worktree"] = json!(false);
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router fix",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("create_worktree"));
}

#[tokio::test]
async fn task_strategy_apply_uses_explicit_cwd_for_dirty_isolation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let dirty_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(dirty_repo.path()).unwrap();
    std::fs::write(dirty_repo.path().join("dirty.txt"), "dirty\n").unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, dirty_repo.path().to_path_buf()));
    }

    let mut plan = staged_team_plan_json();
    plan["layers"]["worktree"] = json!(false);
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": dirty_repo.path(),
            "goal": "Implement the router fix",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("worktree_name"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_explicit_clean_cwd_ignores_dirty_leader_surface_for_isolation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let dirty_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(dirty_repo.path()).unwrap();
    std::fs::write(dirty_repo.path().join("dirty.txt"), "dirty\n").unwrap();
    let clean_repo = tempfile::tempdir().unwrap();
    git2::Repository::init(clean_repo.path()).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let dirty_surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(dirty_surface_id, dirty_repo.path().to_path_buf()));
        model.create_workspace("clean-target", clean_repo.path());
    }

    let mut plan = staged_team_plan_json();
    plan["layers"]["worktree"] = json!(false);
    plan["approvals"] = json!(["start_run"]);

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": dirty_surface_id,
            "cwd": clean_repo.path(),
            "goal": "Implement the router fix",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    assert!(result["blocked_approvals"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_conflicting_workspace_selectors_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": "workspace-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("cannot combine"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_workspace_alias_targets_same_workspace_for_context_and_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let active_workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let active_workspace_id = active_workspace[0]["id"].as_str().unwrap().to_string();
    let target_workspace = dispatch(
        &state,
        "workspace.create",
        json!({"name": "target", "workingDir": target_dir.path()}),
    )
    .await
    .unwrap();
    let target_workspace_id = target_workspace["id"].as_str().unwrap();
    dispatch(
        &state,
        "workspace.select",
        json!({"id": active_workspace_id}),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspaceId": target_workspace_id,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    let workflow = dispatch(
        &state,
        "workflow.get",
        json!({"workflow_id": "router-run-1"}),
    )
    .await
    .unwrap();
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(workflow["workspace_id"], target_workspace_id);
    assert_eq!(team["workspace_id"], target_workspace_id);
}

#[test]
fn task_strategy_harness_registry_respects_team_provider_order() {
    let registry = crate::task_strategy_runtime::harness_registry_from_capabilities(&json!({
        "team_provider_policy": {
            "provider_order": ["opencode", "codex"]
        },
        "provider_capabilities": {
            "codex": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": true,
                "available_on_path": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false
            },
            "opencode": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": true,
                "executable": "/usr/bin/opencode",
                "disabled_by_config": false
            }
        }
    }));

    let ordered = registry
        .harnesses
        .iter()
        .map(|harness| harness.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ordered, vec!["opencode", "codex"]);
}

#[test]
fn task_strategy_harness_signals_apply_cooldown_and_lockout_separately() {
    let mut registry = crate::task_strategy_runtime::harness_registry_from_capabilities(&json!({
        "team_provider_policy": {
            "provider_order": ["codex", "claude"]
        },
        "provider_capabilities": {
            "codex": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": true,
                "available_on_path": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false
            },
            "claude": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": true,
                "executable": "/usr/bin/claude",
                "disabled_by_config": false
            }
        }
    }));

    crate::task_strategy_runtime::apply_harness_routing_signals(
        &mut registry,
        &json!({
            "codex": {
                "cooldown": true,
                "cooldown_reason": "recent quota error"
            },
            "claude": {
                "locked_out": true,
                "lockout_reason": "task mode disabled"
            }
        }),
    )
    .unwrap();

    let codex = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "codex")
        .unwrap();
    assert!(codex.routing_signals.cooldown);
    assert_eq!(
        codex.routing_signals.cooldown_reason.as_deref(),
        Some("recent quota error")
    );
    assert!(!codex.routing_signals.locked_out);

    let claude = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "claude")
        .unwrap();
    assert!(claude.routing_signals.locked_out);
    assert_eq!(
        claude.routing_signals.lockout_reason.as_deref(),
        Some("task mode disabled")
    );
    assert!(!claude.routing_signals.cooldown);
}

#[tokio::test]
async fn task_strategy_plan_rejects_malformed_harness_signals() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Inspect router state",
            "harness_signals": {
                "codex": {
                    "cooldown": "yes"
                }
            }
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("harness_signals.codex.cooldown"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_oversized_harness_signal_reason() {
    let (state, _backend) = test_state();
    let reason =
        "x".repeat(forktty_core::protocol_limits::SOCKET_TASK_STRATEGY_REASON_MAX_BYTES + 1);

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Inspect router state",
            "harness_signals": {
                "codex": {
                    "cooldown": true,
                    "cooldown_reason": reason
                }
            }
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "payload_too_large");
    assert!(err.to_string().contains("harness_signals.reason"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_control_characters_in_harness_signal_reason() {
    let (state, _backend) = test_state();

    for reason in ["quota\u{1b}[31m", "quota\r"] {
        let err = dispatch(
            &state,
            "task.strategy.plan",
            json!({
                "goal": "Inspect router state",
                "harness_signals": {
                    "codex": {
                        "cooldown": true,
                        "cooldown_reason": reason
                    }
                }
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("control characters"));
    }
}

#[tokio::test]
async fn task_strategy_plan_rejects_oversized_last_known_good_reason() {
    let (state, _backend) = test_state();
    let reason =
        "x".repeat(forktty_core::protocol_limits::SOCKET_TASK_STRATEGY_REASON_MAX_BYTES + 1);

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task router bug",
            "last_known_good": {
                "strategy": "solo_with_verify_loop",
                "harness_id": "codex",
                "reason": reason
            }
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "payload_too_large");
    assert!(err.to_string().contains("last_known_good.reason"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_control_characters_in_last_known_good_reason() {
    let (state, _backend) = test_state();

    for reason in ["previous\u{1b}[31mrun", "previous run\r"] {
        let err = dispatch(
            &state,
            "task.strategy.plan",
            json!({
                "goal": "Fix the task router bug",
                "last_known_good": {
                    "strategy": "solo_with_verify_loop",
                    "reason": reason
                }
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("last_known_good.reason"));
    }
}

#[tokio::test]
async fn task_strategy_plan_rejects_invalid_last_known_good_harness_id() {
    let (state, _backend) = test_state();

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task router bug",
            "last_known_good": {
                "harness_id": "codex with spaces"
            }
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("last_known_good.harness_id"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_invalid_harness_signal_key() {
    let (state, _backend) = test_state();

    let err = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the task router bug",
            "harness_signals": {
                "codex with spaces": {
                    "cooldown": true
                }
            }
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("harness_signals key"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_blank_goal() {
    let (state, _backend) = test_state();
    let err = dispatch(&state, "task.strategy.plan", json!({ "goal": " " }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("goal"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_oversized_goal() {
    let (state, _backend) = test_state();
    let goal = "x".repeat(task_strategy_params::MAX_TASK_STRATEGY_GOAL_BYTES + 1);

    let err = dispatch(&state, "task.strategy.plan", json!({ "goal": goal }))
        .await
        .unwrap_err();

    assert_eq!(err.code(), "payload_too_large");
    assert!(err.to_string().contains("goal"));
}

#[tokio::test]
async fn task_strategy_plan_rejects_terminal_control_characters_in_goal() {
    let (state, _backend) = test_state();

    for goal in [
        "Fix router\u{1b}[31m",
        "Fix router\rsubmit early",
        "Fix router\r",
    ] {
        let err = dispatch(&state, "task.strategy.plan", json!({ "goal": goal }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("terminal control characters"));
    }
}

#[tokio::test]
async fn task_strategy_plan_accepts_multiline_goal_without_terminal_controls() {
    let (state, _backend) = test_state();

    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the router\nRun focused tests",
            "repo_dirty": false
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["task_class"], "focused_bugfix");
}

#[tokio::test]
async fn task_strategy_apply_rejects_oversized_goal_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let goal = "x".repeat(task_strategy_params::MAX_TASK_STRATEGY_GOAL_BYTES + 1);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": goal,
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "payload_too_large");
    assert!(err.to_string().contains("goal"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_terminal_control_characters_in_goal_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    for goal in ["Implement router\rsubmit early", "Implement router\r"] {
        let err = dispatch(
            &state,
            "task.strategy.apply",
            json!({
                "run_id": "router-run-1",
                "goal": goal,
                "approved": ["start_run"],
                "plan": staged_team_plan_json()
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("terminal control characters"));
        let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
        let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
        assert!(workflows.as_array().unwrap().is_empty());
        assert!(teams.as_array().unwrap().is_empty());
    }
}

#[tokio::test]
async fn task_strategy_apply_rejects_missing_start_approval_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("start_run"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_requires_start_run_even_if_plan_omits_it() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["approvals"] = json!([]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("start_run"));
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_requires_worktree_approval_even_if_plan_omits_it() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let worktree_dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", worktree_dir.path(), "feature", "feature-x");
    }
    let mut plan = worktree_team_plan_json();
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement in a worktree",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("create_worktree"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_requires_parallel_approval_even_if_plan_omits_it() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = parallel_research_plan_json();
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Compare router approaches",
            "approved": ["start_run"],
            "submit": true,
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("launch_parallel_workers"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_parallel_research_prompts_include_distinct_lanes() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Review the router in parallel",
            "approved": ["start_run"],
            "plan": parallel_research_two_researchers_plan_json()
        }),
    )
    .await
    .unwrap();

    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let messages = team["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    let first_body = messages[0]["body"].as_str().unwrap();
    let second_body = messages[1]["body"].as_str().unwrap();
    assert_ne!(first_body, second_body);
    assert!(first_body.contains("Lane: correctness and regressions"));
    assert!(second_body.contains("Lane: tests, edge cases, and missing coverage"));

    let tasks = team["tasks"].as_array().unwrap();
    assert!(tasks[0]["detail"]
        .as_str()
        .unwrap()
        .contains("Lane: correctness and regressions"));
    assert!(tasks[1]["detail"]
        .as_str()
        .unwrap()
        .contains("Lane: tests, edge cases, and missing coverage"));
}

#[tokio::test]
async fn task_strategy_apply_can_request_missing_approval_without_workflow_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "blocked");
    assert_eq!(result["blocked_approvals"], json!(["start_run"]));
    let approval_id = result["approval_request"]["id"].as_str().unwrap();
    assert!(approval_id.starts_with("task-strategy:"));
    assert!(approval_id.ends_with(":approvals:start_run"));
    assert_eq!(result["approval_request"]["approval_state"], "pending");
    assert_eq!(result["actions"][0]["method"], "feed.approval.request");
    assert_eq!(result["actions"][0]["status"], "applied");

    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());

    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();
    assert_eq!(feed.as_array().unwrap().len(), 1);
    assert_eq!(feed[0]["type"], "approval");
    assert_eq!(feed[0]["kind"], "task_strategy");
    assert_eq!(feed[0]["approval_state"], "pending");
    assert!(feed[0]["body"].as_str().unwrap().contains("start_run"));
}

#[tokio::test]
async fn task_strategy_apply_approval_request_is_idempotent() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();
    let params = json!({
        "run_id": "router-run-1",
        "goal": "Implement the router",
        "approved": [],
        "request_approval": true,
        "plan": staged_team_plan_json()
    });

    let first = dispatch(&state, "task.strategy.apply", params.clone())
        .await
        .unwrap();
    let second = dispatch(&state, "task.strategy.apply", params)
        .await
        .unwrap();

    assert_eq!(
        first["approval_request"]["id"],
        second["approval_request"]["id"]
    );
    assert_eq!(second["actions"][0]["status"], "already_exists");
    let feed = dispatch(&state, "feed.list", json!({"limit": 10}))
        .await
        .unwrap();
    assert_eq!(feed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn task_strategy_apply_dismisses_superseded_pending_approval_request() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();

    let approval = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let approval_id = approval["approval_request"]["id"].as_str().unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "feed.approval.dismiss" && action["approval_id"] == approval_id
    }));
    let feed = dispatch(&state, "feed.list", json!({"limit": 10}))
        .await
        .unwrap();
    assert_eq!(feed[0]["id"], approval_id);
    assert_eq!(feed[0]["approval_state"], "dismissed");
}

#[tokio::test]
async fn task_strategy_dismissed_approval_request_cannot_be_reapproved() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();

    let approval = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let approval_id = approval["approval_request"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approve"}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("not pending"));
    let feed = dispatch(&state, "feed.list", json!({"limit": 10}))
        .await
        .unwrap();
    assert_eq!(feed[0]["id"], approval_id);
    assert_eq!(feed[0]["approval_state"], "dismissed");
}

#[tokio::test]
async fn task_strategy_apply_dismisses_superseded_pending_approval_when_approval_id_is_also_sent() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();

    let approval = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let approval_id = approval["approval_request"]["id"].as_str().unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "approval_id": approval_id,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "feed.approval.dismiss" && action["approval_id"] == approval_id
    }));
    let feed = dispatch(&state, "feed.list", json!({"limit": 10}))
        .await
        .unwrap();
    assert_eq!(feed[0]["id"], approval_id);
    assert_eq!(feed[0]["approval_state"], "dismissed");
}

#[tokio::test]
async fn task_strategy_apply_uses_approved_feed_request_as_start_approval() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();

    let approval = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let approval_id = approval["approval_request"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approve"}),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "approval_id": approval_id,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    assert_eq!(result["blocked_approvals"], json!([]));
    let workflow = dispatch(
        &state,
        "workflow.get",
        json!({"workflow_id": "router-run-1"}),
    )
    .await
    .unwrap();
    assert_eq!(workflow["status"], "staged");
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_approved_superset_feed_request_satisfies_remaining_approvals() {
    let (mut state, _backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();
    let plan = parallel_research_plan_json();

    let approval = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Compare router approaches",
            "approved": [],
            "request_approval": true,
            "submit": true,
            "plan": plan.clone()
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        approval["blocked_approvals"],
        json!(["start_run", "launch_parallel_workers"])
    );
    let approval_id = approval["approval_request"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approve"}),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Compare router approaches",
            "approved": ["start_run"],
            "approval_id": approval_id,
            "submit": true,
            "plan": plan
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "running");
    assert_eq!(result["blocked_approvals"], json!([]));
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(team["workers"].as_array().unwrap().len(), 2);
    assert!(team["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["delivered"] == true));
}

#[tokio::test]
async fn task_strategy_apply_rejects_approval_id_for_different_goal_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();

    let approval = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let approval_id = approval["approval_request"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approve"}),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Launch a different run",
            "approved": [],
            "approval_id": approval_id,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err
        .to_string()
        .contains("does not match required approval request"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_request_approval_rejects_missing_worktree_name_without_feed_mutation()
{
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement in a worktree",
            "approved": [],
            "request_approval": true,
            "submit": true,
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("worktree_name"));
    let feed = dispatch(&state, "feed.list", json!({"limit": 10}))
        .await
        .unwrap();
    assert!(feed.as_array().unwrap().is_empty());
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_request_approval_rejects_team_submit_without_assignments() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let state = state
        .with_feed_store_path(dir.path().join("feed.json"))
        .unwrap();
    let mut plan = staged_team_plan_json();
    plan["assignments"] = json!([]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": [],
            "request_approval": true,
            "submit": true,
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("at least one team assignment"));
    let feed = dispatch(&state, "feed.list", json!({"limit": 10}))
        .await
        .unwrap();
    assert!(feed.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_team_layer_without_assignments_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["assignments"] = json!([]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("at least one team assignment"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_submit_rejects_unlaunchable_assignment_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["assignments"][0]["harness_id"] = json!("self");

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("self"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_submit_rejects_non_team_plan_without_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Verify the fix",
            "approved": ["start_run"],
            "submit": true,
            "plan": solo_verify_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("team layer"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_submit_requires_parallel_workers_approval_for_multiple_assignments() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement with review",
            "approved": ["start_run"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("launch_parallel_workers"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_stages_workflow_team_tasks_and_messages() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["run_id"], "router-run-1");
    assert_eq!(result["status"], "staged");
    assert_eq!(result["workflow_id"], "router-run-1");
    assert_eq!(result["team_id"], "router-run-1");
    assert_eq!(result["blocked_approvals"], json!([]));
    let action_methods = result["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|action| action["method"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        action_methods,
        vec![
            "workflow.upsert",
            "workflow.plan.set",
            "workflow.loop.set",
            "team.upsert",
            "team.task.upsert",
            "team.message.send",
            "team.task.upsert",
            "team.message.send",
        ]
    );

    let workflow = dispatch(
        &state,
        "workflow.get",
        json!({"workflow_id": "router-run-1"}),
    )
    .await
    .unwrap();
    assert_eq!(workflow["status"], "staged");
    assert_eq!(workflow["mode"], "task_strategy");
    assert!(
        workflow["memory"]
            .as_str()
            .unwrap_or_default()
            .contains("Task strategy: implementer_plus_reviewer."),
        "workflow memory should persist the stable task strategy wire value"
    );
    assert_eq!(workflow["plan"].as_array().unwrap().len(), 2);
    assert_eq!(workflow["loop_stage"], "staged");

    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "staged");
    assert_eq!(team["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(team["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        team["messages"][0]["id"],
        "router-run-1-implementer-1-msg-1"
    );
    assert_eq!(team["messages"][0]["to_worker_id"], Value::Null);
    assert_eq!(team["messages"][0]["delivered"], false);
}

#[tokio::test]
async fn task_strategy_apply_stage_ignores_parallel_worker_approval_until_submit() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = staged_team_plan_json();
    plan["approvals"] = json!(["start_run", "launch_parallel_workers"]);

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": plan
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "staged");
    assert_eq!(result["blocked_approvals"], json!([]));
    assert!(result["actions"].as_array().unwrap().iter().all(|action| {
        action["method"] != "team.worker.launch" && action["method"] != "team.message.dispatch"
    }));
}

#[tokio::test]
async fn task_strategy_apply_is_idempotent_for_staged_messages() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let params = json!({
        "run_id": "router-run-1",
        "goal": "Implement the router",
        "approved": ["start_run"],
        "plan": staged_team_plan_json()
    });

    dispatch(&state, "task.strategy.apply", params.clone())
        .await
        .unwrap();
    let second = dispatch(&state, "task.strategy.apply", params)
        .await
        .unwrap();

    let message_actions = second["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["method"] == "team.message.send")
        .collect::<Vec<_>>();
    assert_eq!(message_actions.len(), 2);
    assert!(message_actions
        .iter()
        .all(|action| action["status"] == "already_exists"));
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(team["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn task_strategy_apply_staged_prompt_suffix_handles_u32_max_message_id() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "router-run-1",
            "message_id": "router-run-1-implementer-1-msg-4294967295",
            "from": "leader",
            "task_id": "router-run-1-implementer-1",
            "body": "stale prompt"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router safely",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "team.message.send"
            && action["message_id"] == "router-run-1-implementer-1-msg-4294967296"
            && action["status"] == "applied"
    }));
}

#[tokio::test]
async fn task_strategy_apply_staged_prompt_with_changed_cwd_queues_new_message() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    let first_dir = repo_dir.path().join("first");
    let second_dir = repo_dir.path().join("second");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, repo_dir.path().to_path_buf()));
    }

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": first_dir,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": second_dir,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "team.message.send"
            && action["message_id"] == "router-run-1-implementer-1-msg-2"
            && action["status"] == "applied"
    }));
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(team["messages"].as_array().unwrap().len(), 4);
    assert!(team["messages"].as_array().unwrap().iter().any(|message| {
        message["id"] == "router-run-1-implementer-1-msg-2"
            && message["delivered"] == false
            && message["body"]
                .as_str()
                .unwrap_or_default()
                .contains(second_dir.to_str().unwrap())
    }));
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_launches_visible_workers_and_dispatches_prompts() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "running");
    assert_eq!(result["blocked_approvals"], json!([]));
    let result_json = serde_json::to_string(&result).unwrap();
    assert!(
        result_json.len() <= 8_000,
        "task.strategy.apply submit response is {} bytes; return action summaries, not full team records",
        result_json.len()
    );
    assert!(!result_json.contains("ForkTTY task assignment."));
    assert!(!result_json.contains("The leader already applied this task strategy"));
    let action_methods = result["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|action| action["method"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        action_methods,
        vec![
            "workflow.upsert",
            "workflow.plan.set",
            "workflow.loop.set",
            "team.upsert",
            "team.task.upsert",
            "team.worker.launch",
            "team.task.upsert",
            "team.message.send",
            "team.message.dispatch",
            "team.task.upsert",
            "team.worker.launch",
            "team.task.upsert",
            "team.message.send",
            "team.message.dispatch",
        ]
    );

    let workflow = dispatch(
        &state,
        "workflow.get",
        json!({"workflow_id": "router-run-1"}),
    )
    .await
    .unwrap();
    assert_eq!(workflow["status"], "running");
    assert_eq!(workflow["loop_stage"], "running");

    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(team["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(team["workers"].as_array().unwrap().len(), 2);
    assert_eq!(team["messages"].as_array().unwrap().len(), 2);
    assert!(team["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|task| task["status"] == "running"));
    assert!(team["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["delivered"] == true));

    let implementer = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let reviewer = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-reviewer-2-worker")
        .unwrap();
    let implementer_surface = implementer["surface_id"].as_str().unwrap();
    let reviewer_surface = reviewer["surface_id"].as_str().unwrap();
    assert_eq!(backend.spawn_shell(implementer_surface).unwrap(), "codex");
    assert_eq!(backend.spawn_shell(reviewer_surface).unwrap(), "claude");
    assert_eq!(backend.sent_text(implementer_surface).unwrap().len(), 2);
    assert_eq!(backend.sent_text(reviewer_surface).unwrap().len(), 2);
    let implementer_text = backend.sent_text(implementer_surface).unwrap().join("");
    assert!(implementer_text.contains("The leader already applied this task strategy"));
    assert!(implementer_text.contains("do not call task.strategy.apply"));
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_retry_after_codex_enter_failure_does_not_resend_prompt() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingEnterBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    let params = json!({
        "run_id": "router-run-1",
        "workspace_id": workspace_id,
        "leader_surface_id": surface_id,
        "goal": "Implement the router",
        "approved": ["start_run", "launch_parallel_workers"],
        "submit": true,
        "plan": staged_team_plan_json()
    });

    let first = dispatch(&state, "task.strategy.apply", params.clone())
        .await
        .unwrap_err();
    assert_eq!(first.code(), "error");
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let implementer = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let implementer_surface_id = implementer["surface_id"].as_str().unwrap();
    assert_eq!(backend.sent_text(implementer_surface_id).unwrap().len(), 1);

    let retry = dispatch(&state, "task.strategy.apply", params)
        .await
        .unwrap_err();
    assert_eq!(retry.code(), "error");
    assert_eq!(backend.sent_text(implementer_surface_id).unwrap().len(), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_uses_explicit_cwd_for_worker_launches_and_prompts() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    let leader_dir = repo_dir.path().join("leader");
    std::fs::create_dir_all(&leader_dir).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, leader_dir.clone()));
    }

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": repo_dir.path(),
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "running");
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    for worker in team["workers"].as_array().unwrap() {
        let surface_id = worker["surface_id"].as_str().unwrap();
        let surface = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == surface_id)
            .unwrap();
        assert_eq!(surface.cwd, repo_dir.path());
    }
    assert!(team["tasks"].as_array().unwrap().iter().all(|task| {
        task["detail"]
            .as_str()
            .unwrap_or_default()
            .contains(repo_dir.path().to_str().unwrap())
    }));
    assert!(team["messages"].as_array().unwrap().iter().all(|message| {
        message["body"]
            .as_str()
            .unwrap_or_default()
            .contains(repo_dir.path().to_str().unwrap())
    }));
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_reuses_worker_when_explicit_cwd_spelling_changes() {
    use std::os::unix::fs::symlink;

    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    let symlink_dir = tempfile::tempdir().unwrap();
    let repo_link = symlink_dir.path().join("repo-link");
    symlink(repo_dir.path(), &repo_link).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, repo_dir.path().to_path_buf()));
    }
    let first_params = json!({
        "run_id": "router-run-1",
        "workspace_id": workspace_id,
        "leader_surface_id": surface_id,
        "cwd": repo_link,
        "goal": "Implement the router",
        "approved": ["start_run", "launch_parallel_workers"],
        "submit": true,
        "plan": staged_team_plan_json()
    });
    let retry_params = json!({
        "run_id": "router-run-1",
        "workspace_id": workspace_id,
        "leader_surface_id": surface_id,
        "cwd": repo_dir.path(),
        "goal": "Implement the router",
        "approved": ["start_run", "launch_parallel_workers"],
        "submit": true,
        "plan": staged_team_plan_json()
    });

    dispatch(&state, "task.strategy.apply", first_params)
        .await
        .unwrap();
    let first_team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let first_implementer = first_team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let first_surface_id = first_implementer["surface_id"]
        .as_str()
        .unwrap()
        .to_string();
    let sent_before_retry = backend.sent_text(&first_surface_id).unwrap().len();

    let retry = dispatch(&state, "task.strategy.apply", retry_params)
        .await
        .unwrap();

    assert_eq!(retry["status"], "running");
    let retry_team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let retry_implementer = retry_team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    assert_eq!(
        retry_implementer["surface_id"].as_str().unwrap(),
        first_surface_id
    );
    assert_eq!(
        backend.sent_text(&first_surface_id).unwrap().len(),
        sent_before_retry
    );
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_does_not_reuse_staged_prompt_with_stale_cwd() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    let staged_dir = repo_dir.path().join("staged");
    let submit_dir = repo_dir.path().join("submit");
    std::fs::create_dir_all(&staged_dir).unwrap();
    std::fs::create_dir_all(&submit_dir).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, repo_dir.path().to_path_buf()));
    }

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": staged_dir,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": submit_dir,
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "team.message.send"
            && action["message_id"] == "router-run-1-implementer-1-msg-2"
            && action["status"] == "applied"
    }));
    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let implementer = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let implementer_surface_id = implementer["surface_id"].as_str().unwrap();
    let sent = backend.sent_text(implementer_surface_id).unwrap().join("");
    assert!(sent.contains(submit_dir.to_str().unwrap()));
    assert!(!sent.contains(staged_dir.to_str().unwrap()));
    let inbox = dispatch(
        &state,
        "team.inbox",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker"
        }),
    )
    .await
    .unwrap();
    assert!(inbox.as_array().unwrap().is_empty());
    let messages = team["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["id"] == "router-run-1-implementer-1-msg-1"
            && message["delivered"] == false
            && message["superseded"] == true
            && message["body"]
                .as_str()
                .unwrap_or_default()
                .contains(staged_dir.to_str().unwrap())
    }));
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_relaunches_worker_when_record_surface_runtime_is_missing() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let stale_worker = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker",
            "agent": "codex",
            "role": "implementer",
            "assigned_task_id": "router-run-1-implementer-1"
        }),
    )
    .await
    .unwrap();
    let stale_surface_id = stale_worker["surface"]["id"].as_str().unwrap().to_string();
    backend.close(&stale_surface_id).unwrap();
    assert!(state
        .model
        .lock()
        .unwrap()
        .surface(&stale_surface_id)
        .is_some());

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "running");
    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "team.worker.launch"
            && action["worker_id"] == "router-run-1-implementer-1-worker"
            && action["status"] == "applied"
    }));

    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let implementer = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let relaunched_surface_id = implementer["surface_id"].as_str().unwrap();
    assert_ne!(relaunched_surface_id, stale_surface_id);
    assert!(team["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["delivered"] == true));
    assert!(backend.sent_text(&stale_surface_id).is_err());
    assert_eq!(backend.spawn_shell(relaunched_surface_id).unwrap(), "codex");
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_redispatches_prompt_after_worker_relaunch() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, repo_dir.path().to_path_buf()));
    }
    let params = json!({
        "run_id": "router-run-1",
        "workspace_id": workspace_id,
        "leader_surface_id": surface_id,
        "cwd": repo_dir.path(),
        "goal": "Implement the router",
        "approved": ["start_run", "launch_parallel_workers"],
        "submit": true,
        "plan": staged_team_plan_json()
    });

    dispatch(&state, "task.strategy.apply", params.clone())
        .await
        .unwrap();
    let first_team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let first_implementer = first_team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let stale_surface_id = first_implementer["surface_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!backend.sent_text(&stale_surface_id).unwrap().is_empty());
    backend.close(&stale_surface_id).unwrap();

    let result = dispatch(&state, "task.strategy.apply", params)
        .await
        .unwrap();

    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "team.worker.launch"
            && action["worker_id"] == "router-run-1-implementer-1-worker"
            && action["status"] == "applied"
    }));
    assert!(result["actions"].as_array().unwrap().iter().any(|action| {
        action["method"] == "team.message.dispatch"
            && action["message_id"] == "router-run-1-implementer-1-msg-2"
            && action["status"] == "applied"
    }));
    let second_team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    let relaunched_implementer = second_team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "router-run-1-implementer-1-worker")
        .unwrap();
    let relaunched_surface_id = relaunched_implementer["surface_id"].as_str().unwrap();
    assert_ne!(relaunched_surface_id, stale_surface_id);
    let relaunched_text = backend.sent_text(relaunched_surface_id).unwrap().join("");
    assert!(relaunched_text.contains("ForkTTY task assignment."));
    assert!(relaunched_text.contains(repo_dir.path().to_str().unwrap()));
    assert!(second_team["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            message["id"] == "router-run-1-implementer-1-msg-2"
                && message["delivered"] == true
                && message["body"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(repo_dir.path().to_str().unwrap())
        }));
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_rejects_stale_explicit_cwd_worker_when_retry_omits_cwd() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    let leader_dir = repo_dir.path().join("leader");
    let stale_dir = repo_dir.path().join("stale");
    std::fs::create_dir_all(&leader_dir).unwrap();
    std::fs::create_dir_all(&stale_dir).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, leader_dir.clone()));
    }

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker",
            "agent": "codex",
            "role": "implementer",
            "assigned_task_id": "router-run-1-implementer-1",
            "cwd": stale_dir
        }),
    )
    .await
    .unwrap();
    let stale_surface_id = launched["surface"]["id"].as_str().unwrap();

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("cwd expected"));
    assert!(backend.sent_text(stale_surface_id).unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_rejects_live_worker_cwd_mismatch_before_dispatch() {
    let (mut state, _backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    git2::Repository::init(repo_dir.path()).unwrap();
    let leader_dir = repo_dir.path().join("leader");
    std::fs::create_dir_all(&leader_dir).unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_cwd(surface_id, leader_dir.clone()));
    }

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker",
            "agent": "codex",
            "role": "implementer",
            "assigned_task_id": "router-run-1-implementer-1"
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "workspace_id": workspace_id,
            "leader_surface_id": surface_id,
            "cwd": repo_dir.path(),
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("cwd expected"));
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_rejects_live_worker_harness_mismatch_before_dispatch() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "router-run-1",
            "name": "Task strategy router-run-1",
            "goal": "Implement the router"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.task.upsert",
        json!({
            "team_id": "router-run-1",
            "task_id": "router-run-1-implementer-1",
            "title": "Implement via codex",
            "status": "open"
        }),
    )
    .await
    .unwrap();
    let existing = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker",
            "agent": "codex",
            "role": "implementer",
            "assigned_task_id": "router-run-1-implementer-1"
        }),
    )
    .await
    .unwrap();
    let existing_surface_id = existing["surface"]["id"].as_str().unwrap().to_string();

    let mut plan = staged_team_plan_json();
    plan["assignments"][0]["harness_id"] = json!("claude");
    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err
        .to_string()
        .contains("router-run-1-implementer-1-worker"));
    assert!(err.to_string().contains("different assignment"));
    assert!(backend.sent_text(&existing_surface_id).unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_rejects_live_worker_blocked_status_before_dispatch() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "router-run-1",
            "name": "Task strategy router-run-1",
            "goal": "Implement the router"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.task.upsert",
        json!({
            "team_id": "router-run-1",
            "task_id": "router-run-1-implementer-1",
            "title": "Implement via codex",
            "status": "open"
        }),
    )
    .await
    .unwrap();
    let existing = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker",
            "agent": "codex",
            "role": "implementer",
            "assigned_task_id": "router-run-1-implementer-1"
        }),
    )
    .await
    .unwrap();
    let existing_surface_id = existing["surface"]["id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "router-run-1",
            "worker_id": "router-run-1-implementer-1-worker",
            "status": "blocked"
        }),
    )
    .await
    .unwrap();

    let mut plan = staged_team_plan_json();
    plan["approvals"] = json!(["start_run", "launch_parallel_workers"]);
    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run", "launch_parallel_workers"],
            "submit": true,
            "plan": plan
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("status blocked is not reusable"));
    assert!(backend.sent_text(&existing_surface_id).unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_submit_requires_worktree_name_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run", "create_worktree"],
            "submit": true,
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("worktree_name"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_staged_worktree_requires_worktree_name_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "goal": "Implement the router",
            "approved": ["start_run", "create_worktree"],
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("worktree_name"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_rejects_combined_worktree_name_and_cwd_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "cwd": repo_dir.path(),
            "goal": "Implement the router",
            "approved": ["start_run"],
            "plan": staged_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("cannot combine worktree_name and cwd"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_strategy_apply_submit_rejects_unopened_worktree_before_mutation() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router",
            "approved": ["start_run", "create_worktree", "launch_parallel_workers"],
            "submit": true,
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("feature-x"));
    let workflows = dispatch(&state, "workflow.list", json!({})).await.unwrap();
    let teams = dispatch(&state, "team.list", json!({})).await.unwrap();
    assert!(workflows.as_array().unwrap().is_empty());
    assert!(teams.as_array().unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_launches_workers_in_open_worktree_workspace() {
    let (mut state, backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let worktree_dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let (worktree_workspace_id, worktree_surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace =
            model.create_worktree_workspace("feature", worktree_dir.path(), "feature", "feature-x");
        (workspace.id, workspace.focused_surface_id)
    };

    let result = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router",
            "approved": ["start_run", "create_worktree", "launch_parallel_workers"],
            "submit": true,
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["status"], "running");
    let workflow = dispatch(
        &state,
        "workflow.get",
        json!({"workflow_id": "router-run-1"}),
    )
    .await
    .unwrap();
    assert_eq!(workflow["workspace_id"], worktree_workspace_id);

    let team = dispatch(&state, "team.get", json!({"team_id": "router-run-1"}))
        .await
        .unwrap();
    assert_eq!(team["workspace_id"], worktree_workspace_id);
    assert_eq!(team["workers"].as_array().unwrap().len(), 2);
    assert!(team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|worker| worker["worktree_name"] == "feature-x"));
    for worker in team["workers"].as_array().unwrap() {
        let surface_id = worker["surface_id"].as_str().unwrap();
        assert_ne!(surface_id, worktree_surface_id);
        let surface = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == surface_id)
            .unwrap();
        assert_eq!(surface.cwd, worktree_dir.path());
    }
    for message in team["messages"].as_array().unwrap() {
        let body = message["body"].as_str().unwrap();
        assert!(body.contains("Worktree: feature-x"));
        assert!(body.contains(worktree_dir.path().to_str().unwrap()));
    }
    for task in team["tasks"].as_array().unwrap() {
        let detail = task["detail"].as_str().unwrap();
        assert!(detail.contains("Worktree: feature-x"));
        assert!(detail.contains(worktree_dir.path().to_str().unwrap()));
    }
}

#[tokio::test]
#[serial_test::serial]
async fn task_strategy_apply_submit_rejects_worktree_worker_when_effective_cwd_changes() {
    let (mut state, _backend) = test_state();
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _claude = write_fake_program(bin_dir.path(), "claude");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let worktree_dir = tempfile::tempdir().unwrap();
    let stale_dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    {
        let mut model = state.model.lock().unwrap();
        model.create_worktree_workspace("feature", worktree_dir.path(), "feature", "feature-x");
    };

    dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router",
            "approved": ["start_run", "create_worktree", "launch_parallel_workers"],
            "submit": true,
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap();
    {
        let mut model = state.model.lock().unwrap();
        let focused = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some("feature-x"))
            .unwrap()
            .focused_surface_id;
        let target_surface = model.add_tab(&focused).unwrap();
        assert!(model.set_surface_cwd(&target_surface.id, stale_dir.path().to_path_buf()));
    }

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
            "worktree_name": "feature-x",
            "goal": "Implement the router",
            "approved": ["start_run", "create_worktree", "launch_parallel_workers"],
            "submit": true,
            "plan": worktree_team_plan_json()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("cwd expected"));
}

fn staged_team_plan_json() -> Value {
    json!({
        "task_class": "feature_implementation",
        "strategy": "implementer_plus_reviewer",
        "layers": {
            "workflow": true,
            "team": true,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "implementer",
                "harness_id": "codex",
                "reason": "primary harness"
            },
            {
                "role": "reviewer",
                "harness_id": "claude",
                "reason": "review harness"
            }
        ],
        "approvals": ["start_run", "launch_parallel_workers"],
        "reasons": ["classified task as FeatureImplementation"],
        "safety_notes": ["visible setup only"]
    })
}

fn worktree_team_plan_json() -> Value {
    let mut plan = staged_team_plan_json();
    plan["layers"]["worktree"] = json!(true);
    plan["approvals"] = json!(["start_run", "create_worktree", "launch_parallel_workers"]);
    plan
}

fn solo_verify_plan_json() -> Value {
    json!({
        "task_class": "focused_bugfix",
        "strategy": "solo_with_verify_loop",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "implementer",
                "harness_id": "codex",
                "reason": "primary harness"
            }
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as FocusedBugfix"],
        "safety_notes": ["visible setup only"]
    })
}

fn parallel_research_plan_json() -> Value {
    json!({
        "task_class": "parallel_research",
        "strategy": "parallel_research",
        "layers": {
            "workflow": true,
            "team": true,
            "loop_metadata": false,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "researcher",
                "harness_id": "codex",
                "reason": "primary research harness"
            },
            {
                "role": "researcher",
                "harness_id": "codex",
                "reason": "second research lane"
            }
        ],
        "approvals": ["start_run", "launch_parallel_workers"],
        "reasons": ["classified task as ParallelResearch"],
        "safety_notes": ["visible setup only"]
    })
}

fn parallel_research_two_researchers_plan_json() -> Value {
    json!({
        "task_class": "parallel_research",
        "strategy": "parallel_research",
        "layers": {
            "workflow": true,
            "team": true,
            "loop_metadata": false,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {
                "role": "researcher",
                "harness_id": "claude",
                "reason": "primary research harness"
            },
            {
                "role": "researcher",
                "harness_id": "claude",
                "reason": "second research lane"
            }
        ],
        "approvals": ["start_run", "launch_parallel_workers"],
        "reasons": ["classified task as ParallelResearch"],
        "safety_notes": ["visible setup only"]
    })
}
