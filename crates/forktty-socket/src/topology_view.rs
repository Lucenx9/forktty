use forktty_core::{SurfaceKind, WorkspaceModel};
use forktty_terminal::TerminalSurfaceState;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::surface_effective_project_cwd;

pub(crate) fn tree(model: &WorkspaceModel, workspace_id: Option<&str>) -> Value {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .filter(|workspace| workspace_id.is_none_or(|id| workspace.id == id))
        .map(|workspace| {
            let surfaces = model.list_surfaces(Some(&workspace.id));
            json!({
                "id": workspace.id,
                "name": workspace.name,
                "active": workspace.active,
                "working_dir": workspace.working_dir,
                "git_branch": workspace.git_branch,
                "worktree_name": workspace.worktree_name,
                "focused_surface_id": workspace.focused_surface_id,
                "needs_attention": workspace.needs_attention,
                "pane_tree": workspace.pane_tree,
                "surface_count": surfaces.len(),
                "surfaces": surfaces,
            })
        })
        .collect::<Vec<_>>();
    json!({ "workspaces": workspaces })
}

pub(crate) fn surface_list_rows(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    terminal_surfaces: Vec<TerminalSurfaceState>,
) -> Value {
    let terminal_by_id = terminal_surfaces
        .into_iter()
        .map(|surface| (surface.surface_id.clone(), surface))
        .collect::<HashMap<_, _>>();
    let rows = model
        .list_surfaces(workspace_id)
        .into_iter()
        .map(|surface| {
            let runtime = terminal_by_id.get(&surface.id);
            let mut row = serde_json::to_value(&surface).unwrap_or_else(|_| json!({}));
            if let Some(row) = row.as_object_mut() {
                row.insert(
                    "shell".to_string(),
                    runtime
                        .map(|surface| json!(surface.shell.clone()))
                        .unwrap_or(Value::Null),
                );
                row.insert(
                    "cols".to_string(),
                    runtime
                        .map(|surface| json!(surface.cols))
                        .unwrap_or(Value::Null),
                );
                row.insert(
                    "rows".to_string(),
                    runtime
                        .map(|surface| json!(surface.rows))
                        .unwrap_or(Value::Null),
                );
                row.insert(
                    "pid".to_string(),
                    runtime
                        .and_then(|surface| surface.pid)
                        .map_or(Value::Null, Value::from),
                );
                row.insert(
                    "effective_project_cwd".to_string(),
                    json!(surface_effective_project_cwd(&surface)),
                );
            }
            row
        })
        .collect::<Vec<_>>();
    Value::Array(rows)
}

pub(crate) fn system_top(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    terminal_surfaces: Vec<TerminalSurfaceState>,
) -> Value {
    let terminal_by_id = terminal_surfaces
        .into_iter()
        .map(|surface| (surface.surface_id.clone(), surface))
        .collect::<HashMap<_, _>>();
    let mut total_surfaces = 0usize;
    let mut unread_surfaces = 0usize;
    let mut agents = 0usize;

    let workspaces = model
        .list_workspaces()
        .into_iter()
        .filter(|workspace| workspace_id.is_none_or(|id| workspace.id == id))
        .map(|workspace| {
            let surfaces = model.list_surfaces(Some(&workspace.id));
            total_surfaces += surfaces.len();
            unread_surfaces += surfaces.iter().filter(|surface| surface.unread).count();
            agents += surfaces
                .iter()
                .filter(|surface| surface.agent_session.is_some())
                .count();
            let surface_rows = surfaces
                .iter()
                .map(|surface| {
                    let runtime = terminal_by_id.get(&surface.id);
                    let agent = surface
                        .agent_session
                        .as_ref()
                        .map(|agent| {
                            json!({
                                "agent": agent.agent,
                                "session_id": agent.session_id,
                                "resume_cwd": agent.resume_cwd,
                                "permission_mode": agent.permission_mode,
                                "lifecycle": agent.lifecycle,
                                "last_activity_ms": agent.last_activity_ms,
                            })
                        })
                        .unwrap_or(Value::Null);
                    json!({
                        "id": surface.id,
                        "workspace_id": surface.workspace_id,
                        "title": surface.title,
                        "kind": surface_kind_label(&surface.kind),
                        "focused": workspace.focused_surface_id == surface.id,
                        "unread": surface.unread,
                        "needs_attention": surface.needs_attention,
                        "cwd": surface.cwd,
                        "shell": runtime.map(|surface| surface.shell.clone()),
                        "cols": runtime.map(|surface| surface.cols),
                        "rows": runtime.map(|surface| surface.rows),
                        "pid": runtime.and_then(|surface| surface.pid),
                        "agent": agent,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "id": workspace.id,
                "name": workspace.name,
                "active": workspace.active,
                "working_dir": workspace.working_dir,
                "git_branch": workspace.git_branch,
                "worktree_name": workspace.worktree_name,
                "focused_surface_id": workspace.focused_surface_id,
                "needs_attention": workspace.needs_attention,
                "listening_ports": workspace.listening_ports,
                "surface_count": surface_rows.len(),
                "unread_surface_count": surface_rows
                    .iter()
                    .filter(|surface| surface["unread"].as_bool().unwrap_or(false))
                    .count(),
                "surfaces": surface_rows,
                "status": model.list_status(&workspace.id),
                "progress": model.list_progress(&workspace.id),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "totals": {
            "workspaces": workspaces.len(),
            "surfaces": total_surfaces,
            "unread_surfaces": unread_surfaces,
            "agents": agents,
        },
        "workspaces": workspaces,
    })
}

fn surface_kind_label(kind: &SurfaceKind) -> &'static str {
    match kind {
        SurfaceKind::Terminal => "terminal",
        SurfaceKind::Browser { .. } => "browser",
        SurfaceKind::Ssh { .. } => "ssh",
    }
}
