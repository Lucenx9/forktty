//! Top-level socket CLI command router.

use super::agent::{
    handle_agent_health, handle_agent_reclaim_plan, handle_agents, handle_hibernate_agent,
    handle_reclaim_agents, handle_resume_agent,
};
#[cfg(feature = "browser")]
use super::browser::handle_browser;
use super::cleanup::{handle_cleanup, handle_orchestration_cleanup};
use super::feed::handle_feed;
use super::hooks::handle_hooks;
use super::mcp::handle_mcp;
use super::remote::{handle_remote_status, handle_remotes};
use super::skills::handle_skills;
use super::status::{
    handle_clear_logs, handle_clear_notifications, handle_clear_progress, handle_clear_status,
    handle_context_snapshot, handle_list_progress, handle_list_status, handle_log, handle_logs,
    handle_notifications, handle_set_progress, handle_set_status, handle_status, handle_statusline,
};
use super::surface::{
    handle_capture_tail, handle_close_surface, handle_focus_surface, handle_new_tab,
    handle_read_screen, handle_select_tab, handle_send_text, handle_split_surface, handle_surfaces,
    handle_top, handle_tree,
};
use super::system::{
    handle_capabilities, handle_completions, handle_events, handle_examples, handle_help,
    handle_identify, handle_notify, handle_ping, handle_socket_doctor, handle_wait,
};
use super::task::{handle_task_apply, handle_task_plan};
use super::team::{
    handle_team, handle_team_events, handle_team_get, handle_team_inbox, handle_team_list,
    handle_team_message_ack, handle_team_message_dispatch, handle_team_message_send,
    handle_team_summary, handle_team_task_upsert, handle_team_upsert, handle_team_worker_health,
    handle_team_worker_heartbeat, handle_team_worker_launch, handle_team_worker_nudge,
    handle_team_worker_shutdown, handle_team_worker_upsert,
};
use super::workflow::{
    handle_workflow_evidence_add, handle_workflow_get, handle_workflow_loop_gate,
    handle_workflow_loop_iteration_done, handle_workflow_loop_iteration_start,
    handle_workflow_loop_publish, handle_workflow_loop_set, handle_workflow_loop_step_done,
    handle_workflow_plan_set, handle_workflow_replay, handle_workflow_upsert, handle_workflows,
};
use super::workspace::{
    handle_close_workspace, handle_create_workspace, handle_focus, handle_list, handle_ssh,
};
use super::worktree::{
    handle_project_action_list, handle_project_action_run, handle_worktree_doctor,
    handle_worktree_list, handle_worktree_merge, handle_worktree_open, handle_worktree_remove,
    handle_worktree_status,
};
use super::{CliContext, CliError, CliResult};

pub(super) fn dispatch_command(
    context: &CliContext,
    command: &str,
    args: Vec<String>,
) -> CliResult<()> {
    match command {
        "list" => handle_list(context, args),
        "create-workspace" => handle_create_workspace(context, args),
        "focus" => handle_focus(context, args),
        "close-workspace" => handle_close_workspace(context, args),
        "notify" => handle_notify(context, args),
        "surfaces" | "surface-list" | "surface:list" => handle_surfaces(context, args),
        "agents" | "agent-list" | "agent:list" => handle_agents(context, args),
        "agent-health" | "agent:health" => handle_agent_health(context, args),
        "agent-reclaim-plan" | "agent:reclaim-plan" | "agent.reclaim.plan" => {
            handle_agent_reclaim_plan(context, args)
        }
        "hibernate-agent" | "agent-hibernate" | "agent:hibernate" | "agent.hibernate" => {
            handle_hibernate_agent(context, args)
        }
        "reclaim-agents" | "agent-reclaim" | "agent:reclaim" | "agent.reclaim" => {
            handle_reclaim_agents(context, args)
        }
        "resume-agent" | "agent-resume" | "agent:resume" => handle_resume_agent(context, args),
        "team" => handle_team(context, args),
        "teams" | "team-list" | "team:list" | "team.list" => handle_team_list(context, args),
        "team-get" | "team:get" | "team.get" => handle_team_get(context, args),
        "team-upsert" | "team:upsert" | "team.upsert" => handle_team_upsert(context, args),
        "team-worker-upsert" | "team:worker-upsert" | "team.worker.upsert" => {
            handle_team_worker_upsert(context, args)
        }
        "team-worker-heartbeat" | "team:worker-heartbeat" | "team.worker.heartbeat" => {
            handle_team_worker_heartbeat(context, args)
        }
        "team-worker-launch" | "team:worker-launch" | "team.worker.launch" => {
            handle_team_worker_launch(context, args)
        }
        "team-worker-health" | "team:worker-health" | "team.worker.health" => {
            handle_team_worker_health(context, args)
        }
        "team-worker-nudge" | "team:worker-nudge" | "team.worker.nudge" => {
            handle_team_worker_nudge(context, args)
        }
        "team-worker-shutdown" | "team:worker-shutdown" | "team.worker.shutdown" => {
            handle_team_worker_shutdown(context, args)
        }
        "team-task-upsert" | "team:task-upsert" | "team.task.upsert" => {
            handle_team_task_upsert(context, args)
        }
        "team-message-send" | "team:message-send" | "team.message.send" => {
            handle_team_message_send(context, args)
        }
        "team-message-dispatch" | "team:message-dispatch" | "team.message.dispatch" => {
            handle_team_message_dispatch(context, args)
        }
        "team-message-ack" | "team:message-ack" | "team.message.ack" => {
            handle_team_message_ack(context, args)
        }
        "team-inbox" | "team:inbox" | "team.inbox" => handle_team_inbox(context, args),
        "team-summary" | "team:summary" | "team.summary" => handle_team_summary(context, args),
        "team-events" | "team:events" | "team.events" => handle_team_events(context, args),
        "split-surface" | "surface-split" | "surface:split" => handle_split_surface(context, args),
        "focus-surface" | "surface-focus" | "surface:focus" => handle_focus_surface(context, args),
        "close-surface" | "surface-close" | "surface:close" => handle_close_surface(context, args),
        "new-tab" | "pane-new-tab" | "pane:new-tab" => handle_new_tab(context, args),
        "select-tab" | "pane-select-tab" | "pane:select-tab" => handle_select_tab(context, args),
        "send-text" | "send_text" => handle_send_text(context, args),
        "read-screen" | "read_screen" | "surface-read-text" | "surface:read-text" => {
            handle_read_screen(context, args)
        }
        "capture-tail" | "capture_tail" | "surface-capture-tail" | "surface:capture-tail" => {
            handle_capture_tail(context, args)
        }
        "tree" | "topology-tree" | "topology:tree" | "topology.tree" => handle_tree(context, args),
        "top" => handle_top(context, args),
        "remotes" | "remote-list" | "remote:list" | "remote.list" => handle_remotes(context, args),
        "remote-status" | "remote:status" | "remote.status" => handle_remote_status(context, args),
        "worktree-list" | "worktree:list" | "worktree.list" => handle_worktree_list(context, args),
        "worktree-status" | "worktree:status" | "worktree.status" => {
            handle_worktree_status(context, args)
        }
        "worktree-create" | "worktree:create" | "worktree.create" => {
            handle_worktree_open(context, args, "worktree.create", "worktree-create")
        }
        "worktree-attach" | "worktree:attach" | "worktree.attach" => {
            handle_worktree_open(context, args, "worktree.attach", "worktree-attach")
        }
        "worktree-remove" | "worktree:remove" | "worktree.remove" => {
            handle_worktree_remove(context, args)
        }
        "worktree-merge" | "worktree:merge" | "worktree.merge" => {
            handle_worktree_merge(context, args)
        }
        "worktree-doctor" | "worktree:doctor" | "worktree.doctor" => {
            handle_worktree_doctor(context, args)
        }
        "actions" | "project-actions" | "project:action:list" | "project.action.list" => {
            handle_project_action_list(context, args)
        }
        "action-run" | "project-action-run" | "project:action:run" | "project.action.run" => {
            handle_project_action_run(context, args)
        }
        "set-status" => handle_set_status(context, args),
        "list-status" => handle_list_status(context, args),
        "clear-status" => handle_clear_status(context, args),
        "set-progress" => handle_set_progress(context, args),
        "list-progress" => handle_list_progress(context, args),
        "clear-progress" => handle_clear_progress(context, args),
        "status" => handle_status(context, args),
        "statusline" | "status-line" | "status:summary" => handle_statusline(context, args),
        "context-snapshot" | "context_snapshot" | "context:snapshot" | "context.snapshot" => {
            handle_context_snapshot(context, args)
        }
        "feed" | "feed-list" | "feed:list" => handle_feed(context, args),
        "cleanup" => handle_cleanup(context, args),
        "orchestration-cleanup" | "orchestration:cleanup" | "orchestration.cleanup" => {
            handle_orchestration_cleanup(context, args)
        }
        "task-plan" | "task:plan" | "task.strategy.plan" => handle_task_plan(context, args),
        "task-apply" | "task:apply" | "task.strategy.apply" => handle_task_apply(context, args),
        "workflows" | "workflow-list" | "workflow:list" | "workflow.list" => {
            handle_workflows(context, args)
        }
        "workflow-get" | "workflow:get" | "workflow.get" => handle_workflow_get(context, args),
        "workflow-upsert" | "workflow:upsert" | "workflow.upsert" => {
            handle_workflow_upsert(context, args)
        }
        "workflow-loop-set" | "workflow:loop:set" | "workflow.loop.set" | "loop-set" => {
            handle_workflow_loop_set(context, args)
        }
        "workflow-loop-gate" | "workflow:loop:gate" | "workflow.loop.gate" | "loop-gate" => {
            handle_workflow_loop_gate(context, args)
        }
        "workflow-loop-step-done"
        | "workflow:loop:step-done"
        | "workflow.loop.step_done"
        | "loop-step-done" => handle_workflow_loop_step_done(context, args),
        "workflow-loop-iteration-start"
        | "workflow:loop:iteration-start"
        | "workflow.loop.iteration_start"
        | "loop-iteration-start" => handle_workflow_loop_iteration_start(context, args),
        "workflow-loop-iteration-done"
        | "workflow:loop:iteration-done"
        | "workflow.loop.iteration_done"
        | "loop-iteration-done" => handle_workflow_loop_iteration_done(context, args),
        "workflow-loop-publish"
        | "workflow:loop:publish"
        | "workflow.loop.publish"
        | "loop-publish" => handle_workflow_loop_publish(context, args),
        "workflow-plan-set" | "workflow:plan-set" | "workflow.plan.set" => {
            handle_workflow_plan_set(context, args)
        }
        "workflow-evidence-add" | "workflow:evidence-add" | "workflow.evidence.add" => {
            handle_workflow_evidence_add(context, args)
        }
        "workflow-replay" | "workflow:replay" | "workflow.replay" => {
            handle_workflow_replay(context, args)
        }
        "log" => handle_log(context, args),
        "logs" | "list-logs" => handle_logs(context, args),
        "clear-logs" => handle_clear_logs(context, args),
        "notifications" => handle_notifications(context, args),
        "clear-notifications" | "notifications-clear" | "notification:clear" => {
            handle_clear_notifications(context, args)
        }
        "hooks" => handle_hooks(context, args),
        "mcp" => handle_mcp(context, args),
        "skills" | "skill" => handle_skills(context, args),
        "doctor" => handle_socket_doctor(context, args),
        "ping" => handle_ping(context, args),
        "identify" | "system-identify" | "system:identify" | "system.identify" => {
            handle_identify(context, args)
        }
        "capabilities" => handle_capabilities(context, args),
        "wait" => handle_wait(context, args),
        "events" => handle_events(context, args),
        "examples" => handle_examples(context, args),
        "completion" | "completions" => handle_completions(context, args),
        #[cfg(feature = "browser")]
        "browser" => handle_browser(context, args),
        #[cfg(not(feature = "browser"))]
        "browser" => Err(CliError::new(
            "browser commands require building ForkTTY from source with --features browser",
        )),
        "ssh" => handle_ssh(context, args),
        "help" => handle_help(context, args),
        other => Err(CliError::new(format!("Unknown command: {other}"))),
    }
}
