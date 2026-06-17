use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

pub const WORKFLOW_FORMAT_VERSION: u32 = 1;
const MAX_WORKFLOW_STORE_BYTES: u64 = 1_048_576;
const MAX_WORKFLOWS: usize = 512;
const MAX_PLAN_STEPS: usize = 128;
const MAX_EVIDENCE_ITEMS: usize = 128;
const MAX_EVENTS: usize = 4096;
const DEFAULT_QUERY_LIMIT: usize = 50;
const MAX_QUERY_LIMIT: usize = 200;
const MAX_ID_BYTES: usize = 160;
const MAX_SHORT_TEXT_BYTES: usize = 512;
const MAX_LONG_TEXT_BYTES: usize = 65_536;

static WORKFLOW_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Error, Debug)]
pub enum WorkflowError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Workflow data directory not found")]
    NoDataDir,
    #[error("Unsupported workflow format version: {0}")]
    UnsupportedVersion(u32),
    #[error("Invalid workflow data: {0}")]
    InvalidData(String),
    #[error("Workflow not found")]
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStoreData {
    #[serde(default = "default_workflow_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<WorkflowState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<WorkflowEvent>,
    #[serde(default)]
    pub next_event_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowState {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub mode: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<WorkflowPlanStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<WorkflowEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowPlanStep {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowEvidence {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowEvent {
    pub seq: u64,
    pub workflow_id: String,
    pub kind: String,
    pub at_ms: u64,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowUpsert {
    pub workflow_id: Option<String>,
    pub workspace_id: Option<String>,
    pub surface_id: Option<String>,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub mode: Option<String>,
    pub status: Option<String>,
    pub goal: Option<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowPlanStepInput {
    pub id: String,
    pub title: String,
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowEvidenceInput {
    pub id: Option<String>,
    pub kind: String,
    pub title: String,
    pub text: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowQuery {
    pub workspace_id: Option<String>,
    pub surface_id: Option<String>,
    pub session_id: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowReplayQuery {
    pub workflow_id: Option<String>,
    pub query: Option<String>,
    pub since_seq: Option<u64>,
    pub limit: Option<usize>,
}

impl Default for WorkflowStoreData {
    fn default() -> Self {
        Self {
            version: WORKFLOW_FORMAT_VERSION,
            workflows: Vec::new(),
            events: Vec::new(),
            next_event_seq: 1,
        }
    }
}

impl WorkflowStoreData {
    pub fn list(&self, query: &WorkflowQuery) -> Vec<WorkflowState> {
        let needle = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        let limit = bounded_limit(query.limit);
        let mut rows = self
            .workflows
            .iter()
            .filter(|workflow| {
                query
                    .workspace_id
                    .as_deref()
                    .is_none_or(|id| workflow.workspace_id.as_deref() == Some(id))
                    && query
                        .surface_id
                        .as_deref()
                        .is_none_or(|id| workflow.surface_id.as_deref() == Some(id))
                    && query
                        .session_id
                        .as_deref()
                        .is_none_or(|id| workflow.session_id.as_deref() == Some(id))
                    && needle
                        .as_deref()
                        .is_none_or(|needle| workflow.matches_query(needle))
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|workflow| std::cmp::Reverse(workflow.updated_at_ms));
        rows.truncate(limit);
        rows
    }

    pub fn get(&self, id: &str) -> Option<WorkflowState> {
        self.workflows
            .iter()
            .find(|workflow| workflow.id == id)
            .cloned()
    }

    pub fn upsert(
        &mut self,
        input: WorkflowUpsert,
        now_ms: u64,
    ) -> Result<WorkflowState, WorkflowError> {
        let mode = clean_optional_short("mode", input.mode.as_deref())?
            .unwrap_or_else(|| "default".to_string());
        let id = workflow_id_for(&input, &mode)?;
        let workspace_id = clean_optional_id("workspace_id", input.workspace_id.as_deref())?;
        let surface_id = clean_optional_id("surface_id", input.surface_id.as_deref())?;
        let agent = clean_optional_short("agent", input.agent.as_deref())?;
        let session_id = clean_optional_short("session_id", input.session_id.as_deref())?;
        let status = clean_optional_short("status", input.status.as_deref())?
            .unwrap_or_else(|| "active".to_string());
        let goal = clean_optional_long("goal", input.goal.as_deref())?;
        let memory = clean_optional_long("memory", input.memory.as_deref())?;

        let mut created = false;
        let index = match self.workflows.iter().position(|workflow| workflow.id == id) {
            Some(index) => index,
            None => {
                if self.workflows.len() >= MAX_WORKFLOWS {
                    return Err(WorkflowError::InvalidData(format!(
                        "workflow store is full (limit {MAX_WORKFLOWS})"
                    )));
                }
                created = true;
                self.workflows.push(WorkflowState {
                    id: id.clone(),
                    workspace_id: None,
                    surface_id: None,
                    agent: None,
                    session_id: None,
                    mode: mode.clone(),
                    status: status.clone(),
                    goal: None,
                    memory: None,
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                    plan: Vec::new(),
                    evidence: Vec::new(),
                });
                self.workflows.len() - 1
            }
        };
        let workflow = &mut self.workflows[index];
        workflow.workspace_id = workspace_id.or(workflow.workspace_id.take());
        workflow.surface_id = surface_id.or(workflow.surface_id.take());
        workflow.agent = agent.or(workflow.agent.take());
        workflow.session_id = session_id.or(workflow.session_id.take());
        workflow.mode = mode;
        workflow.status = status;
        if goal.is_some() {
            workflow.goal = goal;
        }
        if memory.is_some() {
            workflow.memory = memory;
        }
        workflow.updated_at_ms = now_ms;
        let row = workflow.clone();
        self.push_event(
            &row.id,
            if created {
                "workflow.created"
            } else {
                "workflow.updated"
            },
            format!("{} {}", row.mode, row.status),
            now_ms,
        )?;
        validate_workflow(&row)?;
        Ok(row)
    }

    pub fn set_plan(
        &mut self,
        workflow_id: &str,
        steps: Vec<WorkflowPlanStepInput>,
        now_ms: u64,
    ) -> Result<WorkflowState, WorkflowError> {
        let id = clean_id("workflow_id", workflow_id)?;
        if steps.len() > MAX_PLAN_STEPS {
            return Err(WorkflowError::InvalidData(format!(
                "plan has too many steps (limit {MAX_PLAN_STEPS})"
            )));
        }
        let mut seen = HashSet::new();
        let plan = steps
            .into_iter()
            .map(|step| {
                let step_id = clean_id("step.id", &step.id)?;
                if !seen.insert(step_id.clone()) {
                    return Err(WorkflowError::InvalidData(format!(
                        "duplicate plan step id: {step_id}"
                    )));
                }
                Ok(WorkflowPlanStep {
                    id: step_id,
                    title: clean_short("step.title", &step.title)?,
                    status: clean_short("step.status", &step.status)?,
                    detail: clean_optional_long("step.detail", step.detail.as_deref())?,
                    updated_at_ms: now_ms,
                })
            })
            .collect::<Result<Vec<_>, WorkflowError>>()?;
        let workflow = self
            .workflows
            .iter_mut()
            .find(|workflow| workflow.id == id)
            .ok_or(WorkflowError::NotFound)?;
        workflow.plan = plan;
        workflow.updated_at_ms = now_ms;
        let row = workflow.clone();
        self.push_event(
            &row.id,
            "workflow.plan.set",
            format!("{} plan step(s)", row.plan.len()),
            now_ms,
        )?;
        Ok(row)
    }

    pub fn add_evidence(
        &mut self,
        workflow_id: &str,
        input: WorkflowEvidenceInput,
        now_ms: u64,
    ) -> Result<WorkflowState, WorkflowError> {
        let id = clean_id("workflow_id", workflow_id)?;
        let workflow = self
            .workflows
            .iter_mut()
            .find(|workflow| workflow.id == id)
            .ok_or(WorkflowError::NotFound)?;
        if workflow.evidence.len() >= MAX_EVIDENCE_ITEMS {
            return Err(WorkflowError::InvalidData(format!(
                "workflow evidence is full (limit {MAX_EVIDENCE_ITEMS})"
            )));
        }
        let evidence_id = match input.id {
            Some(id) => clean_id("evidence.id", &id)?,
            None => format!("evidence-{}", workflow.evidence.len() + 1),
        };
        if workflow
            .evidence
            .iter()
            .any(|evidence| evidence.id == evidence_id)
        {
            return Err(WorkflowError::InvalidData(format!(
                "duplicate evidence id: {evidence_id}"
            )));
        }
        let evidence = WorkflowEvidence {
            id: evidence_id,
            kind: clean_short("evidence.kind", &input.kind)?,
            title: clean_short("evidence.title", &input.title)?,
            text: clean_optional_long("evidence.text", input.text.as_deref())?,
            path: input.path,
            created_at_ms: now_ms,
        };
        validate_evidence(&evidence)?;
        workflow.evidence.push(evidence.clone());
        workflow.updated_at_ms = now_ms;
        let row = workflow.clone();
        self.push_event(
            &row.id,
            "workflow.evidence.added",
            format!("{} {}", evidence.kind, evidence.title),
            now_ms,
        )?;
        Ok(row)
    }

    pub fn replay(&self, query: &WorkflowReplayQuery) -> Vec<WorkflowEvent> {
        let needle = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        let limit = bounded_limit(query.limit);
        let mut rows = self
            .events
            .iter()
            .filter(|event| {
                query
                    .workflow_id
                    .as_deref()
                    .is_none_or(|id| event.workflow_id == id)
                    && query.since_seq.is_none_or(|seq| event.seq > seq)
                    && needle.as_deref().is_none_or(|needle| {
                        event.kind.to_ascii_lowercase().contains(needle)
                            || event.summary.to_ascii_lowercase().contains(needle)
                            || event.workflow_id.to_ascii_lowercase().contains(needle)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|event| event.seq);
        rows.truncate(limit);
        rows
    }

    fn push_event(
        &mut self,
        workflow_id: &str,
        kind: &str,
        summary: String,
        now_ms: u64,
    ) -> Result<(), WorkflowError> {
        let event = WorkflowEvent {
            seq: self.next_event_seq.max(1),
            workflow_id: clean_id("event.workflow_id", workflow_id)?,
            kind: clean_short("event.kind", kind)?,
            at_ms: now_ms,
            summary: clean_short("event.summary", &summary)?,
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

impl WorkflowState {
    fn matches_query(&self, needle: &str) -> bool {
        let mut haystacks = vec![
            self.id.as_str(),
            self.mode.as_str(),
            self.status.as_str(),
            self.goal.as_deref().unwrap_or_default(),
            self.memory.as_deref().unwrap_or_default(),
            self.workspace_id.as_deref().unwrap_or_default(),
            self.surface_id.as_deref().unwrap_or_default(),
            self.agent.as_deref().unwrap_or_default(),
            self.session_id.as_deref().unwrap_or_default(),
        ];
        for step in &self.plan {
            haystacks.push(step.id.as_str());
            haystacks.push(step.title.as_str());
            haystacks.push(step.status.as_str());
            haystacks.push(step.detail.as_deref().unwrap_or_default());
        }
        for evidence in &self.evidence {
            haystacks.push(evidence.id.as_str());
            haystacks.push(evidence.kind.as_str());
            haystacks.push(evidence.title.as_str());
            haystacks.push(evidence.text.as_deref().unwrap_or_default());
        }
        haystacks
            .into_iter()
            .any(|text| text.to_ascii_lowercase().contains(needle))
    }
}

pub fn workflow_store_path() -> Result<PathBuf, WorkflowError> {
    let dir = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("forktty"))
        .ok_or(WorkflowError::NoDataDir)?;
    Ok(dir.join("workflow-v1.json"))
}

pub fn load_workflows() -> Result<WorkflowStoreData, WorkflowError> {
    load_workflows_from_path(&workflow_store_path()?)
}

pub fn load_workflows_from_path(path: &Path) -> Result<WorkflowStoreData, WorkflowError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(WorkflowError::InvalidData(format!(
                "workflow store must not be a symlink: {}",
                path.display()
            )));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(WorkflowError::InvalidData(format!(
                "workflow store must be a regular file: {}",
                path.display()
            )));
        }
        Ok(metadata) if metadata.len() > MAX_WORKFLOW_STORE_BYTES => {
            return Err(WorkflowError::InvalidData(format!(
                "workflow store is too large: {} bytes",
                metadata.len()
            )));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkflowStoreData::default());
        }
        Err(err) => return Err(err.into()),
    }
    let text = fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(WorkflowStoreData::default());
    }
    let data: WorkflowStoreData = serde_json::from_str(&text)?;
    validate_store(&data)?;
    Ok(data)
}

pub fn update_workflows<F, T>(update: F) -> Result<T, WorkflowError>
where
    F: FnOnce(&mut WorkflowStoreData) -> Result<T, WorkflowError>,
{
    update_workflows_at_path(&workflow_store_path()?, update)
}

pub fn update_workflows_at_path<F, T>(path: &Path, update: F) -> Result<T, WorkflowError>
where
    F: FnOnce(&mut WorkflowStoreData) -> Result<T, WorkflowError>,
{
    let _guard = WORKFLOW_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let mut data = load_workflows_from_path(path)?;
    let result = update(&mut data)?;
    validate_store(&data)?;
    save_workflows_to_path(path, &data)?;
    Ok(result)
}

pub fn save_workflows_to_path(path: &Path, data: &WorkflowStoreData) -> Result<(), WorkflowError> {
    validate_store(data)?;
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(WorkflowError::InvalidData(format!(
            "workflow store must not be a symlink: {}",
            path.display()
        )));
    }
    let json = serde_json::to_string_pretty(data)?;
    if json.len() as u64 > MAX_WORKFLOW_STORE_BYTES {
        return Err(WorkflowError::InvalidData(
            "workflow store is too large".to_string(),
        ));
    }
    let nonce = now_ms();
    let tmp_path = path.with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
    let result = (|| -> Result<(), WorkflowError> {
        let mut tmp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        apply_private_file_permissions(&tmp_path)?;
        tmp_file.write_all(json.as_bytes())?;
        tmp_file.sync_all()?;
        fs::rename(&tmp_path, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn default_workflow_version() -> u32 {
    WORKFLOW_FORMAT_VERSION
}

fn bounded_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT)
}

fn workflow_id_for(input: &WorkflowUpsert, mode: &str) -> Result<String, WorkflowError> {
    if let Some(id) = input.workflow_id.as_deref() {
        return clean_id("workflow_id", id);
    }
    if let Some(session_id) = input.session_id.as_deref() {
        let agent = input.agent.as_deref().unwrap_or("agent");
        return clean_id(
            "workflow_id",
            &format!(
                "session:{}:{}:{}",
                id_component(agent),
                id_component(session_id),
                id_component(mode)
            ),
        );
    }
    if let Some(surface_id) = input.surface_id.as_deref() {
        return clean_id(
            "workflow_id",
            &format!(
                "surface:{}:{}",
                id_component(surface_id),
                id_component(mode)
            ),
        );
    }
    if let Some(workspace_id) = input.workspace_id.as_deref() {
        return clean_id(
            "workflow_id",
            &format!(
                "workspace:{}:{}",
                id_component(workspace_id),
                id_component(mode)
            ),
        );
    }
    Err(WorkflowError::InvalidData(
        "workflow requires workflow_id, session_id, surface_id, or workspace_id".to_string(),
    ))
}

fn id_component(value: &str) -> String {
    let mut out = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '@') {
                ch
            } else {
                '_'
            }
        })
        .take(48)
        .collect::<String>();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

fn clean_optional_id(field: &str, value: Option<&str>) -> Result<Option<String>, WorkflowError> {
    value.map(|value| clean_id(field, value)).transpose()
}

fn clean_id(field: &str, value: &str) -> Result<String, WorkflowError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(WorkflowError::InvalidData(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > MAX_ID_BYTES {
        return Err(WorkflowError::InvalidData(format!(
            "{field} is too long (limit {MAX_ID_BYTES} bytes)"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/' | '@'))
    {
        return Err(WorkflowError::InvalidData(format!(
            "{field} may contain only ASCII letters, numbers, _, -, ., :, /, and @"
        )));
    }
    Ok(value.to_string())
}

fn clean_optional_short(field: &str, value: Option<&str>) -> Result<Option<String>, WorkflowError> {
    value.map(|value| clean_short(field, value)).transpose()
}

fn clean_short(field: &str, value: &str) -> Result<String, WorkflowError> {
    clean_text(field, value, MAX_SHORT_TEXT_BYTES, true)
}

fn clean_optional_long(field: &str, value: Option<&str>) -> Result<Option<String>, WorkflowError> {
    value
        .map(|value| clean_text(field, value, MAX_LONG_TEXT_BYTES, false))
        .transpose()
}

fn clean_text(
    field: &str,
    value: &str,
    max_bytes: usize,
    trim: bool,
) -> Result<String, WorkflowError> {
    if value.trim().is_empty() {
        return Err(WorkflowError::InvalidData(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(WorkflowError::InvalidData(format!(
            "{field} is too long (limit {max_bytes} bytes)"
        )));
    }
    if value
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err(WorkflowError::InvalidData(format!(
            "{field} contains control characters"
        )));
    }
    Ok(if trim { value.trim() } else { value }.to_string())
}

fn validate_store(data: &WorkflowStoreData) -> Result<(), WorkflowError> {
    if data.version != WORKFLOW_FORMAT_VERSION {
        return Err(WorkflowError::UnsupportedVersion(data.version));
    }
    if data.workflows.len() > MAX_WORKFLOWS {
        return Err(WorkflowError::InvalidData(format!(
            "too many workflows (limit {MAX_WORKFLOWS})"
        )));
    }
    if data.events.len() > MAX_EVENTS {
        return Err(WorkflowError::InvalidData(format!(
            "too many workflow events (limit {MAX_EVENTS})"
        )));
    }
    let mut workflow_ids = HashSet::new();
    for workflow in &data.workflows {
        validate_workflow(workflow)?;
        if !workflow_ids.insert(workflow.id.as_str()) {
            return Err(WorkflowError::InvalidData(format!(
                "duplicate workflow id: {}",
                workflow.id
            )));
        }
    }
    let mut last_seq = 0;
    for event in &data.events {
        clean_id("event.workflow_id", &event.workflow_id)?;
        clean_short("event.kind", &event.kind)?;
        clean_short("event.summary", &event.summary)?;
        if event.seq <= last_seq {
            return Err(WorkflowError::InvalidData(
                "workflow events must be sorted by increasing seq".to_string(),
            ));
        }
        last_seq = event.seq;
    }
    Ok(())
}

fn validate_workflow(workflow: &WorkflowState) -> Result<(), WorkflowError> {
    clean_id("workflow.id", &workflow.id)?;
    clean_optional_id("workflow.workspace_id", workflow.workspace_id.as_deref())?;
    clean_optional_id("workflow.surface_id", workflow.surface_id.as_deref())?;
    clean_optional_short("workflow.agent", workflow.agent.as_deref())?;
    clean_optional_short("workflow.session_id", workflow.session_id.as_deref())?;
    clean_short("workflow.mode", &workflow.mode)?;
    clean_short("workflow.status", &workflow.status)?;
    clean_optional_long("workflow.goal", workflow.goal.as_deref())?;
    clean_optional_long("workflow.memory", workflow.memory.as_deref())?;
    if workflow.plan.len() > MAX_PLAN_STEPS {
        return Err(WorkflowError::InvalidData(format!(
            "workflow has too many plan steps (limit {MAX_PLAN_STEPS})"
        )));
    }
    let mut step_ids = HashSet::new();
    for step in &workflow.plan {
        clean_id("plan.id", &step.id)?;
        clean_short("plan.title", &step.title)?;
        clean_short("plan.status", &step.status)?;
        clean_optional_long("plan.detail", step.detail.as_deref())?;
        if !step_ids.insert(step.id.as_str()) {
            return Err(WorkflowError::InvalidData(format!(
                "duplicate plan step id: {}",
                step.id
            )));
        }
    }
    if workflow.evidence.len() > MAX_EVIDENCE_ITEMS {
        return Err(WorkflowError::InvalidData(format!(
            "workflow has too many evidence items (limit {MAX_EVIDENCE_ITEMS})"
        )));
    }
    let mut evidence_ids = HashSet::new();
    for evidence in &workflow.evidence {
        validate_evidence(evidence)?;
        if !evidence_ids.insert(evidence.id.as_str()) {
            return Err(WorkflowError::InvalidData(format!(
                "duplicate evidence id: {}",
                evidence.id
            )));
        }
    }
    Ok(())
}

fn validate_evidence(evidence: &WorkflowEvidence) -> Result<(), WorkflowError> {
    clean_id("evidence.id", &evidence.id)?;
    clean_short("evidence.kind", &evidence.kind)?;
    clean_short("evidence.title", &evidence.title)?;
    clean_optional_long("evidence.text", evidence.text.as_deref())?;
    if evidence.text.is_none() && evidence.path.is_none() {
        return Err(WorkflowError::InvalidData(
            "evidence requires text or path".to_string(),
        ));
    }
    if let Some(path) = &evidence.path {
        if path.as_os_str().is_empty() {
            return Err(WorkflowError::InvalidData(
                "evidence path must not be empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn ensure_private_dir(path: &Path) -> Result<(), WorkflowError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

fn apply_private_file_permissions(path: &Path) -> Result<(), WorkflowError> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<(), WorkflowError> {
    if let Some(parent) = path.parent() {
        let dir = fs::File::open(parent)?;
        dir.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_store_upserts_plan_evidence_and_replays() {
        let mut store = WorkflowStoreData::default();
        let workflow = store
            .upsert(
                WorkflowUpsert {
                    workspace_id: Some("workspace-1".to_string()),
                    surface_id: Some("surface-1".to_string()),
                    agent: Some("codex".to_string()),
                    session_id: Some("codex-session".to_string()),
                    mode: Some("review".to_string()),
                    status: Some("running".to_string()),
                    goal: Some("Review the diff".to_string()),
                    memory: Some("Prefer socket-first work".to_string()),
                    ..WorkflowUpsert::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(workflow.id, "session:codex:codex-session:review");

        let workflow = store
            .set_plan(
                &workflow.id,
                vec![WorkflowPlanStepInput {
                    id: "inspect".to_string(),
                    title: "Inspect the diff".to_string(),
                    status: "done".to_string(),
                    detail: None,
                }],
                11,
            )
            .unwrap();
        assert_eq!(workflow.plan.len(), 1);

        let workflow = store
            .add_evidence(
                &workflow.id,
                WorkflowEvidenceInput {
                    id: Some("tests".to_string()),
                    kind: "test".to_string(),
                    title: "cargo test".to_string(),
                    text: Some("  passed\n".to_string()),
                    path: None,
                },
                12,
            )
            .unwrap();
        assert_eq!(workflow.evidence[0].id, "tests");
        assert_eq!(workflow.evidence[0].text.as_deref(), Some("  passed\n"));

        let listed = store.list(&WorkflowQuery {
            query: Some("socket-first".to_string()),
            ..WorkflowQuery::default()
        });
        assert_eq!(listed.len(), 1);

        let replay = store.replay(&WorkflowReplayQuery {
            workflow_id: Some(workflow.id),
            ..WorkflowReplayQuery::default()
        });
        assert_eq!(
            replay
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                "workflow.created",
                "workflow.plan.set",
                "workflow.evidence.added"
            ]
        );
    }

    #[test]
    fn workflow_store_rejects_shell_control_text_and_duplicate_steps() {
        let mut store = WorkflowStoreData::default();
        let workflow = store
            .upsert(
                WorkflowUpsert {
                    workflow_id: Some("workflow-1".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    ..WorkflowUpsert::default()
                },
                1,
            )
            .unwrap();
        let duplicate = store
            .set_plan(
                &workflow.id,
                vec![
                    WorkflowPlanStepInput {
                        id: "a".to_string(),
                        title: "A".to_string(),
                        status: "pending".to_string(),
                        detail: None,
                    },
                    WorkflowPlanStepInput {
                        id: "a".to_string(),
                        title: "Again".to_string(),
                        status: "pending".to_string(),
                        detail: None,
                    },
                ],
                2,
            )
            .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate plan step id"));

        let control = store
            .upsert(
                WorkflowUpsert {
                    workflow_id: Some("workflow-2".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    goal: Some("bad\u{0007}".to_string()),
                    ..WorkflowUpsert::default()
                },
                3,
            )
            .unwrap_err();
        assert!(control.to_string().contains("control characters"));
    }

    #[test]
    fn workflow_store_round_trips_to_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workflow-v1.json");
        let workflow = update_workflows_at_path(&path, |store| {
            store.upsert(
                WorkflowUpsert {
                    workflow_id: Some("workflow-1".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    goal: Some("Persist me".to_string()),
                    ..WorkflowUpsert::default()
                },
                1,
            )
        })
        .unwrap();
        assert_eq!(workflow.id, "workflow-1");
        let loaded = load_workflows_from_path(&path).unwrap();
        assert_eq!(
            loaded.get("workflow-1").unwrap().goal.as_deref(),
            Some("Persist me")
        );
        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
