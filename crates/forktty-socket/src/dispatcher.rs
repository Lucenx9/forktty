//! JSON-RPC method dispatcher.

use crate::{
    agent_runtime, browser_import, browser_runtime, context_runtime, hook_session,
    metadata_runtime, methods, project_action_runtime, remote, status_runtime, surface_runtime,
    system_runtime, topology_runtime, workspace_runtime, worktree_runtime, DispatchError,
    SocketAppState,
};
use serde_json::Value;

pub async fn dispatch(
    state: &SocketAppState,
    method: &str,
    params: Value,
) -> Result<Value, DispatchError> {
    #[cfg(not(feature = "browser"))]
    if method.starts_with("browser.") {
        return Err(DispatchError::MethodNotFound(method.to_string()));
    }

    let mut params = params;
    let _hook_session_end = hook_session::prepare_hook_session_targets(state, method, &mut params)?;

    match method {
        "system.ping" => Ok(system_runtime::ping()),
        "system.capabilities" => Ok(system_runtime::capabilities()),
        "system.identify" => system_runtime::identify(state, &params),
        "context.snapshot" => context_runtime::snapshot(state, &params).await,
        "agent.health" => agent_runtime::health(state, &params),
        "agent.hibernate" => agent_runtime::hibernate(state, &params),
        "agent.list" => agent_runtime::list(state, &params),
        "agent.reclaim.plan" => agent_runtime::reclaim_plan(state, &params),
        "agent.reclaim" => agent_runtime::reclaim(state, &params),
        "agent.resume" => agent_runtime::resume(state, &params).await,
        "status.summary" => status_runtime::summary(state, &params),
        "remote.list" => remote::list(state, &params),
        "remote.status" => remote::status(state, &params),
        "system.top" => topology_runtime::system_top(state, &params),
        "workspace.list" => topology_runtime::workspace_list(state),
        "workspace.create" => workspace_runtime::create(state, &params),
        "workspace.create_ssh" => workspace_runtime::create_ssh(state, &params),
        "workspace.select" => workspace_runtime::select(state, &params).await,
        "workspace.close" => workspace_runtime::close(state, &params).await,
        "worktree.list" => worktree_runtime::list(state, &params).await,
        "worktree.status" => worktree_runtime::status(state, &params).await,
        "worktree.create" => worktree_runtime::create(state, &params).await,
        "worktree.attach" => worktree_runtime::attach(state, &params).await,
        "worktree.remove" => worktree_runtime::remove(state, &params).await,
        "worktree.merge" => worktree_runtime::merge(state, &params).await,
        "project.action.list" => project_action_runtime::list(state, &params).await,
        "project.action.run" => project_action_runtime::run(state, &params).await,
        "surface.list" => topology_runtime::surface_list(state, &params),
        "surface.read_text" => topology_runtime::surface_read_text(state, &params),
        "surface.capture_tail" => topology_runtime::surface_capture_tail(state, &params),
        "surface.send_text" => topology_runtime::surface_send_text(state, &params),
        "topology.tree" => topology_runtime::tree(state, &params),
        "surface.split" => surface_runtime::split(state, &params).await,
        "browser.open" => browser_runtime::open(state, &params),
        "browser.navigate" => browser_runtime::navigate(state, &params),
        "browser.snapshot" => browser_runtime::snapshot(state, &params).await,
        "browser.click" => browser_runtime::click(state, &params).await,
        "browser.fill" => browser_runtime::fill(state, &params).await,
        "browser.back" => browser_runtime::back(state, &params).await,
        "browser.forward" => browser_runtime::forward(state, &params).await,
        "browser.reload" => browser_runtime::reload(state, &params).await,
        "browser.profile.list" => browser_runtime::profile_list(state, &params),
        "browser.profile.create" => browser_runtime::profile_create(state, &params),
        "browser.profile.delete" => browser_runtime::profile_delete(state, &params),
        "browser.history.list" => browser_runtime::history_list(state, &params),
        "browser.history.search" => browser_runtime::history_search(state, &params),
        "browser.history.clear" => browser_runtime::history_clear(state, &params),
        "browser.bookmark.add" => browser_runtime::bookmark_add(state, &params),
        "browser.bookmark.list" => browser_runtime::bookmark_list(state, &params),
        "browser.bookmark.remove" => browser_runtime::bookmark_remove(state, &params),
        "browser.import.discover" => Ok(browser_import::browser_import_discover_json()),
        "browser.import.preview" => browser_import::browser_import_preview(&params).await,
        "browser.import.run" => browser_import::browser_import_run(state, &params).await,
        "pane.new_tab" => surface_runtime::new_tab(state, &params).await,
        "pane.select_tab" => surface_runtime::select_tab(state, &params),
        "surface.focus" => surface_runtime::focus(state, &params).await,
        "surface.close" => surface_runtime::close(state, &params).await,
        "notification.create" => metadata_runtime::notification_create(state, &params),
        "notification.list" => metadata_runtime::notification_list(state),
        "notification.clear" => metadata_runtime::notification_clear(state),
        "metadata.set_status" => metadata_runtime::set_status(state, &params),
        "metadata.list_status" => metadata_runtime::list_status(state, &params),
        "metadata.clear_status" => metadata_runtime::clear_status(state, &params),
        "metadata.set_progress" => metadata_runtime::set_progress(state, &params),
        "metadata.list_progress" => metadata_runtime::list_progress(state, &params),
        "metadata.clear_progress" => metadata_runtime::clear_progress(state, &params),
        "metadata.log" => metadata_runtime::log(state, &params),
        "metadata.list_logs" => metadata_runtime::list_logs(state, &params),
        "metadata.clear_logs" => metadata_runtime::clear_logs(state, &params),
        _ => Err(DispatchError::MethodNotFound(method.to_string())),
    }
}

pub(crate) fn method_allowed_from_socket(method: &str) -> bool {
    methods::allowed_from_socket(method)
}
