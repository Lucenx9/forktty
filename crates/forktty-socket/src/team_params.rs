use forktty_core::{
    protocol_limits, validate_worktree_name, TeamEventQuery, TeamHeartbeat, TeamInboxQuery,
    TeamMessageAck, TeamMessageSend, TeamQuery, TeamTaskUpsert, TeamUpsert, TeamWorkerUpsert,
    WorkspaceSelector,
};
use serde_json::Value;
use std::path::PathBuf;

use crate::{
    format_param_names, optional_bool_param, optional_limit_param, optional_non_blank_string_param,
    optional_string_array_param, optional_string_param, optional_u64_param, required_string_param,
    required_trimmed_string, DispatchError, SocketAppState, DEFAULT_TEAM_WORKER_STALE_MS,
};

fn optional_team_workspace_id(
    state: &SocketAppState,
    params: &Value,
) -> Result<Option<String>, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    match team_workspace_selector_from_params(params)? {
        Some(selector) => model
            .workspace_id_for(selector)
            .map(Some)
            .ok_or(DispatchError::NotFound("workspace".to_string())),
        None => Ok(None),
    }
}

fn team_target_ids(
    state: &SocketAppState,
    params: &Value,
    surface_key: &'static str,
) -> Result<(Option<String>, Option<String>), DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let mut workspace_id = match team_workspace_selector_from_params(params)? {
        Some(selector) => Some(
            model
                .workspace_id_for(selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
        ),
        None => None,
    };
    let surface_id = optional_non_blank_string_param(params, surface_key)?.map(str::to_string);
    if let Some(surface_id) = surface_id.as_deref() {
        let surface = model
            .surface(surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        if let Some(workspace_id) = workspace_id.as_deref() {
            if surface.workspace_id != workspace_id {
                return Err(DispatchError::InvalidParam(format!(
                    "{surface_key} does not belong to workspace {workspace_id}"
                )));
            }
        } else {
            workspace_id = Some(surface.workspace_id.clone());
        }
    }
    Ok((workspace_id, surface_id))
}

#[derive(Clone, Copy)]
enum TeamWorkspaceSelectorKind {
    Id,
    Name,
    WorktreeName,
}

struct TeamWorkspaceSelectorParam<'a> {
    key: &'static str,
    kind: TeamWorkspaceSelectorKind,
    value: &'a str,
}

fn team_workspace_selector_from_params(
    params: &Value,
) -> Result<Option<WorkspaceSelector<'_>>, DispatchError> {
    let mut selectors = Vec::new();
    for (key, kind) in [
        ("workspace_id", TeamWorkspaceSelectorKind::Id),
        ("workspaceId", TeamWorkspaceSelectorKind::Id),
        ("workspace_name", TeamWorkspaceSelectorKind::Name),
        ("workspaceName", TeamWorkspaceSelectorKind::Name),
        ("worktreeName", TeamWorkspaceSelectorKind::WorktreeName),
        ("worktree_name", TeamWorkspaceSelectorKind::WorktreeName),
    ] {
        if let Some(value) = optional_non_blank_string_param(params, key)? {
            selectors.push(TeamWorkspaceSelectorParam { key, kind, value });
        }
    }
    if selectors.is_empty() {
        return Ok(None);
    }
    if selectors.len() > 1 {
        return Err(format!(
            "Ambiguous workspace selector: cannot combine {}",
            format_param_names(selectors.iter().map(|selector| selector.key))
        )
        .into());
    }
    let selector = &selectors[0];
    match selector.kind {
        TeamWorkspaceSelectorKind::Id => Ok(Some(WorkspaceSelector::Id(selector.value))),
        TeamWorkspaceSelectorKind::Name => Ok(Some(WorkspaceSelector::Name(selector.value))),
        TeamWorkspaceSelectorKind::WorktreeName => {
            Ok(Some(WorkspaceSelector::WorktreeName(selector.value)))
        }
    }
}

fn validate_optional_surface_id(
    state: &SocketAppState,
    params: &Value,
    key: &'static str,
) -> Result<(), DispatchError> {
    let Some(surface_id) = optional_non_blank_string_param(params, key)? else {
        return Ok(());
    };
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    if model.surface(surface_id).is_some() {
        Ok(())
    } else {
        Err(DispatchError::NotFound("surface".to_string()))
    }
}

fn validate_optional_worktree_name(params: &Value) -> Result<(), DispatchError> {
    if let Some(worktree_name) = optional_non_blank_string_param(params, "worktree_name")? {
        validate_worktree_name(worktree_name)?;
    }
    Ok(())
}

fn optional_existing_absolute_cwd(
    params: &Value,
    param: &'static str,
) -> Result<Option<PathBuf>, DispatchError> {
    let Some(cwd) = optional_non_blank_string_param(params, param)? else {
        return Ok(None);
    };
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {param}: expected an absolute path"
        )));
    }
    if !path.is_dir() {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {param}: expected an existing directory"
        )));
    }
    Ok(Some(path))
}

fn team_worker_action_text(
    text: Option<&str>,
    default_text: &'static str,
) -> Result<String, DispatchError> {
    let text = text.unwrap_or(default_text);
    if text.is_empty() {
        return Err(DispatchError::InvalidParam(
            "team worker action text must not be empty".to_string(),
        ));
    }
    if text.len() > protocol_limits::SOCKET_SEND_TEXT_MAX_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field: "text",
            limit: protocol_limits::SOCKET_SEND_TEXT_MAX_BYTES,
            actual: text.len(),
        });
    }
    Ok(text.to_string())
}

pub(crate) struct TeamListRequest {
    pub(crate) query: TeamQuery,
}

impl TeamListRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            query: TeamQuery {
                workspace_id: optional_team_workspace_id(state, params)?,
                status: optional_non_blank_string_param(params, "status")?.map(str::to_string),
                query: optional_non_blank_string_param(params, "query")?.map(str::to_string),
                limit: optional_limit_param(params, "limit")?,
            },
        })
    }
}

pub(crate) struct TeamGetRequest {
    pub(crate) team_id: String,
}

impl TeamGetRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
        })
    }
}

pub(crate) struct TeamUpsertRequest {
    pub(crate) input: TeamUpsert,
}

impl TeamUpsertRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let (workspace_id, leader_surface_id) =
            team_target_ids(state, params, "leader_surface_id")?;
        Ok(Self {
            input: TeamUpsert {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                workspace_id,
                leader_surface_id,
                name: optional_non_blank_string_param(params, "name")?.map(str::to_string),
                status: optional_non_blank_string_param(params, "status")?.map(str::to_string),
                goal: optional_string_param(params, "goal")?.map(str::to_string),
            },
        })
    }
}

pub(crate) struct TeamFinishRequest {
    pub(crate) team_id: String,
    pub(crate) dry_run: bool,
    pub(crate) close_workers: bool,
    pub(crate) force: bool,
}

impl TeamFinishRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
            dry_run: optional_bool_param(params, "dry_run")?.unwrap_or(false),
            close_workers: optional_bool_param(params, "close_workers")?.unwrap_or(false),
            force: optional_bool_param(params, "force")?.unwrap_or(false),
        })
    }
}

pub(crate) struct TeamWorkerUpsertRequest {
    pub(crate) input: TeamWorkerUpsert,
}

impl TeamWorkerUpsertRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        validate_optional_surface_id(state, params, "surface_id")?;
        validate_optional_worktree_name(params)?;
        Ok(Self {
            input: TeamWorkerUpsert {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                worker_id: required_trimmed_string(params, "worker_id")?.to_string(),
                role: optional_non_blank_string_param(params, "role")?.map(str::to_string),
                agent: optional_non_blank_string_param(params, "agent")?.map(str::to_string),
                surface_id: optional_non_blank_string_param(params, "surface_id")?
                    .map(str::to_string),
                worktree_name: optional_non_blank_string_param(params, "worktree_name")?
                    .map(str::to_string),
                status: optional_non_blank_string_param(params, "status")?.map(str::to_string),
                assigned_task_id: optional_non_blank_string_param(params, "assigned_task_id")?
                    .map(str::to_string),
            },
        })
    }
}

pub(crate) struct TeamWorkerHeartbeatRequest {
    pub(crate) input: TeamHeartbeat,
}

impl TeamWorkerHeartbeatRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            input: TeamHeartbeat {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                worker_id: required_trimmed_string(params, "worker_id")?.to_string(),
                status: optional_non_blank_string_param(params, "status")?.map(str::to_string),
                assigned_task_id: optional_non_blank_string_param(params, "assigned_task_id")?
                    .map(str::to_string),
            },
        })
    }
}

pub(crate) struct TeamWorkerLaunchRequest {
    pub(crate) team_id: String,
    pub(crate) worker_id: String,
    pub(crate) requested_agent: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) assigned_task_id: Option<String>,
    pub(crate) worktree_name: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) extra_args: Vec<String>,
}

impl TeamWorkerLaunchRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        validate_optional_worktree_name(params)?;
        let worktree_name =
            optional_non_blank_string_param(params, "worktree_name")?.map(str::to_string);
        let cwd = optional_existing_absolute_cwd(params, "cwd")?;
        if worktree_name.is_some() && cwd.is_some() {
            return Err(DispatchError::InvalidParam(
                "cannot combine worktree_name and cwd".to_string(),
            ));
        }
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
            worker_id: required_trimmed_string(params, "worker_id")?.to_string(),
            requested_agent: optional_non_blank_string_param(params, "agent")?.map(str::to_string),
            role: optional_non_blank_string_param(params, "role")?.map(str::to_string),
            assigned_task_id: optional_non_blank_string_param(params, "assigned_task_id")?
                .map(str::to_string),
            worktree_name,
            cwd,
            extra_args: optional_string_array_param(params, "args")?.unwrap_or_default(),
        })
    }
}

pub(crate) struct TeamWorkerHealthRequest {
    pub(crate) team_id: String,
    pub(crate) stale_after_ms: u64,
}

impl TeamWorkerHealthRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
            stale_after_ms: optional_u64_param(params, "stale_after_ms")?
                .unwrap_or(DEFAULT_TEAM_WORKER_STALE_MS),
        })
    }
}

pub(crate) struct TeamWorkerNudgeRequest {
    pub(crate) team_id: String,
    pub(crate) worker_id: String,
    pub(crate) text: String,
}

impl TeamWorkerNudgeRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
            worker_id: required_trimmed_string(params, "worker_id")?.to_string(),
            text: team_worker_action_text(
                optional_string_param(params, "text")?,
                "Still with us? Please send a ForkTTY team.worker.heartbeat update.\r",
            )?,
        })
    }
}

pub(crate) struct TeamWorkerShutdownRequest {
    pub(crate) team_id: String,
    pub(crate) worker_id: String,
    pub(crate) submit: bool,
    pub(crate) close_surface: bool,
    pub(crate) text: String,
}

impl TeamWorkerShutdownRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
            worker_id: required_trimmed_string(params, "worker_id")?.to_string(),
            submit: optional_bool_param(params, "submit")?.unwrap_or(true),
            close_surface: optional_bool_param(params, "close_surface")?.unwrap_or(false),
            text: team_worker_action_text(
                optional_string_param(params, "text")?,
                "Please stop after your current safe checkpoint and report shutdown.",
            )?,
        })
    }
}

pub(crate) struct TeamTaskUpsertRequest {
    pub(crate) input: TeamTaskUpsert,
}

impl TeamTaskUpsertRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            input: TeamTaskUpsert {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                task_id: required_trimmed_string(params, "task_id")?.to_string(),
                title: optional_non_blank_string_param(params, "title")?.map(str::to_string),
                status: optional_non_blank_string_param(params, "status")?.map(str::to_string),
                detail: optional_string_param(params, "detail")?.map(str::to_string),
                depends_on: optional_string_array_param(params, "depends_on")?,
                assigned_worker_id: optional_non_blank_string_param(params, "assigned_worker_id")?
                    .map(str::to_string),
            },
        })
    }
}

pub(crate) struct TeamMessageSendRequest {
    pub(crate) input: TeamMessageSend,
}

impl TeamMessageSendRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            input: TeamMessageSend {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                message_id: optional_non_blank_string_param(params, "message_id")?
                    .map(str::to_string),
                from: required_trimmed_string(params, "from")?.to_string(),
                to_worker_id: optional_non_blank_string_param(params, "to_worker_id")?
                    .map(str::to_string),
                task_id: optional_non_blank_string_param(params, "task_id")?.map(str::to_string),
                body: required_string_param(params, "body")?.to_string(),
            },
        })
    }
}

pub(crate) struct TeamMessageDispatchRequest {
    pub(crate) team_id: String,
    pub(crate) message_id: String,
    pub(crate) worker_id: Option<String>,
    pub(crate) submit: bool,
}

impl TeamMessageDispatchRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
            message_id: required_trimmed_string(params, "message_id")?.to_string(),
            worker_id: optional_non_blank_string_param(params, "worker_id")?.map(str::to_string),
            submit: optional_bool_param(params, "submit")?.unwrap_or(false),
        })
    }
}

pub(crate) struct TeamMessageAckRequest {
    pub(crate) input: TeamMessageAck,
}

impl TeamMessageAckRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            input: TeamMessageAck {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                message_id: required_trimmed_string(params, "message_id")?.to_string(),
                worker_id: optional_non_blank_string_param(params, "worker_id")?
                    .map(str::to_string),
            },
        })
    }
}

pub(crate) struct TeamInboxRequest {
    pub(crate) query: TeamInboxQuery,
}

impl TeamInboxRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            query: TeamInboxQuery {
                team_id: required_trimmed_string(params, "team_id")?.to_string(),
                worker_id: optional_non_blank_string_param(params, "worker_id")?
                    .map(str::to_string),
                include_delivered: optional_bool_param(params, "include_delivered")?
                    .unwrap_or(false),
                limit: optional_limit_param(params, "limit")?,
            },
        })
    }
}

pub(crate) struct TeamSummaryRequest {
    pub(crate) team_id: String,
}

impl TeamSummaryRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            team_id: required_trimmed_string(params, "team_id")?.to_string(),
        })
    }
}

pub(crate) struct TeamEventsRequest {
    pub(crate) query: TeamEventQuery,
}

impl TeamEventsRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            query: TeamEventQuery {
                team_id: optional_non_blank_string_param(params, "team_id")?.map(str::to_string),
                since_seq: optional_u64_param(params, "since_seq")?,
                limit: optional_limit_param(params, "limit")?,
            },
        })
    }
}
