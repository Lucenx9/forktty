use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const TEAM_FORMAT_VERSION: u32 = 1;
const MAX_TEAM_STORE_BYTES: u64 = 1024 * 1024;
const MAX_TEAMS: usize = 128;
const MAX_WORKERS_PER_TEAM: usize = 64;
const MAX_TASKS_PER_TEAM: usize = 512;
const MAX_MESSAGES_PER_TEAM: usize = 1024;
const MAX_EVENTS: usize = 4096;
const MAX_SHORT_TEXT_BYTES: usize = 512;
const MAX_LONG_TEXT_BYTES: usize = 16 * 1024;
const DEFAULT_QUERY_LIMIT: usize = 50;
const MAX_QUERY_LIMIT: usize = 200;
static TEAM_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum TeamError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported team format version: {0}")]
    UnsupportedVersion(u32),
    #[error("Invalid team data: {0}")]
    Invalid(String),
    #[error("Team not found: {0}")]
    TeamNotFound(String),
    #[error("Worker not found: {0}")]
    WorkerNotFound(String),
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Message not found: {0}")]
    MessageNotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamStoreData {
    #[serde(default = "default_team_version")]
    pub version: u32,
    #[serde(default)]
    pub teams: Vec<TeamState>,
    #[serde(default)]
    pub events: Vec<TeamEvent>,
    #[serde(default = "default_next_event_seq")]
    pub next_event_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamState {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_surface_id: Option<String>,
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub workers: Vec<TeamWorker>,
    #[serde(default)]
    pub tasks: Vec<TeamTask>,
    #[serde(default)]
    pub messages: Vec<TeamMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamWorker {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_task_id: Option<String>,
    #[serde(default)]
    pub last_heartbeat_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub launched_at_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub last_nudge_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub shutdown_requested_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamTask {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_worker_id: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMessage {
    pub id: String,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub body: String,
    pub created_at_ms: u64,
    #[serde(default)]
    pub delivered: bool,
    #[serde(default)]
    pub acknowledged_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamEvent {
    pub seq: u64,
    pub team_id: String,
    pub kind: String,
    pub summary: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamQuery {
    pub workspace_id: Option<String>,
    pub status: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamUpsert {
    pub team_id: String,
    pub workspace_id: Option<String>,
    pub leader_surface_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub goal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamWorkerUpsert {
    pub team_id: String,
    pub worker_id: String,
    pub role: Option<String>,
    pub agent: Option<String>,
    pub surface_id: Option<String>,
    pub worktree_name: Option<String>,
    pub status: Option<String>,
    pub assigned_task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamWorkerLaunch {
    pub team_id: String,
    pub worker_id: String,
    pub role: Option<String>,
    pub agent: String,
    pub surface_id: String,
    pub worktree_name: Option<String>,
    pub assigned_task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamWorkerAction {
    pub team_id: String,
    pub worker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamHeartbeat {
    pub team_id: String,
    pub worker_id: String,
    pub status: Option<String>,
    pub assigned_task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamTaskUpsert {
    pub team_id: String,
    pub task_id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub detail: Option<String>,
    pub depends_on: Option<Vec<String>>,
    pub assigned_worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMessageSend {
    pub team_id: String,
    pub message_id: Option<String>,
    pub from: String,
    pub to_worker_id: Option<String>,
    pub task_id: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMessageAck {
    pub team_id: String,
    pub message_id: String,
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamInboxQuery {
    pub team_id: String,
    pub worker_id: Option<String>,
    pub include_delivered: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamEventQuery {
    pub team_id: Option<String>,
    pub since_seq: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamSummary {
    pub team_id: String,
    pub status: String,
    pub workers_total: usize,
    pub workers_active: usize,
    pub tasks_total: usize,
    pub tasks_open: usize,
    pub messages_pending: usize,
    pub last_event_seq: u64,
}

fn default_team_version() -> u32 {
    TEAM_FORMAT_VERSION
}

fn default_next_event_seq() -> u64 {
    1
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

impl Default for TeamStoreData {
    fn default() -> Self {
        Self {
            version: TEAM_FORMAT_VERSION,
            teams: Vec::new(),
            events: Vec::new(),
            next_event_seq: 1,
        }
    }
}

impl TeamStoreData {
    pub fn list(&self, query: &TeamQuery) -> Vec<TeamState> {
        let limit = query
            .limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .min(MAX_QUERY_LIMIT);
        let needle = query.query.as_deref().map(str::to_ascii_lowercase);
        let mut rows = self
            .teams
            .iter()
            .filter(|team| {
                query
                    .workspace_id
                    .as_deref()
                    .is_none_or(|id| team.workspace_id.as_deref() == Some(id))
                    && query
                        .status
                        .as_deref()
                        .is_none_or(|status| team.status == status)
                    && needle
                        .as_deref()
                        .is_none_or(|needle| team.matches_query(needle))
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|team| std::cmp::Reverse(team.updated_at_ms));
        rows.truncate(limit);
        rows
    }

    pub fn get(&self, team_id: &str) -> Option<TeamState> {
        self.teams.iter().find(|team| team.id == team_id).cloned()
    }

    pub fn upsert_team(&mut self, input: TeamUpsert, now_ms: u64) -> Result<TeamState, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let workspace_id = clean_optional_id("workspace_id", input.workspace_id.as_deref())?;
        let leader_surface_id =
            clean_optional_id("leader_surface_id", input.leader_surface_id.as_deref())?;
        let name = clean_optional_short("name", input.name.as_deref())?;
        let status = clean_optional_short("status", input.status.as_deref())?;
        let goal = clean_optional_long("goal", input.goal.as_deref())?;
        let index = match self.teams.iter().position(|team| team.id == team_id) {
            Some(index) => index,
            None => {
                if self.teams.len() >= MAX_TEAMS {
                    return Err(TeamError::Invalid(format!(
                        "team store is full (limit {MAX_TEAMS})"
                    )));
                }
                self.teams.push(TeamState {
                    id: team_id.clone(),
                    workspace_id: workspace_id.clone(),
                    leader_surface_id: leader_surface_id.clone(),
                    name: name.clone().unwrap_or_else(|| team_id.clone()),
                    status: status.clone().unwrap_or_else(|| "active".to_string()),
                    goal: goal.clone(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                    workers: Vec::new(),
                    tasks: Vec::new(),
                    messages: Vec::new(),
                });
                self.teams.len() - 1
            }
        };

        let team = &mut self.teams[index];
        team.workspace_id = workspace_id.or(team.workspace_id.take());
        team.leader_surface_id = leader_surface_id.or(team.leader_surface_id.take());
        if let Some(name) = name {
            team.name = name;
        }
        if let Some(status) = status {
            team.status = status;
        }
        if goal.is_some() {
            team.goal = goal;
        }
        team.updated_at_ms = now_ms;
        let team = team.clone();
        self.push_event(
            &team_id,
            "team.upserted",
            format!("team {}", team.id),
            now_ms,
        );
        Ok(team)
    }

    pub fn upsert_worker(
        &mut self,
        input: TeamWorkerUpsert,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let role = clean_optional_short("role", input.role.as_deref())?;
        let agent = clean_optional_short("agent", input.agent.as_deref())?;
        let surface_id = clean_optional_id("surface_id", input.surface_id.as_deref())?;
        let worktree_name = clean_optional_id("worktree_name", input.worktree_name.as_deref())?;
        let status = clean_optional_short("status", input.status.as_deref())?;
        let assigned_task_id =
            clean_optional_id("assigned_task_id", input.assigned_task_id.as_deref())?;
        let team = self.team_mut(&team_id)?;
        if let Some(task_id) = assigned_task_id.as_deref() {
            ensure_task_exists(team, task_id)?;
        }
        let index = match team
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
        {
            Some(index) => index,
            None => {
                if team.workers.len() >= MAX_WORKERS_PER_TEAM {
                    return Err(TeamError::Invalid(format!(
                        "team worker list is full (limit {MAX_WORKERS_PER_TEAM})"
                    )));
                }
                team.workers.push(TeamWorker {
                    id: worker_id.clone(),
                    role: None,
                    agent: None,
                    surface_id: None,
                    worktree_name: None,
                    status: "idle".to_string(),
                    assigned_task_id: None,
                    last_heartbeat_ms: 0,
                    launched_at_ms: 0,
                    last_nudge_ms: 0,
                    shutdown_requested_at_ms: 0,
                });
                team.workers.len() - 1
            }
        };
        let worker = &mut team.workers[index];
        if role.is_some() {
            worker.role = role;
        }
        if agent.is_some() {
            worker.agent = agent;
        }
        if surface_id.is_some() {
            worker.surface_id = surface_id;
        }
        if worktree_name.is_some() {
            worker.worktree_name = worktree_name;
        }
        if let Some(status) = status {
            worker.status = status;
        }
        if assigned_task_id.is_some() {
            worker.assigned_task_id = assigned_task_id;
        }
        team.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.upserted",
            format!("worker {}", worker.id),
            now_ms,
        );
        Ok(worker)
    }

    pub fn launch_worker(
        &mut self,
        input: TeamWorkerLaunch,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let role = clean_optional_short("role", input.role.as_deref())?;
        let agent = clean_short("agent", &input.agent)?;
        let surface_id = clean_id("surface_id", &input.surface_id)?;
        let worktree_name = clean_optional_id("worktree_name", input.worktree_name.as_deref())?;
        let assigned_task_id =
            clean_optional_id("assigned_task_id", input.assigned_task_id.as_deref())?;
        let team = self.team_mut(&team_id)?;
        if let Some(task_id) = assigned_task_id.as_deref() {
            ensure_task_exists(team, task_id)?;
        }
        let index = match team
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
        {
            Some(index) => index,
            None => {
                if team.workers.len() >= MAX_WORKERS_PER_TEAM {
                    return Err(TeamError::Invalid(format!(
                        "team worker list is full (limit {MAX_WORKERS_PER_TEAM})"
                    )));
                }
                team.workers.push(TeamWorker {
                    id: worker_id.clone(),
                    role: None,
                    agent: None,
                    surface_id: None,
                    worktree_name: None,
                    status: "idle".to_string(),
                    assigned_task_id: None,
                    last_heartbeat_ms: 0,
                    launched_at_ms: 0,
                    last_nudge_ms: 0,
                    shutdown_requested_at_ms: 0,
                });
                team.workers.len() - 1
            }
        };
        let worker = &mut team.workers[index];
        if role.is_some() {
            worker.role = role;
        }
        worker.agent = Some(agent);
        worker.surface_id = Some(surface_id);
        if worktree_name.is_some() {
            worker.worktree_name = worktree_name;
        }
        if assigned_task_id.is_some() {
            worker.assigned_task_id = assigned_task_id;
        }
        worker.status = "running".to_string();
        worker.launched_at_ms = now_ms;
        worker.last_heartbeat_ms = now_ms;
        worker.shutdown_requested_at_ms = 0;
        team.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.launched",
            format!("worker {}", worker.id),
            now_ms,
        );
        Ok(worker)
    }

    pub fn heartbeat(
        &mut self,
        input: TeamHeartbeat,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let status = clean_optional_short("status", input.status.as_deref())?;
        let assigned_task_id =
            clean_optional_id("assigned_task_id", input.assigned_task_id.as_deref())?;
        let team = self.team_mut(&team_id)?;
        if let Some(task_id) = assigned_task_id.as_deref() {
            ensure_task_exists(team, task_id)?;
        }
        let worker = team
            .workers
            .iter_mut()
            .find(|worker| worker.id == worker_id)
            .ok_or_else(|| TeamError::WorkerNotFound(worker_id.clone()))?;
        if let Some(status) = status {
            worker.status = status;
        }
        if assigned_task_id.is_some() {
            worker.assigned_task_id = assigned_task_id;
        }
        worker.last_heartbeat_ms = now_ms;
        team.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.heartbeat",
            format!("worker {}", worker.id),
            now_ms,
        );
        Ok(worker)
    }

    pub fn mark_worker_nudged(
        &mut self,
        input: TeamWorkerAction,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let team = self.team_mut(&team_id)?;
        let worker = team
            .workers
            .iter_mut()
            .find(|worker| worker.id == worker_id)
            .ok_or_else(|| TeamError::WorkerNotFound(worker_id.clone()))?;
        worker.last_nudge_ms = now_ms;
        team.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.nudged",
            format!("worker {}", worker.id),
            now_ms,
        );
        Ok(worker)
    }

    pub fn request_worker_shutdown(
        &mut self,
        input: TeamWorkerAction,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let team = self.team_mut(&team_id)?;
        let worker = team
            .workers
            .iter_mut()
            .find(|worker| worker.id == worker_id)
            .ok_or_else(|| TeamError::WorkerNotFound(worker_id.clone()))?;
        worker.status = "shutdown_requested".to_string();
        worker.shutdown_requested_at_ms = now_ms;
        team.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.shutdown_requested",
            format!("worker {}", worker.id),
            now_ms,
        );
        Ok(worker)
    }

    pub fn upsert_task(
        &mut self,
        input: TeamTaskUpsert,
        now_ms: u64,
    ) -> Result<TeamTask, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let task_id = clean_id("task_id", &input.task_id)?;
        let title = clean_optional_short("title", input.title.as_deref())?;
        let status = clean_optional_short("status", input.status.as_deref())?;
        let detail = clean_optional_long("detail", input.detail.as_deref())?;
        let depends_on = match input.depends_on {
            Some(ids) => Some(
                ids.iter()
                    .map(|id| clean_id("depends_on", id))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => None,
        };
        let assigned_worker_id =
            clean_optional_id("assigned_worker_id", input.assigned_worker_id.as_deref())?;
        let team = self.team_mut(&team_id)?;
        if let Some(worker_id) = assigned_worker_id.as_deref() {
            ensure_worker_exists(team, worker_id)?;
        }
        if let Some(depends_on) = depends_on.as_ref() {
            validate_task_dependencies(team, &task_id, depends_on)?;
        }
        let index = match team.tasks.iter().position(|task| task.id == task_id) {
            Some(index) => index,
            None => {
                if team.tasks.len() >= MAX_TASKS_PER_TEAM {
                    return Err(TeamError::Invalid(format!(
                        "team task list is full (limit {MAX_TASKS_PER_TEAM})"
                    )));
                }
                team.tasks.push(TeamTask {
                    id: task_id.clone(),
                    title: title.clone().unwrap_or_else(|| task_id.clone()),
                    status: status.clone().unwrap_or_else(|| "open".to_string()),
                    detail: detail.clone(),
                    depends_on: depends_on.clone().unwrap_or_default(),
                    assigned_worker_id: assigned_worker_id.clone(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                });
                team.tasks.len() - 1
            }
        };
        let task = &mut team.tasks[index];
        if let Some(title) = title {
            task.title = title;
        }
        if let Some(status) = status {
            task.status = status;
        }
        if detail.is_some() {
            task.detail = detail;
        }
        if let Some(depends_on) = depends_on {
            task.depends_on = depends_on;
        }
        if assigned_worker_id.is_some() {
            task.assigned_worker_id = assigned_worker_id;
        }
        task.updated_at_ms = now_ms;
        team.updated_at_ms = now_ms;
        let task = task.clone();
        self.push_event(
            &team_id,
            "team.task.upserted",
            format!("task {}", task.id),
            now_ms,
        );
        Ok(task)
    }

    pub fn send_message(
        &mut self,
        input: TeamMessageSend,
        now_ms: u64,
    ) -> Result<TeamMessage, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let from = clean_id("from", &input.from)?;
        let to_worker_id = clean_optional_id("to_worker_id", input.to_worker_id.as_deref())?;
        let task_id = clean_optional_id("task_id", input.task_id.as_deref())?;
        let body = clean_long("body", &input.body)?;
        let message_id = match input.message_id {
            Some(id) => clean_id("message_id", &id)?,
            None => format!("msg-{}-{}", now_ms, self.next_event_seq),
        };
        let team = self.team_mut(&team_id)?;
        if let Some(worker_id) = to_worker_id.as_deref() {
            ensure_worker_exists(team, worker_id)?;
        }
        if let Some(task_id) = task_id.as_deref() {
            ensure_task_exists(team, task_id)?;
        }
        if team.messages.iter().any(|message| message.id == message_id) {
            return Err(TeamError::Invalid(format!(
                "message id already exists: {message_id}"
            )));
        }
        if team.messages.len() >= MAX_MESSAGES_PER_TEAM {
            team.messages.remove(0);
        }
        let message = TeamMessage {
            id: message_id,
            from,
            to_worker_id,
            task_id,
            body,
            created_at_ms: now_ms,
            delivered: false,
            acknowledged_at_ms: 0,
        };
        team.messages.push(message.clone());
        team.updated_at_ms = now_ms;
        self.push_event(
            &team_id,
            "team.message.sent",
            format!("message {}", message.id),
            now_ms,
        );
        Ok(message)
    }

    pub fn ack_message(
        &mut self,
        input: TeamMessageAck,
        now_ms: u64,
    ) -> Result<TeamMessage, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let message_id = clean_id("message_id", &input.message_id)?;
        let worker_id = clean_optional_id("worker_id", input.worker_id.as_deref())?;
        let team = self.team_mut(&team_id)?;
        let message = team
            .messages
            .iter_mut()
            .find(|message| {
                message.id == message_id
                    && worker_id.as_deref().is_none_or(|worker_id| {
                        message
                            .to_worker_id
                            .as_deref()
                            .is_none_or(|target| target == worker_id)
                    })
            })
            .ok_or_else(|| TeamError::MessageNotFound(message_id.clone()))?;
        message.delivered = true;
        message.acknowledged_at_ms = now_ms;
        team.updated_at_ms = now_ms;
        let message = message.clone();
        self.push_event(
            &team_id,
            "team.message.acked",
            format!("message {}", message.id),
            now_ms,
        );
        Ok(message)
    }

    pub fn inbox(&self, query: &TeamInboxQuery) -> Result<Vec<TeamMessage>, TeamError> {
        let team_id = clean_id("team_id", &query.team_id)?;
        let worker_id = clean_optional_id("worker_id", query.worker_id.as_deref())?;
        let limit = query
            .limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .min(MAX_QUERY_LIMIT);
        let team = self
            .teams
            .iter()
            .find(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut rows = team
            .messages
            .iter()
            .filter(|message| query.include_delivered || !message.delivered)
            .filter(|message| {
                worker_id.as_deref().is_none_or(|worker_id| {
                    message
                        .to_worker_id
                        .as_deref()
                        .is_none_or(|target| target == worker_id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|message| std::cmp::Reverse(message.created_at_ms));
        rows.truncate(limit);
        Ok(rows)
    }

    pub fn summary(&self, team_id: &str) -> Result<TeamSummary, TeamError> {
        let team_id = clean_id("team_id", team_id)?;
        let team = self
            .teams
            .iter()
            .find(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let workers_active = team
            .workers
            .iter()
            .filter(|worker| matches!(worker.status.as_str(), "running" | "busy" | "active"))
            .count();
        let tasks_open = team
            .tasks
            .iter()
            .filter(|task| !matches!(task.status.as_str(), "done" | "closed" | "cancelled"))
            .count();
        let messages_pending = team
            .messages
            .iter()
            .filter(|message| !message.delivered)
            .count();
        let last_event_seq = self
            .events
            .iter()
            .rev()
            .find(|event| event.team_id == team_id)
            .map(|event| event.seq)
            .unwrap_or(0);
        Ok(TeamSummary {
            team_id: team.id.clone(),
            status: team.status.clone(),
            workers_total: team.workers.len(),
            workers_active,
            tasks_total: team.tasks.len(),
            tasks_open,
            messages_pending,
            last_event_seq,
        })
    }

    pub fn events(&self, query: &TeamEventQuery) -> Result<Vec<TeamEvent>, TeamError> {
        let team_id = clean_optional_id("team_id", query.team_id.as_deref())?;
        let limit = query
            .limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .min(MAX_QUERY_LIMIT);
        let mut rows = self
            .events
            .iter()
            .filter(|event| {
                team_id
                    .as_deref()
                    .is_none_or(|team_id| event.team_id == team_id)
                    && query.since_seq.is_none_or(|seq| event.seq > seq)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|event| event.seq);
        rows.truncate(limit);
        Ok(rows)
    }

    fn team_mut(&mut self, team_id: &str) -> Result<&mut TeamState, TeamError> {
        self.teams
            .iter_mut()
            .find(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.to_string()))
    }

    fn push_event(&mut self, team_id: &str, kind: &str, summary: String, now_ms: u64) {
        let event = TeamEvent {
            seq: self.next_event_seq,
            team_id: team_id.to_string(),
            kind: kind.to_string(),
            summary,
            created_at_ms: now_ms,
        };
        self.next_event_seq = self.next_event_seq.saturating_add(1).max(1);
        self.events.push(event);
        if self.events.len() > MAX_EVENTS {
            let drop_count = self.events.len() - MAX_EVENTS;
            self.events.drain(0..drop_count);
        }
    }
}

impl TeamState {
    fn matches_query(&self, needle: &str) -> bool {
        self.id.to_ascii_lowercase().contains(needle)
            || self.name.to_ascii_lowercase().contains(needle)
            || self
                .goal
                .as_deref()
                .is_some_and(|goal| goal.to_ascii_lowercase().contains(needle))
    }
}

pub fn load_teams() -> Result<TeamStoreData, TeamError> {
    load_teams_from_path(&team_store_path()?)
}

pub fn load_teams_from_path(path: &Path) -> Result<TeamStoreData, TeamError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TeamStoreData::default());
        }
        Err(err) => return Err(err.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TeamError::Invalid(format!(
            "team store path is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_TEAM_STORE_BYTES {
        return Err(TeamError::Invalid(format!(
            "team store exceeds {MAX_TEAM_STORE_BYTES} bytes"
        )));
    }
    let bytes = fs::read(path)?;
    let data: TeamStoreData = serde_json::from_slice(&bytes)?;
    validate_store(&data)?;
    Ok(data)
}

pub fn save_teams_to_path(path: &Path, data: &TeamStoreData) -> Result<(), TeamError> {
    validate_store(data)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let bytes = serde_json::to_vec_pretty(data)?;
    if bytes.len() as u64 > MAX_TEAM_STORE_BYTES {
        return Err(TeamError::Invalid(format!(
            "team store exceeds {MAX_TEAM_STORE_BYTES} bytes"
        )));
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    sync_parent_dir(path)?;
    Ok(())
}

pub fn update_teams<F, T>(update: F) -> Result<T, TeamError>
where
    F: FnOnce(&mut TeamStoreData) -> Result<T, TeamError>,
{
    update_teams_at_path(&team_store_path()?, update)
}

pub fn update_teams_at_path<F, T>(path: &Path, update: F) -> Result<T, TeamError>
where
    F: FnOnce(&mut TeamStoreData) -> Result<T, TeamError>,
{
    let _guard = TEAM_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| TeamError::Invalid("team store lock poisoned".to_string()))?;
    let mut data = load_teams_from_path(path)?;
    let result = update(&mut data)?;
    save_teams_to_path(path, &data)?;
    Ok(result)
}

fn sync_parent_dir(path: &Path) -> Result<(), TeamError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn team_store_path() -> Result<PathBuf, TeamError> {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| TeamError::Invalid("Unable to resolve user state directory".to_string()))?;
    Ok(base.join("forktty").join("team-v1.json"))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn validate_store(data: &TeamStoreData) -> Result<(), TeamError> {
    if data.version != TEAM_FORMAT_VERSION {
        return Err(TeamError::UnsupportedVersion(data.version));
    }
    if data.teams.len() > MAX_TEAMS {
        return Err(TeamError::Invalid(format!(
            "too many teams (limit {MAX_TEAMS})"
        )));
    }
    let mut team_ids = BTreeSet::new();
    for team in &data.teams {
        clean_id("team_id", &team.id)?;
        if !team_ids.insert(team.id.clone()) {
            return Err(TeamError::Invalid(format!(
                "duplicate team id: {}",
                team.id
            )));
        }
        clean_short("name", &team.name)?;
        clean_short("status", &team.status)?;
        if let Some(goal) = &team.goal {
            clean_long("goal", goal)?;
        }
        if team.workers.len() > MAX_WORKERS_PER_TEAM
            || team.tasks.len() > MAX_TASKS_PER_TEAM
            || team.messages.len() > MAX_MESSAGES_PER_TEAM
        {
            return Err(TeamError::Invalid(format!(
                "team {} exceeds item caps",
                team.id
            )));
        }
        validate_team_references(team)?;
    }
    Ok(())
}

fn validate_team_references(team: &TeamState) -> Result<(), TeamError> {
    let worker_ids = team
        .workers
        .iter()
        .map(|worker| clean_id("worker_id", &worker.id))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let task_ids = team
        .tasks
        .iter()
        .map(|task| clean_id("task_id", &task.id))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if worker_ids.len() != team.workers.len() || task_ids.len() != team.tasks.len() {
        return Err(TeamError::Invalid(format!(
            "team {} has duplicate worker or task ids",
            team.id
        )));
    }
    for worker in &team.workers {
        if let Some(role) = &worker.role {
            clean_short("worker.role", role)?;
        }
        if let Some(agent) = &worker.agent {
            clean_short("worker.agent", agent)?;
        }
        if let Some(surface_id) = &worker.surface_id {
            clean_id("worker.surface_id", surface_id)?;
        }
        if let Some(worktree_name) = &worker.worktree_name {
            clean_id("worker.worktree_name", worktree_name)?;
        }
        clean_short("worker.status", &worker.status)?;
        if let Some(task_id) = worker.assigned_task_id.as_deref() {
            if !task_ids.contains(task_id) {
                return Err(TeamError::TaskNotFound(task_id.to_string()));
            }
        }
    }
    for task in &team.tasks {
        clean_short("task.title", &task.title)?;
        clean_short("task.status", &task.status)?;
        if let Some(worker_id) = task.assigned_worker_id.as_deref() {
            if !worker_ids.contains(worker_id) {
                return Err(TeamError::WorkerNotFound(worker_id.to_string()));
            }
        }
        validate_task_dependencies(team, &task.id, &task.depends_on)?;
    }
    for message in &team.messages {
        clean_id("message_id", &message.id)?;
        clean_id("message.from", &message.from)?;
        clean_long("message.body", &message.body)?;
        if let Some(worker_id) = message.to_worker_id.as_deref() {
            if !worker_ids.contains(worker_id) {
                return Err(TeamError::WorkerNotFound(worker_id.to_string()));
            }
        }
        if let Some(task_id) = message.task_id.as_deref() {
            if !task_ids.contains(task_id) {
                return Err(TeamError::TaskNotFound(task_id.to_string()));
            }
        }
    }
    Ok(())
}

fn validate_task_dependencies(
    team: &TeamState,
    task_id: &str,
    depends_on: &[String],
) -> Result<(), TeamError> {
    if depends_on.iter().any(|dependency| dependency == task_id) {
        return Err(TeamError::Invalid(format!(
            "task {task_id} cannot depend on itself"
        )));
    }
    let task_ids = team
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    for dependency in depends_on {
        if !task_ids.contains(dependency.as_str()) {
            return Err(TeamError::TaskNotFound(dependency.clone()));
        }
    }

    let mut graph = team
        .tasks
        .iter()
        .map(|task| {
            (
                task.id.as_str(),
                task.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect::<HashMap<_, Vec<_>>>();
    graph.insert(task_id, depends_on.iter().map(String::as_str).collect());
    if has_cycle(task_id, &graph) {
        return Err(TeamError::Invalid(format!(
            "task dependencies would create a cycle at {task_id}"
        )));
    }
    Ok(())
}

fn has_cycle<'a>(start: &'a str, graph: &HashMap<&'a str, Vec<&'a str>>) -> bool {
    let mut stack = VecDeque::from([(start, false)]);
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    while let Some((node, exit)) = stack.pop_back() {
        if exit {
            visiting.remove(node);
            visited.insert(node);
            continue;
        }
        if visited.contains(node) {
            continue;
        }
        if !visiting.insert(node) {
            return true;
        }
        stack.push_back((node, true));
        for dependency in graph.get(node).into_iter().flatten() {
            if visiting.contains(dependency) {
                return true;
            }
            stack.push_back((dependency, false));
        }
    }
    false
}

fn ensure_worker_exists(team: &TeamState, worker_id: &str) -> Result<(), TeamError> {
    if team.workers.iter().any(|worker| worker.id == worker_id) {
        Ok(())
    } else {
        Err(TeamError::WorkerNotFound(worker_id.to_string()))
    }
}

fn ensure_task_exists(team: &TeamState, task_id: &str) -> Result<(), TeamError> {
    if team.tasks.iter().any(|task| task.id == task_id) {
        Ok(())
    } else {
        Err(TeamError::TaskNotFound(task_id.to_string()))
    }
}

fn clean_optional_id(field: &str, value: Option<&str>) -> Result<Option<String>, TeamError> {
    value.map(|value| clean_id(field, value)).transpose()
}

fn clean_id(field: &str, value: &str) -> Result<String, TeamError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 160 {
        return Err(TeamError::Invalid(format!("invalid {field}")));
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/' | b'@')
    }) {
        return Err(TeamError::Invalid(format!("invalid {field}")));
    }
    Ok(value.to_string())
}

fn clean_optional_short(field: &str, value: Option<&str>) -> Result<Option<String>, TeamError> {
    value.map(|value| clean_short(field, value)).transpose()
}

fn clean_short(field: &str, value: &str) -> Result<String, TeamError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_SHORT_TEXT_BYTES || value.chars().any(char::is_control)
    {
        return Err(TeamError::Invalid(format!("invalid {field}")));
    }
    Ok(value.to_string())
}

fn clean_optional_long(field: &str, value: Option<&str>) -> Result<Option<String>, TeamError> {
    value.map(|value| clean_long(field, value)).transpose()
}

fn clean_long(field: &str, value: &str) -> Result<String, TeamError> {
    if value.is_empty()
        || value.len() > MAX_LONG_TEXT_BYTES
        || value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err(TeamError::Invalid(format!("invalid {field}")));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_store_tracks_workers_tasks_messages_and_summary() {
        let mut store = TeamStoreData::default();
        let team = store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: Some("surface-1".to_string()),
                    name: Some("Launch".to_string()),
                    status: Some("active".to_string()),
                    goal: Some("ship it".to_string()),
                },
                1,
            )
            .unwrap();
        assert_eq!(team.name, "Launch");

        store
            .upsert_worker(
                TeamWorkerUpsert {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    role: Some("implementer".to_string()),
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-2".to_string()),
                    worktree_name: Some("feature/team".to_string()),
                    status: Some("idle".to_string()),
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();
        store
            .upsert_task(
                TeamTaskUpsert {
                    team_id: "team-1".to_string(),
                    task_id: "task-1".to_string(),
                    title: Some("Build runtime".to_string()),
                    status: Some("open".to_string()),
                    detail: Some("control plane".to_string()),
                    depends_on: Some(Vec::new()),
                    assigned_worker_id: Some("worker-1".to_string()),
                },
                3,
            )
            .unwrap();
        let worker = store
            .heartbeat(
                TeamHeartbeat {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    status: Some("running".to_string()),
                    assigned_task_id: Some("task-1".to_string()),
                },
                4,
            )
            .unwrap();
        assert_eq!(worker.last_heartbeat_ms, 4);

        let message = store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: Some("msg-1".to_string()),
                    from: "leader".to_string(),
                    to_worker_id: Some("worker-1".to_string()),
                    task_id: Some("task-1".to_string()),
                    body: "please continue\n".to_string(),
                },
                5,
            )
            .unwrap();
        assert_eq!(message.body, "please continue\n");
        assert_eq!(
            store
                .inbox(&TeamInboxQuery {
                    team_id: "team-1".to_string(),
                    worker_id: Some("worker-1".to_string()),
                    include_delivered: false,
                    limit: None,
                })
                .unwrap()
                .len(),
            1
        );
        store
            .ack_message(
                TeamMessageAck {
                    team_id: "team-1".to_string(),
                    message_id: "msg-1".to_string(),
                    worker_id: Some("worker-1".to_string()),
                },
                6,
            )
            .unwrap();
        let summary = store.summary("team-1").unwrap();
        assert_eq!(summary.workers_active, 1);
        assert_eq!(summary.tasks_open, 1);
        assert_eq!(summary.messages_pending, 0);
        assert_eq!(
            store
                .events(&TeamEventQuery {
                    team_id: None,
                    since_seq: None,
                    limit: None
                })
                .unwrap()
                .len(),
            6
        );
    }

    #[test]
    fn generated_messages_are_unique_and_team_wide_messages_reach_worker_inbox() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: None,
                    goal: None,
                },
                1,
            )
            .unwrap();
        store
            .upsert_worker(
                TeamWorkerUpsert {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    role: None,
                    agent: None,
                    surface_id: None,
                    worktree_name: None,
                    status: None,
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();
        let first = store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: None,
                    from: "leader".to_string(),
                    to_worker_id: None,
                    task_id: None,
                    body: "all".to_string(),
                },
                3,
            )
            .unwrap();
        let second = store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: None,
                    from: "leader".to_string(),
                    to_worker_id: Some("worker-1".to_string()),
                    task_id: None,
                    body: "direct".to_string(),
                },
                3,
            )
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(
            store
                .inbox(&TeamInboxQuery {
                    team_id: "team-1".to_string(),
                    worker_id: Some("worker-1".to_string()),
                    include_delivered: false,
                    limit: None,
                })
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn task_dependencies_must_form_a_dag() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: None,
                    goal: None,
                },
                1,
            )
            .unwrap();
        store
            .upsert_task(
                TeamTaskUpsert {
                    team_id: "team-1".to_string(),
                    task_id: "a".to_string(),
                    title: None,
                    status: None,
                    detail: None,
                    depends_on: Some(Vec::new()),
                    assigned_worker_id: None,
                },
                2,
            )
            .unwrap();
        store
            .upsert_task(
                TeamTaskUpsert {
                    team_id: "team-1".to_string(),
                    task_id: "b".to_string(),
                    title: None,
                    status: None,
                    detail: None,
                    depends_on: Some(vec!["a".to_string()]),
                    assigned_worker_id: None,
                },
                3,
            )
            .unwrap();

        let err = store
            .upsert_task(
                TeamTaskUpsert {
                    team_id: "team-1".to_string(),
                    task_id: "a".to_string(),
                    title: None,
                    status: None,
                    detail: None,
                    depends_on: Some(vec!["b".to_string()]),
                    assigned_worker_id: None,
                },
                4,
            )
            .unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn team_store_round_trips_owner_private_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team-v1.json");
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: Some("Team".to_string()),
                    status: None,
                    goal: None,
                },
                1,
            )
            .unwrap();
        save_teams_to_path(&path, &store).unwrap();

        let loaded = load_teams_from_path(&path).unwrap();
        assert_eq!(loaded.teams[0].id, "team-1");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
