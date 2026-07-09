//! Durable provider-neutral team, worker, task, message, and event state store.

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

#[derive(Debug)]
struct TeamStoreFileLock {
    _file: fs::File,
}

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
    #[error("Team state conflict: {0}")]
    Conflict(String),
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
    pub launched_surface_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub superseded: bool,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub superseded_at_ms: u64,
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
    pub report: Option<String>,
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
    pub cwd: Option<PathBuf>,
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
pub struct TeamMessagesSupersede {
    pub team_id: String,
    pub message_ids: Vec<String>,
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
    #[serde(default)]
    pub workers_with_reports: usize,
    pub tasks_total: usize,
    pub tasks_open: usize,
    pub messages_pending: usize,
    pub last_event_seq: u64,
    #[serde(default)]
    pub consistency_warnings: Vec<String>,
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

fn is_false(value: &bool) -> bool {
    !*value
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
        let existing_index = self.teams.iter().position(|team| team.id == team_id);
        let evict_index = if existing_index.is_none() && self.teams.len() >= MAX_TEAMS {
            Some(oldest_terminal_team_index(&self.teams).ok_or_else(|| {
                TeamError::Invalid(format!("team store is full (limit {MAX_TEAMS})"))
            })?)
        } else {
            None
        };
        let mut updated = match existing_index {
            Some(index) => self.teams[index].clone(),
            None => TeamState {
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
            },
        };

        let workspace_changed = workspace_id
            .as_ref()
            .is_some_and(|workspace_id| updated.workspace_id.as_ref() != Some(workspace_id));
        if workspace_changed && leader_surface_id.is_none() {
            updated.leader_surface_id = None;
        }
        if workspace_id.is_some() {
            updated.workspace_id = workspace_id;
        }
        if leader_surface_id.is_some() {
            updated.leader_surface_id = leader_surface_id;
        }
        if let Some(name) = name {
            updated.name = name;
        }
        if let Some(status) = status {
            updated.status = status;
        }
        if goal.is_some() {
            updated.goal = goal;
        }
        updated.updated_at_ms = now_ms;
        let team = updated.clone();
        self.push_event(
            &team_id,
            "team.upserted",
            format!("team {}", team.id),
            now_ms,
        )?;
        match existing_index {
            Some(index) => self.teams[index] = updated,
            None => {
                if let Some(index) = evict_index {
                    self.teams.remove(index);
                }
                self.teams.push(updated);
            }
        }
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
        let report = clean_optional_long("report", input.report.as_deref())?;
        let status = clean_optional_short("status", input.status.as_deref())?;
        let assigned_task_id =
            clean_optional_id("assigned_task_id", input.assigned_task_id.as_deref())?;
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        if let Some(task_id) = assigned_task_id.as_deref() {
            ensure_task_exists(&updated, task_id)?;
        }
        let worker_index = match updated
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
        {
            Some(index) => index,
            None => {
                if updated.workers.len() >= MAX_WORKERS_PER_TEAM {
                    return Err(TeamError::Invalid(format!(
                        "team worker list is full (limit {MAX_WORKERS_PER_TEAM})"
                    )));
                }
                updated.workers.push(TeamWorker {
                    id: worker_id.clone(),
                    role: None,
                    agent: None,
                    surface_id: None,
                    launched_surface_id: None,
                    worktree_name: None,
                    cwd: None,
                    report: None,
                    status: "idle".to_string(),
                    assigned_task_id: None,
                    last_heartbeat_ms: 0,
                    launched_at_ms: 0,
                    last_nudge_ms: 0,
                    shutdown_requested_at_ms: 0,
                });
                updated.workers.len() - 1
            }
        };
        let worker = &mut updated.workers[worker_index];
        if role.is_some() {
            worker.role = role;
        }
        if agent.is_some() {
            worker.agent = agent;
        }
        if surface_id.is_some() {
            worker.surface_id = surface_id;
            worker.launched_surface_id = None;
            worker.cwd = None;
        }
        if worktree_name.is_some() {
            worker.worktree_name = worktree_name;
        }
        if report.is_some() {
            worker.report = report;
        }
        if let Some(status) = status {
            worker.status = status;
            if worker_status_is_active(&worker.status) {
                worker.shutdown_requested_at_ms = 0;
            }
        }
        if assigned_task_id.is_some() {
            worker.assigned_task_id = assigned_task_id;
        }
        updated.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.upserted",
            format!("worker {}", worker.id),
            now_ms,
        )?;
        self.teams[index] = updated;
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
        let cwd = clean_optional_absolute_cwd(input.cwd.as_deref())?;
        let assigned_task_id =
            clean_optional_id("assigned_task_id", input.assigned_task_id.as_deref())?;
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        if let Some(task_id) = assigned_task_id.as_deref() {
            ensure_task_exists(&updated, task_id)?;
        }
        let worker_index = match updated
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
        {
            Some(index) => index,
            None => {
                if updated.workers.len() >= MAX_WORKERS_PER_TEAM {
                    return Err(TeamError::Invalid(format!(
                        "team worker list is full (limit {MAX_WORKERS_PER_TEAM})"
                    )));
                }
                updated.workers.push(TeamWorker {
                    id: worker_id.clone(),
                    role: None,
                    agent: None,
                    surface_id: None,
                    launched_surface_id: None,
                    worktree_name: None,
                    cwd: None,
                    report: None,
                    status: "idle".to_string(),
                    assigned_task_id: None,
                    last_heartbeat_ms: 0,
                    launched_at_ms: 0,
                    last_nudge_ms: 0,
                    shutdown_requested_at_ms: 0,
                });
                updated.workers.len() - 1
            }
        };
        let worker = &mut updated.workers[worker_index];
        if role.is_some() {
            worker.role = role;
        }
        worker.agent = Some(agent);
        worker.surface_id = Some(surface_id);
        worker.launched_surface_id = worker.surface_id.clone();
        if worktree_name.is_some() {
            worker.worktree_name = worktree_name;
        }
        worker.cwd = cwd;
        worker.report = None;
        if assigned_task_id.is_some() {
            worker.assigned_task_id = assigned_task_id;
        }
        worker.status = "running".to_string();
        worker.launched_at_ms = now_ms;
        worker.last_heartbeat_ms = 0;
        worker.shutdown_requested_at_ms = 0;
        updated.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.launched",
            format!("worker {}", worker.id),
            now_ms,
        )?;
        self.teams[index] = updated;
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
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        if let Some(task_id) = assigned_task_id.as_deref() {
            ensure_task_exists(&updated, task_id)?;
        }
        let worker = updated
            .workers
            .iter_mut()
            .find(|worker| worker.id == worker_id)
            .ok_or_else(|| TeamError::WorkerNotFound(worker_id.clone()))?;
        if let Some(status) = status {
            worker.status = status;
            if worker_status_is_active(&worker.status) {
                worker.shutdown_requested_at_ms = 0;
            }
        }
        if assigned_task_id.is_some() {
            worker.assigned_task_id = assigned_task_id;
        }
        worker.last_heartbeat_ms = now_ms;
        updated.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.heartbeat",
            format!("worker {}", worker.id),
            now_ms,
        )?;
        self.teams[index] = updated;
        Ok(worker)
    }

    pub fn mark_worker_nudged(
        &mut self,
        input: TeamWorkerAction,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        let worker = updated
            .workers
            .iter_mut()
            .find(|worker| worker.id == worker_id)
            .ok_or_else(|| TeamError::WorkerNotFound(worker_id.clone()))?;
        worker.last_nudge_ms = now_ms;
        updated.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.nudged",
            format!("worker {}", worker.id),
            now_ms,
        )?;
        self.teams[index] = updated;
        Ok(worker)
    }

    pub fn request_worker_shutdown(
        &mut self,
        input: TeamWorkerAction,
        now_ms: u64,
    ) -> Result<TeamWorker, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let worker_id = clean_id("worker_id", &input.worker_id)?;
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        let worker = updated
            .workers
            .iter_mut()
            .find(|worker| worker.id == worker_id)
            .ok_or_else(|| TeamError::WorkerNotFound(worker_id.clone()))?;
        worker.status = "shutdown_requested".to_string();
        worker.shutdown_requested_at_ms = now_ms;
        updated.updated_at_ms = now_ms;
        let worker = worker.clone();
        self.push_event(
            &team_id,
            "team.worker.shutdown_requested",
            format!("worker {}", worker.id),
            now_ms,
        )?;
        self.teams[index] = updated;
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
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        if let Some(worker_id) = assigned_worker_id.as_deref() {
            ensure_worker_exists(&updated, worker_id)?;
        }
        if let Some(depends_on) = depends_on.as_ref() {
            validate_task_dependencies(&updated, &task_id, depends_on)?;
        }
        let task_index = match updated.tasks.iter().position(|task| task.id == task_id) {
            Some(index) => index,
            None => {
                if updated.tasks.len() >= MAX_TASKS_PER_TEAM {
                    return Err(TeamError::Invalid(format!(
                        "team task list is full (limit {MAX_TASKS_PER_TEAM})"
                    )));
                }
                updated.tasks.push(TeamTask {
                    id: task_id.clone(),
                    title: title.clone().unwrap_or_else(|| task_id.clone()),
                    status: status.clone().unwrap_or_else(|| "open".to_string()),
                    detail: detail.clone(),
                    depends_on: depends_on.clone().unwrap_or_default(),
                    assigned_worker_id: assigned_worker_id.clone(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                });
                updated.tasks.len() - 1
            }
        };
        let task = &mut updated.tasks[task_index];
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
        updated.updated_at_ms = now_ms;
        let task = task.clone();
        self.push_event(
            &team_id,
            "team.task.upserted",
            format!("task {}", task.id),
            now_ms,
        )?;
        self.teams[index] = updated;
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
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        if let Some(worker_id) = to_worker_id.as_deref() {
            ensure_worker_exists(&updated, worker_id)?;
        }
        if let Some(task_id) = task_id.as_deref() {
            ensure_task_exists(&updated, task_id)?;
        }
        if updated
            .messages
            .iter()
            .any(|message| message.id == message_id)
        {
            return Err(TeamError::Invalid(format!(
                "message id already exists: {message_id}"
            )));
        }
        if updated.messages.len() >= MAX_MESSAGES_PER_TEAM {
            let Some(index) = updated
                .messages
                .iter()
                .position(|message| message.delivered || message.superseded)
            else {
                return Err(TeamError::Conflict(
                    "team message list is full with pending messages".to_string(),
                ));
            };
            updated.messages.remove(index);
        }
        let message = TeamMessage {
            id: message_id,
            from,
            to_worker_id,
            task_id,
            body,
            created_at_ms: now_ms,
            delivered: false,
            superseded: false,
            superseded_at_ms: 0,
            acknowledged_at_ms: 0,
        };
        updated.messages.push(message.clone());
        updated.updated_at_ms = now_ms;
        self.push_event(
            &team_id,
            "team.message.sent",
            format!("message {}", message.id),
            now_ms,
        )?;
        self.teams[index] = updated;
        Ok(message)
    }

    pub fn supersede_messages(
        &mut self,
        input: TeamMessagesSupersede,
        now_ms: u64,
    ) -> Result<Vec<TeamMessage>, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let message_ids = input
            .message_ids
            .into_iter()
            .map(|message_id| clean_id("message_id", &message_id))
            .collect::<Result<Vec<_>, _>>()?;
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        let mut superseded = Vec::new();
        for message in updated.messages.iter_mut() {
            if message_ids
                .iter()
                .any(|message_id| message_id == &message.id)
                && !message.delivered
                && !message.superseded
            {
                message.superseded = true;
                message.superseded_at_ms = now_ms;
                superseded.push(message.clone());
            }
        }
        if !superseded.is_empty() {
            updated.updated_at_ms = now_ms;
            self.push_event(
                &team_id,
                "team.message.superseded",
                format!("messages {}", superseded.len()),
                now_ms,
            )?;
            self.teams[index] = updated;
        }
        Ok(superseded)
    }

    pub fn ack_message(
        &mut self,
        input: TeamMessageAck,
        now_ms: u64,
    ) -> Result<TeamMessage, TeamError> {
        let team_id = clean_id("team_id", &input.team_id)?;
        let message_id = clean_id("message_id", &input.message_id)?;
        let worker_id = clean_optional_id("worker_id", input.worker_id.as_deref())?;
        let index = self
            .teams
            .iter()
            .position(|team| team.id == team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        let mut updated = self.teams[index].clone();
        let message = updated
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
        if message.superseded {
            return Err(TeamError::Conflict("message was superseded".to_string()));
        }
        if message.delivered {
            return Ok(message.clone());
        }
        message.delivered = true;
        message.acknowledged_at_ms = now_ms;
        updated.updated_at_ms = now_ms;
        let message = message.clone();
        self.push_event(
            &team_id,
            "team.message.acked",
            format!("message {}", message.id),
            now_ms,
        )?;
        self.teams[index] = updated;
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
            .filter(|message| {
                query.include_delivered || (!message.delivered && !message.superseded)
            })
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
            .filter(|worker| {
                matches!(
                    worker.status.as_str(),
                    "running" | "busy" | "active" | "working"
                )
            })
            .count();
        let workers_with_reports = team
            .workers
            .iter()
            .filter(|worker| {
                worker
                    .report
                    .as_deref()
                    .is_some_and(|report| !report.is_empty())
            })
            .count();
        let tasks_open = team
            .tasks
            .iter()
            .filter(|task| !matches!(task.status.as_str(), "done" | "closed" | "cancelled"))
            .count();
        let messages_pending = team
            .messages
            .iter()
            .filter(|message| !message.delivered && !message.superseded)
            .count();
        let last_event_seq = self
            .events
            .iter()
            .rev()
            .find(|event| event.team_id == team_id)
            .map(|event| event.seq)
            .unwrap_or(0);
        let mut consistency_warnings = Vec::new();
        if team.status == "done" {
            if workers_active > 0 {
                consistency_warnings.push("done_with_active_workers".to_string());
            }
            if tasks_open > 0 {
                consistency_warnings.push("done_with_open_tasks".to_string());
            }
            if messages_pending > 0 {
                consistency_warnings.push("done_with_pending_messages".to_string());
            }
        } else if matches!(team.status.as_str(), "active" | "running" | "working")
            && workers_active == 0
            && tasks_open == 0
            && messages_pending == 0
            && (!team.workers.is_empty() || !team.tasks.is_empty() || !team.messages.is_empty())
        {
            consistency_warnings.push("active_without_open_work".to_string());
        }
        Ok(TeamSummary {
            team_id: team.id.clone(),
            status: team.status.clone(),
            workers_total: team.workers.len(),
            workers_active,
            workers_with_reports,
            tasks_total: team.tasks.len(),
            tasks_open,
            messages_pending,
            last_event_seq,
            consistency_warnings,
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

    fn push_event(
        &mut self,
        team_id: &str,
        kind: &str,
        summary: String,
        now_ms: u64,
    ) -> Result<(), TeamError> {
        let highest_seq = self
            .events
            .last()
            .map(|last| last.seq)
            .unwrap_or(0)
            .max(self.next_event_seq.saturating_sub(1));
        let seq = highest_seq
            .checked_add(1)
            .ok_or_else(|| TeamError::Invalid("team event sequence overflow".to_string()))?;
        let event = TeamEvent {
            seq,
            team_id: clean_id("event.team_id", team_id)?,
            kind: clean_short("event.kind", kind)?,
            summary: clean_short("event.summary", &summary)?,
            created_at_ms: now_ms,
        };
        self.next_event_seq = event.seq.saturating_add(1);
        self.events.push(event);
        if self.events.len() > MAX_EVENTS {
            let drop_count = self.events.len() - MAX_EVENTS;
            self.events.drain(0..drop_count);
        }
        Ok(())
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
    let mut data: TeamStoreData = serde_json::from_slice(&bytes)?;
    repair_loaded_store(&mut data)?;
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
    let _file_lock = acquire_team_store_file_lock(path)?;
    let _guard = TEAM_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| TeamError::Invalid("team store lock poisoned".to_string()))?;
    let mut data = load_teams_from_path(path)?;
    let result = update(&mut data)?;
    save_teams_to_path(path, &data)?;
    Ok(result)
}

fn acquire_team_store_file_lock(path: &Path) -> Result<TeamStoreFileLock, TeamError> {
    let lock_path = team_store_lock_path(path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock_path)?;
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600))?;
    file.lock()?;
    Ok(TeamStoreFileLock { _file: file })
}

fn team_store_lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
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
    if data.events.len() > MAX_EVENTS {
        return Err(TeamError::Invalid(format!(
            "too many team events (limit {MAX_EVENTS})"
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
    let mut last_seq = 0;
    for event in &data.events {
        clean_id("event.team_id", &event.team_id)?;
        clean_short("event.kind", &event.kind)?;
        clean_short("event.summary", &event.summary)?;
        if event.seq <= last_seq {
            return Err(TeamError::Invalid(
                "team events must be sorted by increasing seq".to_string(),
            ));
        }
        last_seq = event.seq;
    }
    Ok(())
}

fn repair_loaded_store(data: &mut TeamStoreData) -> Result<(), TeamError> {
    for team in &mut data.teams {
        let mut repaired: Vec<TeamMessage> = Vec::with_capacity(team.messages.len());
        for message in std::mem::take(&mut team.messages) {
            if let Some(existing) = repaired
                .iter_mut()
                .find(|existing| existing.id == message.id)
            {
                merge_legacy_duplicate_message_state(existing, &message);
            } else {
                repaired.push(message);
            }
        }
        team.messages = repaired;
    }

    let mut last_seq = 0;
    let needs_event_resequence = data.events.iter().any(|event| {
        let non_increasing = event.seq <= last_seq;
        last_seq = event.seq;
        non_increasing
    });
    if needs_event_resequence {
        for (index, event) in data.events.iter_mut().enumerate() {
            event.seq = u64::try_from(index + 1)
                .map_err(|_| TeamError::Invalid("team event sequence overflow".to_string()))?;
        }
        data.next_event_seq = u64::try_from(data.events.len() + 1)
            .map_err(|_| TeamError::Invalid("team event sequence overflow".to_string()))?;
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
        if let Some(report) = &worker.report {
            clean_long("worker.report", report)?;
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
    let mut message_ids = BTreeSet::new();
    for message in &team.messages {
        let message_id = clean_id("message_id", &message.id)?;
        if !message_ids.insert(message_id.clone()) {
            return Err(TeamError::Invalid(format!(
                "team {} has duplicate message id: {message_id}",
                team.id
            )));
        }
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

fn merge_legacy_duplicate_message_state(existing: &mut TeamMessage, duplicate: &TeamMessage) {
    if duplicate.delivered {
        existing.delivered = true;
        existing.acknowledged_at_ms = existing
            .acknowledged_at_ms
            .max(duplicate.acknowledged_at_ms);
    }
    if duplicate.superseded {
        existing.superseded = true;
        existing.superseded_at_ms = existing.superseded_at_ms.max(duplicate.superseded_at_ms);
    }
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

fn oldest_terminal_team_index(teams: &[TeamState]) -> Option<usize> {
    teams
        .iter()
        .enumerate()
        .filter(|(_, team)| is_terminal_team_status(&team.status))
        .min_by(|(_, left), (_, right)| {
            left.updated_at_ms
                .cmp(&right.updated_at_ms)
                .then(left.created_at_ms.cmp(&right.created_at_ms))
                .then(left.id.cmp(&right.id))
        })
        .map(|(index, _)| index)
}

fn is_terminal_team_status(status: &str) -> bool {
    matches!(status, "done" | "closed" | "cancelled" | "finished")
}

fn worker_status_is_active(status: &str) -> bool {
    matches!(status, "running" | "busy" | "active" | "working")
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

fn clean_optional_absolute_cwd(value: Option<&Path>) -> Result<Option<PathBuf>, TeamError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_absolute() {
        return Err(TeamError::Invalid("invalid cwd".to_string()));
    }
    Ok(Some(value.to_path_buf()))
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
    fn moving_team_to_another_workspace_clears_stale_leader_surface() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-a".to_string()),
                    leader_surface_id: Some("surface-a".to_string()),
                    name: None,
                    status: None,
                    goal: None,
                },
                1,
            )
            .unwrap();

        let moved = store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-b".to_string()),
                    leader_surface_id: None,
                    name: None,
                    status: None,
                    goal: None,
                },
                2,
            )
            .unwrap();

        assert_eq!(moved.workspace_id.as_deref(), Some("workspace-b"));
        assert_eq!(moved.leader_surface_id, None);
    }

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
                    report: None,
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
                    assigned_worker_id: None,
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
        assert!(summary.consistency_warnings.is_empty());
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("done".to_string()),
                    goal: None,
                },
                7,
            )
            .unwrap();
        assert_eq!(
            store.summary("team-1").unwrap().consistency_warnings,
            vec![
                "done_with_active_workers".to_string(),
                "done_with_open_tasks".to_string()
            ]
        );
        store
            .heartbeat(
                TeamHeartbeat {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    status: Some("working".to_string()),
                    assigned_task_id: Some("task-1".to_string()),
                },
                8,
            )
            .unwrap();
        assert_eq!(store.summary("team-1").unwrap().workers_active, 1);
        assert_eq!(
            store
                .events(&TeamEventQuery {
                    team_id: None,
                    since_seq: None,
                    limit: None
                })
                .unwrap()
                .len(),
            8
        );
    }

    #[test]
    fn ack_message_rejects_superseded_messages() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: Some("surface-1".to_string()),
                    name: Some("Launch".to_string()),
                    status: Some("active".to_string()),
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
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-2".to_string()),
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();
        store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: Some("msg-stale".to_string()),
                    from: "leader".to_string(),
                    to_worker_id: Some("worker-1".to_string()),
                    task_id: None,
                    body: "stale assignment".to_string(),
                },
                3,
            )
            .unwrap();
        store
            .supersede_messages(
                TeamMessagesSupersede {
                    team_id: "team-1".to_string(),
                    message_ids: vec!["msg-stale".to_string()],
                },
                4,
            )
            .unwrap();

        let ack = store.ack_message(
            TeamMessageAck {
                team_id: "team-1".to_string(),
                message_id: "msg-stale".to_string(),
                worker_id: Some("worker-1".to_string()),
            },
            5,
        );

        assert!(matches!(
            ack,
            Err(TeamError::Conflict(message)) if message.contains("superseded")
        ));
        assert_eq!(
            store.summary("team-1").unwrap().messages_pending,
            0,
            "superseded messages must stay out of pending counts"
        );
    }

    #[test]
    fn ack_message_is_event_idempotent_when_already_delivered() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: Some("surface-1".to_string()),
                    name: Some("Launch".to_string()),
                    status: Some("active".to_string()),
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
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-2".to_string()),
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();
        store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: Some("msg-1".to_string()),
                    from: "leader".to_string(),
                    to_worker_id: Some("worker-1".to_string()),
                    task_id: None,
                    body: "hello".to_string(),
                },
                3,
            )
            .unwrap();

        store
            .ack_message(
                TeamMessageAck {
                    team_id: "team-1".to_string(),
                    message_id: "msg-1".to_string(),
                    worker_id: Some("worker-1".to_string()),
                },
                4,
            )
            .unwrap();
        store
            .ack_message(
                TeamMessageAck {
                    team_id: "team-1".to_string(),
                    message_id: "msg-1".to_string(),
                    worker_id: Some("worker-1".to_string()),
                },
                5,
            )
            .unwrap();

        let ack_events = store
            .events(&TeamEventQuery {
                team_id: Some("team-1".to_string()),
                since_seq: None,
                limit: None,
            })
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "team.message.acked")
            .count();
        assert_eq!(ack_events, 1);
    }

    #[test]
    fn send_message_does_not_evict_pending_messages_at_cap() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: Some("surface-1".to_string()),
                    name: Some("Launch".to_string()),
                    status: Some("active".to_string()),
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
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-2".to_string()),
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();

        for index in 0..MAX_MESSAGES_PER_TEAM {
            store
                .send_message(
                    TeamMessageSend {
                        team_id: "team-1".to_string(),
                        message_id: Some(format!("msg-{index}")),
                        from: "leader".to_string(),
                        to_worker_id: Some("worker-1".to_string()),
                        task_id: None,
                        body: format!("pending {index}"),
                    },
                    10 + index as u64,
                )
                .unwrap();
        }

        let err = store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: Some("msg-over-cap".to_string()),
                    from: "leader".to_string(),
                    to_worker_id: Some("worker-1".to_string()),
                    task_id: None,
                    body: "new pending".to_string(),
                },
                10_000,
            )
            .unwrap_err();

        assert!(matches!(
            err,
            TeamError::Conflict(message) if message.contains("pending")
        ));
        assert!(store
            .get("team-1")
            .unwrap()
            .messages
            .iter()
            .any(|message| message.id == "msg-0"));
        assert_eq!(store.summary("team-1").unwrap().messages_pending, 1024);
    }

    #[test]
    fn push_event_recovers_from_stale_next_event_seq() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team-v1.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": TEAM_FORMAT_VERSION,
                "teams": [{
                    "id": "team-1",
                    "workspace_id": "workspace-1",
                    "leader_surface_id": "surface-1",
                    "name": "Launch",
                    "status": "active",
                    "created_at_ms": 1,
                    "updated_at_ms": 1,
                    "workers": [],
                    "tasks": [],
                    "messages": []
                }],
                "events": [
                    {
                        "seq": 5,
                        "team_id": "team-1",
                        "kind": "team.created",
                        "summary": "created",
                        "created_at_ms": 1
                    },
                    {
                        "seq": 6,
                        "team_id": "team-1",
                        "kind": "team.updated",
                        "summary": "updated",
                        "created_at_ms": 2
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let mut store = load_teams_from_path(&path).unwrap();
        assert_eq!(store.next_event_seq, 1);

        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("done".to_string()),
                    goal: None,
                },
                3,
            )
            .unwrap();

        assert_eq!(
            store
                .events(&TeamEventQuery {
                    team_id: None,
                    since_seq: Some(6),
                    limit: None,
                })
                .unwrap()
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![7]
        );
        save_teams_to_path(&path, &store).unwrap();
    }

    fn team_store_with_overflow_event(kind: &str) -> TeamStoreData {
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
        store.events.push(TeamEvent {
            seq: u64::MAX,
            team_id: "team-1".to_string(),
            kind: kind.to_string(),
            summary: "previous".to_string(),
            created_at_ms: 2,
        });
        store.next_event_seq = u64::MAX;
        store
    }

    #[test]
    fn worker_upsert_sequence_overflow_does_not_mutate_team() {
        let mut store = team_store_with_overflow_event("team.worker.upserted");

        let err = store
            .upsert_worker(
                TeamWorkerUpsert {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    role: Some("implementer".to_string()),
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-1".to_string()),
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                3,
            )
            .unwrap_err();

        assert!(err.to_string().contains("sequence overflow"));
        let team = store.get("team-1").unwrap();
        assert!(team.workers.is_empty());
        assert_eq!(team.updated_at_ms, 1);
    }

    #[test]
    fn task_upsert_sequence_overflow_does_not_mutate_team() {
        let mut store = team_store_with_overflow_event("team.task.upserted");

        let err = store
            .upsert_task(
                TeamTaskUpsert {
                    team_id: "team-1".to_string(),
                    task_id: "task-1".to_string(),
                    title: Some("Inspect".to_string()),
                    status: Some("done".to_string()),
                    detail: None,
                    depends_on: None,
                    assigned_worker_id: None,
                },
                3,
            )
            .unwrap_err();

        assert!(err.to_string().contains("sequence overflow"));
        let team = store.get("team-1").unwrap();
        assert!(team.tasks.is_empty());
        assert_eq!(team.updated_at_ms, 1);
    }

    #[test]
    fn message_send_sequence_overflow_does_not_mutate_team() {
        let mut store = team_store_with_overflow_event("team.message.sent");

        let err = store
            .send_message(
                TeamMessageSend {
                    team_id: "team-1".to_string(),
                    message_id: Some("msg-1".to_string()),
                    from: "leader".to_string(),
                    to_worker_id: None,
                    task_id: None,
                    body: "inspect".to_string(),
                },
                3,
            )
            .unwrap_err();

        assert!(err.to_string().contains("sequence overflow"));
        let team = store.get("team-1").unwrap();
        assert!(team.messages.is_empty());
        assert_eq!(team.updated_at_ms, 1);
    }

    #[test]
    fn load_repairs_legacy_non_increasing_event_sequences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team-v1.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": TEAM_FORMAT_VERSION,
                "teams": [{
                    "id": "team-1",
                    "workspace_id": "workspace-1",
                    "leader_surface_id": "surface-1",
                    "name": "Launch",
                    "status": "active",
                    "created_at_ms": 1,
                    "updated_at_ms": 1,
                    "workers": [],
                    "tasks": [],
                    "messages": []
                }],
                "events": [
                    {
                        "seq": 5,
                        "team_id": "team-1",
                        "kind": "team.created",
                        "summary": "created",
                        "created_at_ms": 1
                    },
                    {
                        "seq": 1,
                        "team_id": "team-1",
                        "kind": "team.updated",
                        "summary": "updated",
                        "created_at_ms": 2
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut store = load_teams_from_path(&path).unwrap();
        assert_eq!(
            store
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("done".to_string()),
                    goal: None,
                },
                3,
            )
            .unwrap();
        assert_eq!(
            store
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        save_teams_to_path(&path, &store).unwrap();
    }

    #[test]
    fn load_repairs_legacy_duplicate_message_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team-v1.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": TEAM_FORMAT_VERSION,
                "teams": [{
                    "id": "team-1",
                    "name": "Launch",
                    "status": "active",
                    "created_at_ms": 1,
                    "updated_at_ms": 1,
                    "workers": [],
                    "tasks": [],
                    "messages": [
                        {
                            "id": "msg-1",
                            "from": "leader",
                            "body": "first",
                            "created_at_ms": 1
                        },
                        {
                            "id": "msg-1",
                            "from": "leader",
                            "body": "second",
                            "created_at_ms": 2
                        }
                    ]
                }],
                "events": []
            }))
            .unwrap(),
        )
        .unwrap();

        let store = load_teams_from_path(&path).unwrap();
        let team = store.get("team-1").unwrap();
        assert_eq!(team.messages.len(), 1);
        assert_eq!(team.messages[0].body, "first");
        save_teams_to_path(&path, &store).unwrap();
    }

    #[test]
    fn load_repairs_legacy_duplicate_message_ids_without_resurrecting_terminal_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team-v1.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": TEAM_FORMAT_VERSION,
                "teams": [{
                    "id": "team-1",
                    "name": "Launch",
                    "status": "active",
                    "created_at_ms": 1,
                    "updated_at_ms": 1,
                    "workers": [{
                        "id": "worker-1",
                        "surface_id": "surface-1",
                        "status": "running"
                    }],
                    "tasks": [],
                    "messages": [
                        {
                            "id": "msg-1",
                            "from": "leader",
                            "to_worker_id": "worker-1",
                            "body": "pending duplicate",
                            "created_at_ms": 1,
                            "delivered": false
                        },
                        {
                            "id": "msg-1",
                            "from": "leader",
                            "to_worker_id": "worker-1",
                            "body": "delivered duplicate",
                            "created_at_ms": 2,
                            "delivered": true,
                            "acknowledged_at_ms": 9
                        },
                        {
                            "id": "msg-2",
                            "from": "leader",
                            "to_worker_id": "worker-1",
                            "body": "pending duplicate",
                            "created_at_ms": 3,
                            "delivered": false
                        },
                        {
                            "id": "msg-2",
                            "from": "leader",
                            "to_worker_id": "worker-1",
                            "body": "superseded duplicate",
                            "created_at_ms": 4,
                            "superseded": true,
                            "superseded_at_ms": 10
                        }
                    ]
                }],
                "events": []
            }))
            .unwrap(),
        )
        .unwrap();

        let store = load_teams_from_path(&path).unwrap();
        let team = store.get("team-1").unwrap();
        assert_eq!(team.messages.len(), 2);
        let delivered = team
            .messages
            .iter()
            .find(|message| message.id == "msg-1")
            .unwrap();
        assert!(delivered.delivered);
        assert_eq!(delivered.acknowledged_at_ms, 9);
        let superseded = team
            .messages
            .iter()
            .find(|message| message.id == "msg-2")
            .unwrap();
        assert!(superseded.superseded);
        assert_eq!(superseded.superseded_at_ms, 10);
        assert!(store
            .inbox(&TeamInboxQuery {
                team_id: "team-1".to_string(),
                worker_id: Some("worker-1".to_string()),
                include_delivered: false,
                limit: None,
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn update_teams_at_path_waits_for_external_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team-v1.json");
        let lock_path = path.with_extension("lock");
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&lock_path)
            .unwrap();
        lock_file.lock().unwrap();

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let update_path = path.clone();
        let update_thread = std::thread::spawn(move || {
            update_teams_at_path(&update_path, |store| {
                entered_tx.send(()).unwrap();
                store.upsert_team(
                    TeamUpsert {
                        team_id: "team-1".to_string(),
                        workspace_id: None,
                        leader_surface_id: None,
                        name: None,
                        status: Some("active".to_string()),
                        goal: None,
                    },
                    1,
                )
            })
        });

        assert!(
            entered_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "update must wait for the external store lock before loading and mutating"
        );
        drop(lock_file);

        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("update should enter after lock release");
        let team = update_thread.join().unwrap().unwrap();
        assert_eq!(team.id, "team-1");
    }

    #[test]
    fn summary_warns_when_active_team_has_no_open_work() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("active".to_string()),
                    goal: None,
                },
                1,
            )
            .unwrap();
        store
            .upsert_task(
                TeamTaskUpsert {
                    team_id: "team-1".to_string(),
                    task_id: "task-1".to_string(),
                    title: Some("Review".to_string()),
                    status: Some("done".to_string()),
                    detail: None,
                    depends_on: None,
                    assigned_worker_id: None,
                },
                2,
            )
            .unwrap();
        store
            .upsert_worker(
                TeamWorkerUpsert {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    role: Some("review".to_string()),
                    agent: Some("codex".to_string()),
                    surface_id: None,
                    worktree_name: None,
                    report: None,
                    status: Some("shutdown_requested".to_string()),
                    assigned_task_id: Some("task-1".to_string()),
                },
                3,
            )
            .unwrap();

        let summary = store.summary("team-1").unwrap();

        assert_eq!(summary.workers_active, 0);
        assert_eq!(summary.tasks_open, 0);
        assert_eq!(summary.messages_pending, 0);
        assert_eq!(
            summary.consistency_warnings,
            vec!["active_without_open_work".to_string()]
        );
    }

    #[test]
    fn heartbeat_active_status_clears_shutdown_request() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("active".to_string()),
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
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-1".to_string()),
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();
        store
            .request_worker_shutdown(
                TeamWorkerAction {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
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
                    assigned_task_id: None,
                },
                4,
            )
            .unwrap();

        assert_eq!(worker.status, "running");
        assert_eq!(worker.shutdown_requested_at_ms, 0);
    }

    #[test]
    fn upsert_active_status_clears_shutdown_request() {
        let mut store = TeamStoreData::default();
        store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("active".to_string()),
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
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-1".to_string()),
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                2,
            )
            .unwrap();
        store
            .request_worker_shutdown(
                TeamWorkerAction {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                },
                3,
            )
            .unwrap();

        let worker = store
            .upsert_worker(
                TeamWorkerUpsert {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    role: None,
                    agent: None,
                    surface_id: None,
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                4,
            )
            .unwrap();

        assert_eq!(worker.status, "running");
        assert_eq!(worker.shutdown_requested_at_ms, 0);
    }

    #[test]
    fn upsert_team_evicts_oldest_terminal_team_at_cap() {
        let mut store = TeamStoreData::default();
        for index in 0..MAX_TEAMS {
            store
                .upsert_team(
                    TeamUpsert {
                        team_id: format!("team-{index}"),
                        workspace_id: None,
                        leader_surface_id: None,
                        name: None,
                        status: Some("done".to_string()),
                        goal: None,
                    },
                    index as u64 + 1,
                )
                .unwrap();
        }

        let added = store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-new".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("active".to_string()),
                    goal: None,
                },
                10_000,
            )
            .unwrap();

        assert_eq!(added.id, "team-new");
        assert_eq!(store.teams.len(), MAX_TEAMS);
        assert!(store.get("team-0").is_none());
        assert!(store.get("team-new").is_some());
    }

    #[test]
    fn upsert_team_refuses_cap_when_no_terminal_team_can_be_evicted() {
        let mut store = TeamStoreData::default();
        for index in 0..MAX_TEAMS {
            store
                .upsert_team(
                    TeamUpsert {
                        team_id: format!("team-{index}"),
                        workspace_id: None,
                        leader_surface_id: None,
                        name: None,
                        status: Some("active".to_string()),
                        goal: None,
                    },
                    index as u64 + 1,
                )
                .unwrap();
        }

        let err = store
            .upsert_team(
                TeamUpsert {
                    team_id: "team-new".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("active".to_string()),
                    goal: None,
                },
                10_000,
            )
            .unwrap_err();

        assert!(err.to_string().contains("team store is full"));
        assert_eq!(store.teams.len(), MAX_TEAMS);
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
                    report: None,
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
