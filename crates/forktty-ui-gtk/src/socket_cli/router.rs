//! Top-level socket CLI command router.

use super::agent::{
    handle_agent_health, handle_agent_reclaim_plan, handle_agents, handle_hibernate_agent,
    handle_reclaim_agents, handle_resume_agent,
};
#[cfg(feature = "browser")]
use super::browser::handle_browser;
use super::hooks::handle_hooks;
use super::remote::{handle_remote_status, handle_remotes};
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
        "log" => handle_log(context, args),
        "logs" | "list-logs" => handle_logs(context, args),
        "clear-logs" => handle_clear_logs(context, args),
        "notifications" => handle_notifications(context, args),
        "clear-notifications" | "notifications-clear" | "notification:clear" => {
            handle_clear_notifications(context, args)
        }
        "hooks" => handle_hooks(context, args),
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
