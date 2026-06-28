//! Task strategy socket method regression tests.

use super::*;
use forktty_core::HarnessHealth;

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
        .any(|factor| factor["name"] == "harness_readiness"));
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
    assert_eq!(result["strategy"], "implementer_plus_reviewer");
    assert!(result["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|assignment| { assignment["role"] == "reviewer" }));
    assert!(result["candidate_scores"][0]["factors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|factor| factor["name"] == "router_profile_fit"
            && factor["points"].as_i64().unwrap_or_default() > 0));
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

    let claude = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "claude")
        .unwrap();
    assert!(!claude.installed);
    assert_eq!(claude.health, HarnessHealth::Missing);

    let opencode = registry
        .harnesses
        .iter()
        .find(|harness| harness.id == "opencode")
        .unwrap();
    assert!(opencode.installed);
    assert_eq!(opencode.health, HarnessHealth::Disabled);
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
async fn task_strategy_plan_rejects_blank_goal() {
    let (state, _backend) = test_state();
    let err = dispatch(&state, "task.strategy.plan", json!({ "goal": " " }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("goal"));
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
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let mut plan = worktree_team_plan_json();
    plan["approvals"] = json!(["start_run"]);

    let err = dispatch(
        &state,
        "task.strategy.apply",
        json!({
            "run_id": "router-run-1",
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
    assert_eq!(backend.sent_text(implementer_surface).unwrap().len(), 1);
    assert_eq!(backend.sent_text(reviewer_surface).unwrap().len(), 2);
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
                "role": "synthesizer",
                "harness_id": "codex",
                "reason": "synthesizer role"
            }
        ],
        "approvals": ["start_run", "launch_parallel_workers"],
        "reasons": ["classified task as ParallelResearch"],
        "safety_notes": ["visible setup only"]
    })
}
