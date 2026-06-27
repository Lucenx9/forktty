//! In-memory workspace, pane, surface, and per-surface runtime metadata model.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agents::{agent_metadata_aliases, AgentKind};
use crate::session::{SessionData, SESSION_FORMAT_VERSION};

pub type WorkspaceId = String;
pub type SurfaceId = String;

/// Maximum nesting depth of `Split` nodes in a pane tree. Session validation
/// rejects deeper trees on save/load, so split operations must refuse to
/// create a `Split` beyond this depth or every subsequent autosave fails.
pub const MAX_SESSION_SPLIT_DEPTH: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    #[serde(default)]
    pub active: bool,
    pub working_dir: PathBuf,
    #[serde(default)]
    pub git_branch: String,
    #[serde(default)]
    pub worktree_dir: Option<PathBuf>,
    #[serde(default)]
    pub worktree_name: Option<String>,
    pub pane_tree: PaneNode,
    pub focused_surface_id: SurfaceId,
    #[serde(default)]
    pub needs_attention: bool,
    /// Listening TCP ports of this workspace's terminal child processes.
    /// Runtime-only: recomputed each refresh, never persisted to a session.
    #[serde(default, skip_serializing)]
    pub listening_ports: Vec<u16>,
    /// Pull request linked to this workspace's branch, resolved via `gh`.
    /// Runtime-only: refreshed in the background, never persisted to a session.
    #[serde(default, skip_serializing)]
    pub pr: Option<crate::pr::PrInfo>,
}

/// What a surface renders. Defaults to `Terminal` so sessions persisted
/// before this field existed load every surface as a terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceKind {
    #[default]
    Terminal,
    Browser {
        url: String,
        #[serde(default)]
        profile: crate::profile::ProfileId,
    },
    /// The surface's shell process is `ssh <host>`.
    Ssh {
        /// Full ssh target, e.g. `user@example.com` or `[::1]`.
        host: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Surface {
    pub id: SurfaceId,
    pub workspace_id: WorkspaceId,
    pub cwd: PathBuf,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub needs_attention: bool,
    #[serde(default)]
    pub kind: SurfaceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted_scrollback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSession {
    pub agent: AgentKind,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub lifecycle: AgentSessionLifecycle,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub last_activity_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClearedAgentMetadata {
    statuses: Vec<ClearedStatusEntry>,
    progress: Vec<ProgressEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClearedAgentSession {
    agent_session: AgentSession,
    metadata: ClearedAgentMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClearedStatusEntry {
    entry: StatusEntry,
    hook: Option<StatusHookMetadata>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn normalize_persisted_scrollback(text: String) -> Option<String> {
    let text = text
        .chars()
        .filter(|ch| !ch.is_control() || matches!(*ch, '\n' | '\r' | '\t'))
        .collect::<String>();
    if text.is_empty() {
        return None;
    }
    if text.len() <= MAX_PERSISTED_SCROLLBACK_BYTES {
        return Some(text);
    }
    let mut start = text.len() - MAX_PERSISTED_SCROLLBACK_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(text[start..].to_string())
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionLifecycle {
    Running,
    Idle,
    NeedsInput,
    Suspended,
    Ended,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PaneNode {
    #[serde(rename = "leaf")]
    Leaf {
        tabs: Vec<SurfaceId>,
        #[serde(default)]
        active: usize,
    },
    #[serde(rename = "split")]
    Split {
        axis: SplitAxis,
        children: Vec<PaneNode>,
        sizes: Vec<f64>,
    },
}

impl PaneNode {
    /// Construct a leaf holding a single tab.
    pub fn single_leaf(id: SurfaceId) -> PaneNode {
        PaneNode::Leaf {
            tabs: vec![id],
            active: 0,
        }
    }

    /// Return the active surface id of this leaf, or `None` if not a leaf.
    pub fn leaf_active_id(&self) -> Option<&SurfaceId> {
        match self {
            PaneNode::Leaf { tabs, active } => tabs.get(*active),
            PaneNode::Split { .. } => None,
        }
    }

    /// Return the full tab list of this leaf, or `None` if not a leaf.
    pub fn leaf_tabs(&self) -> Option<&[SurfaceId]> {
        match self {
            PaneNode::Leaf { tabs, .. } => Some(tabs.as_slice()),
            PaneNode::Split { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovePosition {
    Before,
    After,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationItem {
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: NotificationKind,
    pub created_at_ms: u128,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default)]
    pub surface_id: Option<SurfaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_metadata: Option<TerminalNotificationMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalNotificationMetadata {
    pub id: String,
    pub report_activation: bool,
    pub report_close: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icon_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_data: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_cache_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urgency: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_after_ms: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notification_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusEntry {
    pub key: String,
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressEntry {
    pub key: String,
    pub label: String,
    pub value: f64,
    #[serde(default)]
    pub total: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEntry {
    pub id: String,
    pub timestamp_ms: u128,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusHookMetadata {
    pub event: String,
    pub order: Option<u128>,
    pub clock: Option<String>,
    pub turn_id: Option<String>,
}

impl StatusHookMetadata {
    pub fn from_order(order: u128) -> Self {
        Self {
            event: String::new(),
            order: Some(order),
            clock: None,
            turn_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Prompt,
    Error,
    Info,
    Custom,
}

#[derive(Debug, Default)]
pub struct WorkspaceModel {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    surfaces: BTreeMap<SurfaceId, Surface>,
    workspace_order: Vec<WorkspaceId>,
    notifications: Vec<NotificationItem>,
    statuses: BTreeMap<WorkspaceId, Vec<StatusEntry>>,
    status_hooks: BTreeMap<WorkspaceId, BTreeMap<String, StatusHookMetadata>>,
    progress: BTreeMap<WorkspaceId, Vec<ProgressEntry>>,
    logs: BTreeMap<WorkspaceId, Vec<LogEntry>>,
    next_workspace: u64,
    next_surface: u64,
    next_notification: u64,
    next_log: u64,
}

const MAX_LOG_ENTRIES: usize = 200;
pub const MAX_PERSISTED_SCROLLBACK_BYTES: usize = 64 * 1024;
/// Upper bound on retained notifications. Notifications accumulate for the life
/// of the process (they are never persisted), so without a cap a long-running
/// instance with a flapping agent would grow this `Vec` without bound. When the
/// cap is exceeded the oldest entries are dropped, keeping the newest.
const MAX_NOTIFICATIONS: usize = 1_000;
/// Upper bound on distinct status/progress keys retained per workspace. These
/// maps are keyed and updated in place, so they only grow when a client posts
/// new keys; without a cap a same-uid socket client could grow them without
/// bound (a slow memory-exhaustion DoS), unlike logs and notifications which
/// are already capped. When the cap is exceeded the oldest entry is dropped.
const MAX_STATUS_ENTRIES: usize = 256;
const MAX_PROGRESS_ENTRIES: usize = 256;
const HOOK_TERMINAL_PROMPT_GUARD_NS: u128 = 2_000_000_000;

fn agent_session_lifecycle_keeps_metadata(lifecycle: AgentSessionLifecycle) -> bool {
    matches!(
        lifecycle,
        AgentSessionLifecycle::Running
            | AgentSessionLifecycle::Idle
            | AgentSessionLifecycle::NeedsInput
            | AgentSessionLifecycle::Unknown
    )
}

fn agent_metadata_status_key_matches(agent: AgentKind, key: &str) -> bool {
    let Some(rest) = key.strip_prefix("agent:") else {
        return false;
    };
    agent_metadata_aliases(agent).iter().any(|alias| {
        rest == *alias
            || rest
                .strip_prefix(alias)
                .is_some_and(|suffix| suffix == ":permission")
    })
}

fn agent_metadata_progress_key_matches(agent: AgentKind, key: &str) -> bool {
    let Some(rest) = key.strip_prefix("agent:") else {
        return false;
    };
    agent_metadata_aliases(agent).iter().any(|alias| {
        rest.strip_prefix(alias)
            .is_some_and(|suffix| suffix.starts_with(':'))
    })
}

impl WorkspaceModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_workspace(
        &mut self,
        name: impl Into<String>,
        working_dir: impl Into<PathBuf>,
    ) -> Workspace {
        for workspace in self.workspaces.values_mut() {
            workspace.active = false;
        }
        let id = self.next_workspace_id();
        let surface_id = self.next_surface_id();
        let working_dir = working_dir.into();
        let surface = Surface {
            id: surface_id.clone(),
            workspace_id: id.clone(),
            cwd: working_dir.clone(),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        };
        let workspace = Workspace {
            id: id.clone(),
            name: name.into(),
            active: true,
            working_dir,
            git_branch: String::new(),
            worktree_dir: None,
            worktree_name: None,
            pane_tree: PaneNode::single_leaf(surface_id.clone()),
            focused_surface_id: surface_id,
            needs_attention: false,
            listening_ports: Vec::new(),
            pr: None,
        };
        self.surfaces.insert(surface.id.clone(), surface);
        self.workspace_order.push(id.clone());
        self.workspaces.insert(id, workspace.clone());
        workspace
    }

    pub fn create_auto_named_workspace(&mut self, working_dir: impl Into<PathBuf>) -> Workspace {
        let workspace = self.create_workspace("workspace", working_dir);
        let id = workspace.id.clone();
        self.rename_workspace(WorkspaceSelector::Id(&id), id.clone())
            .expect("workspace was just created")
    }

    pub fn create_auto_named_ssh_workspace(
        &mut self,
        working_dir: impl Into<PathBuf>,
        host: String,
    ) -> Workspace {
        let workspace = self.create_ssh_workspace("workspace", working_dir, host);
        let id = workspace.id.clone();
        self.rename_workspace(WorkspaceSelector::Id(&id), id.clone())
            .expect("workspace was just created")
    }

    pub fn create_worktree_workspace(
        &mut self,
        name: impl Into<String>,
        working_dir: impl Into<PathBuf>,
        git_branch: impl Into<String>,
        worktree_name: impl Into<String>,
    ) -> Workspace {
        let id = self.create_workspace(name, working_dir).id;
        // `create_workspace` just inserted this id, so the lookup cannot fail.
        let workspace = self
            .workspaces
            .get_mut(&id)
            .expect("workspace just created");
        workspace.git_branch = git_branch.into();
        workspace.worktree_dir = Some(workspace.working_dir.clone());
        workspace.worktree_name = Some(worktree_name.into());
        workspace.clone()
    }

    pub fn restore_session(&mut self, data: SessionData) {
        *self = WorkspaceModel::new();
        // Persisted per-surface state (kind/url). Surfaces absent here — e.g.
        // every surface in a pre-browser session — default to Terminal below.
        let mut persisted_surfaces: BTreeMap<SurfaceId, Surface> = data
            .surfaces
            .into_iter()
            .map(|surface| (surface.id.clone(), surface))
            .collect();
        let active_id = data
            .active_workspace_id
            .or_else(|| {
                data.workspaces
                    .iter()
                    .find(|workspace| workspace.active)
                    .map(|workspace| workspace.id.clone())
            })
            .or_else(|| {
                data.workspaces
                    .first()
                    .map(|workspace| workspace.id.clone())
            });

        for mut workspace in data.workspaces {
            workspace.active = active_id.as_deref() == Some(workspace.id.as_str());
            // Notifications and per-surface unread state are not persisted, so
            // clear any saved attention flag to avoid stale sidebar badges on
            // a fresh boot where no unread items actually exist.
            workspace.needs_attention = false;
            let leaf_ids = leaf_surface_ids(&workspace.pane_tree);
            if !leaf_ids.contains(&workspace.focused_surface_id) {
                if let Some(first_leaf) = leaf_ids.first() {
                    workspace.focused_surface_id = first_leaf.clone();
                }
            }
            for surface_id in leaf_ids {
                // Recover the persisted kind/title (e.g. a browser pane's url)
                // when present; otherwise fall back to a default terminal so
                // pre-browser sessions and missing entries keep working.
                let surface = match persisted_surfaces.remove(&surface_id) {
                    Some(mut persisted) => {
                        persisted.workspace_id = workspace.id.clone();
                        persisted.unread = false;
                        persisted.needs_attention = false;
                        persisted
                    }
                    None => Surface {
                        id: surface_id,
                        workspace_id: workspace.id.clone(),
                        cwd: workspace.working_dir.clone(),
                        title: String::from("shell"),
                        unread: false,
                        needs_attention: false,
                        kind: SurfaceKind::Terminal,
                        agent_session: None,
                        persisted_scrollback: None,
                    },
                };
                self.next_surface = self.next_surface.max(numeric_suffix(&surface.id));
                self.surfaces.insert(surface.id.clone(), surface);
            }
            self.next_workspace = self.next_workspace.max(numeric_suffix(&workspace.id));
            // `workspaces` is a map (dedups on id) but `workspace_order` is a
            // Vec; pushing unconditionally would list a duplicated id twice if
            // the session data ever contains two workspaces sharing an id.
            if !self.workspaces.contains_key(&workspace.id) {
                self.workspace_order.push(workspace.id.clone());
            }
            self.workspaces.insert(workspace.id.clone(), workspace);
        }
        if !self.workspaces.values().any(|workspace| workspace.active) {
            if let Some(first_id) = self.workspace_order.first() {
                if let Some(workspace) = self.workspaces.get_mut(first_id) {
                    workspace.active = true;
                }
            }
        }
        let _ = self.repair_session_invariants();
    }

    pub fn to_session_data(&self) -> SessionData {
        let mut workspaces = self.list_workspaces();
        for workspace in &mut workspaces {
            normalize_workspace_focus(workspace);
            workspace.listening_ports.clear();
            workspace.pr = None;
        }
        // Only non-terminal surfaces and terminal surfaces with restorable
        // agent metadata need to be persisted explicitly: plain terminals are
        // the default and are reconstructed from the pane tree.
        let surfaces = self
            .surfaces
            .values()
            .filter(|surface| {
                !matches!(surface.kind, SurfaceKind::Terminal)
                    || surface.agent_session.is_some()
                    || surface.persisted_scrollback.is_some()
            })
            .cloned()
            .collect();
        SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces,
            active_workspace_id: self.active_workspace_id(),
            surfaces,
        }
    }

    pub fn repair_session_invariants(&mut self) -> bool {
        let mut changed = false;
        let mut valid_surface_ids: BTreeSet<SurfaceId> = BTreeSet::new();
        let workspace_ids = self.workspace_order.clone();

        // First pass: bump monotonic counters past every id present in the
        // model, so subsequent rename allocations in the second pass cannot
        // collide with a yet-unvisited workspace.
        for workspace_id in &workspace_ids {
            if let Some(workspace) = self.workspaces.get(workspace_id) {
                self.next_workspace = self.next_workspace.max(numeric_suffix(&workspace.id));
                for surface_id in leaf_surface_ids(&workspace.pane_tree) {
                    self.next_surface = self.next_surface.max(numeric_suffix(&surface_id));
                }
            }
        }

        for workspace_id in workspace_ids {
            if !self.workspaces.contains_key(&workspace_id) {
                continue;
            }
            {
                let workspace = self
                    .workspaces
                    .get_mut(&workspace_id)
                    .expect("workspace presence verified above");
                if repair_pane_tree_structure(&mut workspace.pane_tree) {
                    changed = true;
                }
            }
            let leaf_ids = leaf_surface_ids(&self.workspaces[&workspace_id].pane_tree);
            // Detect duplicate leaf ids that already appear in this or an
            // earlier workspace and assign fresh ids before they collide in
            // the surface map.
            let mut renames: Vec<(SurfaceId, SurfaceId)> = Vec::new();
            let mut canonical_leaf_ids: Vec<SurfaceId> = Vec::with_capacity(leaf_ids.len());
            let mut workspace_leaf_ids: BTreeSet<SurfaceId> = BTreeSet::new();
            for surface_id in leaf_ids {
                if valid_surface_ids.contains(&surface_id)
                    || !workspace_leaf_ids.insert(surface_id.clone())
                {
                    let new_id = self.next_surface_id();
                    renames.push((surface_id.clone(), new_id.clone()));
                    canonical_leaf_ids.push(new_id);
                    changed = true;
                } else {
                    canonical_leaf_ids.push(surface_id);
                }
            }
            let replacement_leaf_id = if canonical_leaf_ids.is_empty() {
                changed = true;
                let new_id = self.next_surface_id();
                canonical_leaf_ids.push(new_id.clone());
                Some(new_id)
            } else {
                None
            };

            {
                let workspace = self
                    .workspaces
                    .get_mut(&workspace_id)
                    .expect("workspace presence verified above");
                for (old_id, new_id) in &renames {
                    rename_leaf(&mut workspace.pane_tree, old_id, new_id);
                    if workspace.focused_surface_id == *old_id {
                        workspace.focused_surface_id = new_id.clone();
                    }
                }
                if let Some(replacement_leaf_id) = replacement_leaf_id {
                    workspace.pane_tree = PaneNode::single_leaf(replacement_leaf_id.clone());
                    workspace.focused_surface_id = replacement_leaf_id;
                } else if !canonical_leaf_ids.contains(&workspace.focused_surface_id) {
                    if let Some(first_leaf) = canonical_leaf_ids.first() {
                        workspace.focused_surface_id = first_leaf.clone();
                        changed = true;
                    }
                }
            }

            let (workspace_id_owned, working_dir) = {
                let workspace = &self.workspaces[&workspace_id];
                (workspace.id.clone(), workspace.working_dir.clone())
            };
            for surface_id in canonical_leaf_ids {
                match self.surfaces.get_mut(&surface_id) {
                    Some(existing) => {
                        valid_surface_ids.insert(surface_id);
                        if existing.workspace_id != workspace_id_owned {
                            existing.workspace_id = workspace_id_owned.clone();
                            changed = true;
                        }
                    }
                    None => {
                        valid_surface_ids.insert(surface_id.clone());
                        self.surfaces.insert(
                            surface_id.clone(),
                            Surface {
                                id: surface_id,
                                workspace_id: workspace_id_owned.clone(),
                                cwd: working_dir.clone(),
                                title: String::from("shell"),
                                unread: false,
                                needs_attention: false,
                                kind: SurfaceKind::Terminal,
                                agent_session: None,
                                persisted_scrollback: None,
                            },
                        );
                        changed = true;
                    }
                }
            }
        }

        let before = self.surfaces.len();
        self.surfaces.retain(|surface_id, surface| {
            valid_surface_ids.contains(surface_id)
                && self.workspaces.contains_key(&surface.workspace_id)
        });
        changed || self.surfaces.len() != before
    }

    pub fn select_workspace(&mut self, selector: WorkspaceSelector<'_>) -> Option<Workspace> {
        let id = self.workspace_id_for(selector)?;
        for workspace in self.workspaces.values_mut() {
            workspace.active = workspace.id == id;
        }
        self.workspaces.get(&id).cloned()
    }

    pub fn rename_workspace(
        &mut self,
        selector: WorkspaceSelector<'_>,
        name: impl Into<String>,
    ) -> Option<Workspace> {
        let id = self.workspace_id_for(selector)?;
        let workspace = self.workspaces.get_mut(&id)?;
        workspace.name = name.into();
        Some(workspace.clone())
    }

    pub fn move_workspace(
        &mut self,
        source_id: &str,
        target_id: &str,
        position: MovePosition,
    ) -> bool {
        if source_id == target_id {
            return false;
        }
        let Some(source_index) = self.workspace_order.iter().position(|id| id == source_id) else {
            return false;
        };
        if !self.workspace_order.iter().any(|id| id == target_id) {
            return false;
        }
        let previous = self.workspace_order.clone();
        let source = self.workspace_order.remove(source_index);
        let Some(mut target_index) = self.workspace_order.iter().position(|id| id == target_id)
        else {
            self.workspace_order.insert(source_index, source);
            return false;
        };
        if position == MovePosition::After {
            target_index += 1;
        }
        let target_index = target_index.min(self.workspace_order.len());
        self.workspace_order.insert(target_index, source);
        previous != self.workspace_order
    }

    pub fn close_workspace(&mut self, selector: WorkspaceSelector<'_>) -> Option<Workspace> {
        let id = self.workspace_id_for(selector)?;
        let removed = self.workspaces.remove(&id)?;
        self.workspace_order.retain(|candidate| candidate != &id);
        self.surfaces
            .retain(|_, surface| surface.workspace_id != removed.id);
        self.statuses.remove(&removed.id);
        self.status_hooks.remove(&removed.id);
        self.progress.remove(&removed.id);
        self.logs.remove(&removed.id);
        if removed.active {
            if let Some(next_id) = self.workspace_order.first().cloned() {
                if let Some(next) = self.workspaces.get_mut(&next_id) {
                    next.active = true;
                }
            }
        }
        Some(removed)
    }

    pub fn list_workspaces(&self) -> Vec<Workspace> {
        self.workspace_order
            .iter()
            .filter_map(|id| self.workspaces.get(id).cloned())
            .collect()
    }

    pub fn active_workspace_id(&self) -> Option<WorkspaceId> {
        self.workspace_order
            .iter()
            .find(|id| {
                self.workspaces
                    .get(*id)
                    .map(|workspace| workspace.active)
                    .unwrap_or(false)
            })
            .cloned()
            .or_else(|| self.workspace_order.first().cloned())
    }

    pub fn active_workspace(&self) -> Option<Workspace> {
        self.active_workspace_id()
            .and_then(|id| self.workspaces.get(&id).cloned())
    }

    pub fn list_surfaces(&self, workspace_id: Option<&str>) -> Vec<Surface> {
        self.surfaces
            .values()
            .filter(|surface| {
                workspace_id
                    .map(|id| surface.workspace_id == id)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    pub fn surface(&self, id: &str) -> Option<&Surface> {
        self.surfaces.get(id)
    }

    pub fn split_surface(&mut self, surface_id: &str, axis: SplitAxis) -> Option<Surface> {
        self.split_with(
            surface_id,
            axis,
            SurfaceKind::Terminal,
            String::from("shell"),
        )
    }

    /// Split the workspace's focused surface into a new browser pane.
    pub fn open_browser(
        &mut self,
        workspace_id: &str,
        url: &str,
        profile: crate::profile::ProfileId,
        axis: SplitAxis,
    ) -> Option<Surface> {
        let focused = self
            .workspaces
            .get(workspace_id)?
            .focused_surface_id
            .clone();
        let title = browser_title_for(url);
        self.split_with(
            &focused,
            axis,
            SurfaceKind::Browser {
                url: url.to_string(),
                profile,
            },
            title,
        )
    }

    /// Split the workspace's focused surface into a new SSH pane.
    ///
    /// The caller is responsible for spawning the underlying process with
    /// `shell = ssh_binary` and `args = [host]`.
    pub fn open_ssh(
        &mut self,
        workspace_id: &str,
        host: String,
        axis: SplitAxis,
    ) -> Option<Surface> {
        let focused = self
            .workspaces
            .get(workspace_id)?
            .focused_surface_id
            .clone();
        let title = format!("ssh:{host}");
        self.split_with(&focused, axis, SurfaceKind::Ssh { host }, title)
    }

    /// Create a new workspace whose first (and only) surface is an SSH surface.
    ///
    /// This mirrors `create_workspace` but produces a `SurfaceKind::Ssh`
    /// surface instead of `SurfaceKind::Terminal`.
    pub fn create_ssh_workspace(
        &mut self,
        name: impl Into<String>,
        working_dir: impl Into<PathBuf>,
        host: String,
    ) -> Workspace {
        for workspace in self.workspaces.values_mut() {
            workspace.active = false;
        }
        let id = self.next_workspace_id();
        let surface_id = self.next_surface_id();
        let working_dir = working_dir.into();
        let title = format!("ssh:{host}");
        let surface = Surface {
            id: surface_id.clone(),
            workspace_id: id.clone(),
            cwd: working_dir.clone(),
            title,
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Ssh { host },
            agent_session: None,
            persisted_scrollback: None,
        };
        let workspace = Workspace {
            id: id.clone(),
            name: name.into(),
            active: true,
            working_dir,
            git_branch: String::new(),
            worktree_dir: None,
            worktree_name: None,
            pane_tree: PaneNode::single_leaf(surface_id.clone()),
            focused_surface_id: surface_id,
            needs_attention: false,
            listening_ports: Vec::new(),
            pr: None,
        };
        self.surfaces.insert(surface.id.clone(), surface);
        self.workspace_order.push(id.clone());
        self.workspaces.insert(id, workspace.clone());
        workspace
    }

    fn split_with(
        &mut self,
        surface_id: &str,
        axis: SplitAxis,
        kind: SurfaceKind,
        title: String,
    ) -> Option<Surface> {
        let source = self.surfaces.get(surface_id)?.clone();
        // Pre-validate the workspace still owns this surface in its pane tree
        // before allocating an id, so failure paths don't leak monotonic ids.
        let workspace_ref = self.workspaces.get(&source.workspace_id)?;
        if !has_leaf_surface_id(&workspace_ref.pane_tree, surface_id) {
            return None;
        }
        // Refuse splits that would nest a `Split` deeper than session
        // validation accepts; otherwise every subsequent autosave fails.
        if split_would_exceed_depth(&workspace_ref.pane_tree, surface_id, axis) {
            return None;
        }
        let new_id = self.next_surface_id();
        let new_surface = Surface {
            id: new_id.clone(),
            workspace_id: source.workspace_id.clone(),
            cwd: source.cwd.clone(),
            title,
            unread: false,
            needs_attention: false,
            kind,
            agent_session: None,
            persisted_scrollback: None,
        };
        let workspace = self
            .workspaces
            .get_mut(&source.workspace_id)
            .expect("workspace existence verified above");
        let inserted = replace_leaf_with_split(
            &mut workspace.pane_tree,
            surface_id,
            axis,
            PaneNode::single_leaf(new_id.clone()),
        );
        debug_assert!(inserted, "leaf existence pre-validated");
        if !inserted {
            return None;
        }
        workspace.focused_surface_id = new_id;
        self.surfaces
            .insert(new_surface.id.clone(), new_surface.clone());
        Some(new_surface)
    }

    pub fn update_split_partition_ratio(
        &mut self,
        workspace_id: &str,
        left_leaves: &[SurfaceId],
        right_leaves: &[SurfaceId],
        ratio: f64,
    ) -> bool {
        if left_leaves.is_empty() || right_leaves.is_empty() || !ratio.is_finite() {
            return false;
        }
        let ratio = ratio.clamp(0.01, 0.99);
        let Some(workspace) = self.workspaces.get_mut(workspace_id) else {
            return false;
        };
        update_partition_ratio(&mut workspace.pane_tree, left_leaves, right_leaves, ratio)
    }

    pub fn focus_surface(&mut self, surface_id: &str) -> bool {
        let Some(surface) = self.surfaces.get(surface_id) else {
            return false;
        };
        let Some(workspace) = self.workspaces.get_mut(&surface.workspace_id) else {
            return false;
        };
        if !has_leaf_surface_id(&workspace.pane_tree, surface_id) {
            return false;
        }
        workspace.focused_surface_id = surface_id.to_string();
        // If the surface is a non-active tab in its leaf, update active.
        set_leaf_active_for_surface(&mut workspace.pane_tree, surface_id);
        true
    }

    pub fn focus_surface_and_select_workspace(&mut self, surface_id: &str) -> bool {
        let Some(workspace_id) = self
            .surfaces
            .get(surface_id)
            .map(|surface| surface.workspace_id.clone())
        else {
            return false;
        };
        if !self.focus_surface(surface_id) {
            return false;
        }
        for workspace in self.workspaces.values_mut() {
            workspace.active = workspace.id == workspace_id;
        }
        true
    }

    /// Add a new terminal tab to the pane whose tabs contain `near_surface_id`.
    /// The new tab becomes the active tab of that pane and the workspace focus.
    /// Returns the newly created Surface on success.
    pub fn add_tab(&mut self, near_surface_id: &str) -> Option<Surface> {
        let source = self.surfaces.get(near_surface_id)?.clone();
        let workspace_id = source.workspace_id.clone();
        // Verify the surface lives in a leaf of its workspace.
        if !has_leaf_surface_id(
            &self.workspaces.get(&workspace_id)?.pane_tree,
            near_surface_id,
        ) {
            return None;
        }
        let new_id = self.next_surface_id();
        let new_surface = Surface {
            id: new_id.clone(),
            workspace_id: workspace_id.clone(),
            cwd: source.cwd.clone(),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        };
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .expect("workspace verified above");
        if !push_tab_to_leaf(&mut workspace.pane_tree, near_surface_id, new_id.clone()) {
            return None;
        }
        workspace.focused_surface_id = new_id.clone();
        self.surfaces
            .insert(new_surface.id.clone(), new_surface.clone());
        Some(new_surface)
    }

    /// Select an existing tab in any leaf of the owning workspace.
    /// Sets the leaf's `active` index and the workspace `focused_surface_id`.
    /// Returns `true` if the surface was found and activated.
    pub fn select_tab(&mut self, surface_id: &str) -> bool {
        let Some(surface) = self.surfaces.get(surface_id) else {
            return false;
        };
        let workspace_id = surface.workspace_id.clone();
        let Some(workspace) = self.workspaces.get_mut(&workspace_id) else {
            return false;
        };
        if !set_leaf_active_for_surface(&mut workspace.pane_tree, surface_id) {
            return false;
        }
        workspace.focused_surface_id = surface_id.to_string();
        true
    }

    pub fn move_tab(
        &mut self,
        source_surface_id: &str,
        target_surface_id: &str,
        position: MovePosition,
    ) -> bool {
        if source_surface_id == target_surface_id {
            return false;
        }
        let Some(source) = self.surfaces.get(source_surface_id) else {
            return false;
        };
        let Some(target) = self.surfaces.get(target_surface_id) else {
            return false;
        };
        if source.workspace_id != target.workspace_id {
            return false;
        }
        let Some(workspace) = self.workspaces.get_mut(&source.workspace_id) else {
            return false;
        };
        move_tab_in_tree(
            &mut workspace.pane_tree,
            source_surface_id,
            target_surface_id,
            position,
        )
    }

    pub fn swap_panes(&mut self, source_surface_id: &str, target_surface_id: &str) -> bool {
        if source_surface_id == target_surface_id {
            return false;
        }
        let Some(source) = self.surfaces.get(source_surface_id) else {
            return false;
        };
        let Some(target) = self.surfaces.get(target_surface_id) else {
            return false;
        };
        if source.workspace_id != target.workspace_id {
            return false;
        }
        let Some(workspace) = self.workspaces.get_mut(&source.workspace_id) else {
            return false;
        };
        swap_pane_leaves(
            &mut workspace.pane_tree,
            source_surface_id,
            target_surface_id,
        )
    }

    /// Update a browser surface's URL. Same-URL navigation is a successful no-op.
    /// Returns false for terminals, SSH surfaces, or missing ids.
    pub fn set_surface_url(&mut self, surface_id: &str, url: &str) -> bool {
        let Some(url) = validated_committed_browser_url(url) else {
            return false;
        };
        match self.surfaces.get_mut(surface_id) {
            Some(surface) => match &mut surface.kind {
                SurfaceKind::Browser { url: current, .. } => {
                    if current == &url {
                        return true;
                    }
                    *current = url;
                    surface.title = browser_title_for(current);
                    true
                }
                SurfaceKind::Terminal | SurfaceKind::Ssh { .. } => false,
            },
            None => false,
        }
    }

    pub fn set_surface_agent_session(
        &mut self,
        surface_id: &str,
        agent: AgentKind,
        session_id: impl Into<String>,
    ) -> bool {
        let session_id = session_id.into();
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return false;
        }
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        let resume_cwd = surface
            .agent_session
            .as_ref()
            .filter(|session| session.agent == agent && session.session_id == session_id)
            .and_then(|session| session.resume_cwd.clone());
        let permission_mode = surface
            .agent_session
            .as_ref()
            .filter(|session| session.agent == agent && session.session_id == session_id)
            .and_then(|session| session.permission_mode.clone());
        surface.agent_session = Some(AgentSession {
            agent,
            session_id: session_id.to_string(),
            resume_cwd,
            permission_mode,
            lifecycle: AgentSessionLifecycle::Running,
            last_activity_ms: 0,
        });
        true
    }

    pub fn clear_surface_agent_session(&mut self, surface_id: &str) -> Option<AgentSession> {
        self.clear_surface_agent_session_with_metadata(surface_id)
            .map(|cleared| cleared.agent_session)
    }

    pub fn clear_surface_agent_session_with_metadata(
        &mut self,
        surface_id: &str,
    ) -> Option<ClearedAgentSession> {
        let (workspace_id, agent_session) = {
            let surface = self.surfaces.get_mut(surface_id)?;
            let workspace_id = surface.workspace_id.clone();
            let agent_session = surface.agent_session.take()?;
            (workspace_id, agent_session)
        };
        let metadata =
            self.clear_agent_metadata_if_no_active_session(&workspace_id, agent_session.agent);
        Some(ClearedAgentSession {
            agent_session,
            metadata,
        })
    }

    pub fn restore_surface_agent_session(
        &mut self,
        surface_id: &str,
        mut agent_session: AgentSession,
    ) -> bool {
        let session_id = agent_session.session_id.trim();
        if session_id.is_empty() {
            return false;
        }
        agent_session.session_id = session_id.to_string();

        if agent_session
            .resume_cwd
            .as_ref()
            .is_some_and(|cwd| cwd.as_os_str().is_empty() || !cwd.is_absolute())
        {
            return false;
        }

        if let Some(permission_mode) = agent_session.permission_mode.take() {
            let permission_mode = permission_mode.trim();
            if permission_mode.is_empty()
                || permission_mode.len() > 64
                || permission_mode.chars().any(char::is_control)
            {
                return false;
            }
            agent_session.permission_mode = Some(permission_mode.to_string());
        }

        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        if surface.agent_session.is_some() {
            return false;
        }
        surface.agent_session = Some(agent_session);
        true
    }

    pub fn restore_surface_agent_session_with_metadata(
        &mut self,
        surface_id: &str,
        cleared: ClearedAgentSession,
    ) -> bool {
        let Some(workspace_id) = self
            .surfaces
            .get(surface_id)
            .map(|surface| surface.workspace_id.clone())
        else {
            return false;
        };
        if !self.restore_surface_agent_session(surface_id, cleared.agent_session) {
            return false;
        }
        self.restore_agent_metadata(&workspace_id, cleared.metadata)
    }

    pub fn set_surface_agent_session_resume_cwd(
        &mut self,
        surface_id: &str,
        resume_cwd: PathBuf,
    ) -> bool {
        if resume_cwd.as_os_str().is_empty() || !resume_cwd.is_absolute() {
            return false;
        }
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        let Some(agent_session) = surface.agent_session.as_mut() else {
            return false;
        };
        agent_session.resume_cwd = Some(resume_cwd);
        true
    }

    pub fn set_surface_agent_session_permission_mode(
        &mut self,
        surface_id: &str,
        permission_mode: impl Into<String>,
    ) -> bool {
        let permission_mode = permission_mode.into();
        let permission_mode = permission_mode.trim();
        if permission_mode.is_empty()
            || permission_mode.len() > 64
            || permission_mode.chars().any(char::is_control)
        {
            return false;
        }
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        let Some(agent_session) = surface.agent_session.as_mut() else {
            return false;
        };
        agent_session.permission_mode = Some(permission_mode.to_string());
        true
    }

    pub fn set_surface_agent_session_lifecycle(
        &mut self,
        surface_id: &str,
        lifecycle: AgentSessionLifecycle,
    ) -> bool {
        self.set_surface_agent_session_lifecycle_with_cleared_metadata(surface_id, lifecycle)
            .is_some()
    }

    pub fn set_surface_agent_session_lifecycle_with_cleared_metadata(
        &mut self,
        surface_id: &str,
        lifecycle: AgentSessionLifecycle,
    ) -> Option<ClearedAgentMetadata> {
        let (workspace_id, agent) = {
            let surface = self.surfaces.get_mut(surface_id)?;
            let agent_session = surface.agent_session.as_mut()?;
            agent_session.lifecycle = lifecycle;
            (surface.workspace_id.clone(), agent_session.agent)
        };
        Some(if agent_session_lifecycle_keeps_metadata(lifecycle) {
            ClearedAgentMetadata::default()
        } else {
            self.clear_agent_metadata_if_no_active_session(&workspace_id, agent)
        })
    }

    pub fn set_surface_agent_session_last_activity_ms(
        &mut self,
        surface_id: &str,
        last_activity_ms: u64,
    ) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        let Some(agent_session) = surface.agent_session.as_mut() else {
            return false;
        };
        agent_session.last_activity_ms = last_activity_ms;
        true
    }

    pub fn prepare_root_surface_replacement(&mut self, surface_id: &str) -> Option<Surface> {
        let surface = self.surfaces.get(surface_id)?.clone();
        let (workspace_id, working_dir) = {
            let workspace = self.workspaces.get(&surface.workspace_id)?;
            // Only allow replacement for the root leaf that has exactly one tab
            // equal to surface_id. Multi-tab leaves are handled by close_surface.
            let is_sole_root_tab = matches!(
                &workspace.pane_tree,
                PaneNode::Leaf { tabs, .. } if tabs.len() == 1 && tabs[0] == surface_id
            );
            if !is_sole_root_tab {
                return None;
            }
            (workspace.id.clone(), workspace.working_dir.clone())
        };
        let replacement_id = self.next_surface_id();
        Some(Surface {
            id: replacement_id,
            workspace_id,
            cwd: working_dir,
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        })
    }

    pub fn close_surface(&mut self, surface_id: &str) -> Option<Surface> {
        self.close_surface_with_replacement(surface_id, None)
    }

    pub fn close_surface_with_replacement(
        &mut self,
        surface_id: &str,
        prepared_replacement: Option<Surface>,
    ) -> Option<Surface> {
        let surface = self.surfaces.get(surface_id)?.clone();
        let workspace_id = surface.workspace_id.clone();
        let working_dir = self.workspaces.get(&workspace_id)?.working_dir.clone();

        // Check if this surface is one of multiple tabs in its leaf.
        // If so, just remove it from the tab list without collapsing the leaf.
        if let Some(workspace) = self.workspaces.get_mut(&workspace_id) {
            let closing_focused_surface = workspace.focused_surface_id == surface_id;
            if let Some(new_active) = remove_tab_from_leaf(&mut workspace.pane_tree, surface_id) {
                if closing_focused_surface {
                    workspace.focused_surface_id = new_active;
                } else if !has_leaf_surface_id(&workspace.pane_tree, &workspace.focused_surface_id)
                {
                    if let Some(first) = first_leaf_surface_id(&workspace.pane_tree) {
                        workspace.focused_surface_id = first;
                    }
                }
                let removed = self.surfaces.remove(surface_id)?;
                self.clear_removed_surface_metadata(&removed);
                self.recompute_workspace_attention(&workspace_id);
                return Some(removed);
            }
        }

        // The surface is the last (only) tab in its leaf: use the existing
        // leaf-collapse / replacement logic.
        let replacement = match prepared_replacement {
            Some(replacement)
                if replacement.workspace_id == workspace_id
                    && !self.surfaces.contains_key(&replacement.id) =>
            {
                replacement
            }
            _ => {
                let replacement_id = self.next_surface_id();
                Surface {
                    id: replacement_id,
                    workspace_id: workspace_id.clone(),
                    cwd: working_dir,
                    title: String::from("shell"),
                    unread: false,
                    needs_attention: false,
                    kind: SurfaceKind::Terminal,
                    agent_session: None,
                    persisted_scrollback: None,
                }
            }
        };
        let workspace = self.workspaces.get_mut(&workspace_id)?;
        let closing_focused_surface = workspace.focused_surface_id == surface_id;
        let neighbor_focus = neighbor_leaf_surface_id(&workspace.pane_tree, surface_id);
        let removed_root = remove_leaf(&mut workspace.pane_tree, surface_id)?;
        let removed = self.surfaces.remove(surface_id)?;
        if removed_root {
            workspace.pane_tree = PaneNode::single_leaf(replacement.id.clone());
            workspace.focused_surface_id = replacement.id.clone();
            self.surfaces.insert(replacement.id.clone(), replacement);
        } else if closing_focused_surface {
            // Move focus to the surviving sibling next to the closed pane
            // rather than teleporting to the first pane of the workspace.
            if let Some(next_focus) =
                neighbor_focus.or_else(|| first_leaf_surface_id(&workspace.pane_tree))
            {
                workspace.focused_surface_id = next_focus;
            }
        } else if !has_leaf_surface_id(&workspace.pane_tree, &workspace.focused_surface_id) {
            if let Some(first) = first_leaf_surface_id(&workspace.pane_tree) {
                workspace.focused_surface_id = first;
            }
        }
        self.clear_removed_surface_metadata(&removed);
        self.recompute_workspace_attention(&workspace_id);
        Some(removed)
    }

    pub fn mark_surface_unread(&mut self, surface_id: &str, unread: bool) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        surface.unread = unread;
        surface.needs_attention = unread;
        let workspace_id = surface.workspace_id.clone();
        self.recompute_workspace_attention(&workspace_id);
        true
    }

    pub fn set_surface_title(&mut self, surface_id: &str, title: impl Into<String>) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        surface.title = title.into();
        true
    }

    pub fn set_surface_cwd(&mut self, surface_id: &str, cwd: PathBuf) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        surface.cwd = cwd;
        true
    }

    pub fn set_surface_persisted_scrollback(
        &mut self,
        surface_id: &str,
        text: Option<String>,
    ) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        surface.persisted_scrollback = text.and_then(normalize_persisted_scrollback);
        true
    }

    /// Replace a workspace's listening-port hint. Returns `true` when the set of
    /// ports actually changed, so callers can skip redundant UI refreshes.
    pub fn set_listening_ports(&mut self, workspace_id: &str, mut ports: Vec<u16>) -> bool {
        let Some(workspace) = self.workspaces.get_mut(workspace_id) else {
            return false;
        };
        ports.sort_unstable();
        ports.dedup();
        if workspace.listening_ports == ports {
            return false;
        }
        workspace.listening_ports = ports;
        true
    }

    /// Replace a workspace's linked-PR hint. Returns `true` when it changed, so
    /// callers can skip redundant UI refreshes.
    pub fn set_pr(&mut self, workspace_id: &str, pr: Option<crate::pr::PrInfo>) -> bool {
        let Some(workspace) = self.workspaces.get_mut(workspace_id) else {
            return false;
        };
        if workspace.pr == pr {
            return false;
        }
        workspace.pr = pr;
        true
    }

    pub fn create_notification(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
        workspace_id: Option<WorkspaceId>,
        surface_id: Option<SurfaceId>,
    ) -> NotificationItem {
        let item = NotificationItem {
            id: self.next_notification_id(),
            title: title.into(),
            body: body.into(),
            kind,
            created_at_ms: now_ms(),
            read: false,
            workspace_id,
            surface_id,
            terminal_metadata: None,
        };
        self.mark_notification_target_unread(
            item.workspace_id.as_deref(),
            item.surface_id.as_deref(),
        );
        self.notifications.push(item.clone());
        if self.notifications.len() > MAX_NOTIFICATIONS {
            let overflow = self.notifications.len() - MAX_NOTIFICATIONS;
            self.notifications.drain(0..overflow);
        }
        item
    }

    pub fn update_notification(
        &mut self,
        notification_id: &str,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
    ) -> Option<NotificationItem> {
        let index = self
            .notifications
            .iter()
            .position(|notification| notification.id == notification_id)?;
        {
            let notification = &mut self.notifications[index];
            notification.title = title.into();
            notification.body = body.into();
            notification.kind = kind;
            notification.created_at_ms = now_ms().max(notification.created_at_ms.saturating_add(1));
            notification.read = false;
        }
        let item = self.notifications[index].clone();
        self.mark_notification_target_unread(
            item.workspace_id.as_deref(),
            item.surface_id.as_deref(),
        );
        Some(item)
    }

    pub fn list_notifications(&self) -> Vec<NotificationItem> {
        self.notifications.clone()
    }

    pub fn set_notification_terminal_metadata(
        &mut self,
        notification_id: &str,
        metadata: Option<TerminalNotificationMetadata>,
    ) -> Option<NotificationItem> {
        let notification = self
            .notifications
            .iter_mut()
            .find(|notification| notification.id == notification_id)?;
        notification.terminal_metadata = metadata;
        Some(notification.clone())
    }

    pub fn unread_notification_count(&self) -> usize {
        self.notifications
            .iter()
            .filter(|notification| !notification.read)
            .count()
    }

    pub fn mark_notifications_read(&mut self) {
        for notification in &mut self.notifications {
            notification.read = true;
        }
        self.recompute_notification_attention();
    }

    pub fn dismiss_notification(&mut self, notification_id: &str) -> bool {
        let before = self.notifications.len();
        self.notifications
            .retain(|notification| notification.id != notification_id);
        let removed = self.notifications.len() != before;
        if removed {
            self.recompute_notification_attention();
        }
        removed
    }

    pub fn clear_notifications(&mut self) {
        self.notifications.clear();
        self.recompute_notification_attention();
    }

    pub fn set_status(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
    ) -> Option<StatusEntry> {
        self.set_status_ordered(workspace_id, key, label, value, color, None)
    }

    pub fn set_status_ordered(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        order: Option<u128>,
    ) -> Option<StatusEntry> {
        self.set_status_with_hook_metadata(
            workspace_id,
            key,
            label,
            value,
            color,
            order.map(StatusHookMetadata::from_order),
        )
    }

    pub fn set_status_with_hook_metadata(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        hook: Option<StatusHookMetadata>,
    ) -> Option<StatusEntry> {
        self.set_status_with_hook_metadata_applied(workspace_id, key, label, value, color, hook)
            .map(|(entry, _applied)| entry)
    }

    pub fn set_status_with_hook_metadata_applied(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        hook: Option<StatusHookMetadata>,
    ) -> Option<(StatusEntry, bool)> {
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let entry = StatusEntry {
            key: key.into(),
            label: label.into(),
            value: value.into(),
            color,
        };
        if let Some(hook) = hook {
            let current_hook = self
                .status_hooks
                .get(workspace_id)
                .and_then(|hooks| hooks.get(&entry.key));
            if should_ignore_hook_status(current_hook, &hook) {
                return self
                    .statuses
                    .get(workspace_id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|existing| existing.key == entry.key)
                            .cloned()
                    })
                    .or(Some(entry))
                    .map(|entry| (entry, false));
            }
            let next_hook = merge_hook_metadata(current_hook, hook);
            self.status_hooks
                .entry(workspace_id.to_string())
                .or_default()
                .insert(entry.key.clone(), next_hook);
        } else if let Some(hooks) = self.status_hooks.get_mut(workspace_id) {
            hooks.remove(&entry.key);
            let remove_workspace_hooks = hooks.is_empty();
            if remove_workspace_hooks {
                self.status_hooks.remove(workspace_id);
            }
        }
        let entries = self.statuses.entry(workspace_id.to_string()).or_default();
        let evicted_key = if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            *existing = entry.clone();
            None
        } else {
            // A new key grows the map; cap it so a misbehaving client cannot
            // exhaust memory. Drop the oldest entry when at capacity.
            let evicted = (entries.len() >= MAX_STATUS_ENTRIES).then(|| entries.remove(0).key);
            entries.push(entry.clone());
            evicted
        };
        // Keep the parallel hook map from outliving its evicted status entry.
        if let Some(evicted_key) = evicted_key {
            if let Some(hooks) = self.status_hooks.get_mut(workspace_id) {
                hooks.remove(&evicted_key);
                if hooks.is_empty() {
                    self.status_hooks.remove(workspace_id);
                }
            }
        }
        Some((entry, true))
    }

    pub fn list_status(&self, workspace_id: &str) -> Vec<StatusEntry> {
        self.statuses.get(workspace_id).cloned().unwrap_or_default()
    }

    pub fn list_status_limited(&self, workspace_id: &str, limit: usize) -> Vec<StatusEntry> {
        self.statuses
            .get(workspace_id)
            .map(|entries| entries[entries.len().saturating_sub(limit)..].to_vec())
            .unwrap_or_default()
    }

    pub fn clear_status(&mut self, workspace_id: &str, key: Option<&str>) -> bool {
        self.clear_status_ordered(workspace_id, key, None)
    }

    pub fn clear_status_ordered(
        &mut self,
        workspace_id: &str,
        key: Option<&str>,
        order: Option<u128>,
    ) -> bool {
        self.clear_status_with_hook_metadata(
            workspace_id,
            key,
            order.map(StatusHookMetadata::from_order),
        )
    }

    pub fn clear_status_with_hook_metadata(
        &mut self,
        workspace_id: &str,
        key: Option<&str>,
        hook: Option<StatusHookMetadata>,
    ) -> bool {
        if !self.workspaces.contains_key(workspace_id) {
            return false;
        }
        match (key, hook) {
            (Some(key), Some(hook)) => {
                let current_hook = self
                    .status_hooks
                    .get(workspace_id)
                    .and_then(|hooks| hooks.get(key));
                if should_ignore_hook_status(current_hook, &hook) {
                    return true;
                }
                let next_hook = merge_hook_metadata(current_hook, hook);
                self.status_hooks
                    .entry(workspace_id.to_string())
                    .or_default()
                    .insert(key.to_string(), next_hook);
            }
            (Some(key), None) => {
                if let Some(hooks) = self.status_hooks.get_mut(workspace_id) {
                    hooks.remove(key);
                    let remove_workspace_hooks = hooks.is_empty();
                    if remove_workspace_hooks {
                        self.status_hooks.remove(workspace_id);
                    }
                }
            }
            (None, None) => {
                self.status_hooks.remove(workspace_id);
            }
            (None, Some(_)) => {
                self.status_hooks.remove(workspace_id);
            }
        }
        let Some(entries) = self.statuses.get_mut(workspace_id) else {
            return true;
        };
        if let Some(key) = key {
            entries.retain(|entry| entry.key != key);
        } else {
            entries.clear();
        }
        true
    }

    pub fn set_progress(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: f64,
        total: Option<f64>,
    ) -> Option<ProgressEntry> {
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let total = total.filter(|total| *total > 0.0);
        let value = total
            .map(|total| value.min(total))
            .unwrap_or(value)
            .max(0.0);
        let entry = ProgressEntry {
            key: key.into(),
            label: label.into(),
            value,
            total,
        };
        let entries = self.progress.entry(workspace_id.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            *existing = entry.clone();
        } else {
            // A new key grows the map; cap it so a misbehaving client cannot
            // exhaust memory. Drop the oldest entry when at capacity.
            if entries.len() >= MAX_PROGRESS_ENTRIES {
                entries.remove(0);
            }
            entries.push(entry.clone());
        }
        Some(entry)
    }

    pub fn list_progress(&self, workspace_id: &str) -> Vec<ProgressEntry> {
        self.progress.get(workspace_id).cloned().unwrap_or_default()
    }

    pub fn list_progress_limited(&self, workspace_id: &str, limit: usize) -> Vec<ProgressEntry> {
        self.progress
            .get(workspace_id)
            .map(|entries| entries[entries.len().saturating_sub(limit)..].to_vec())
            .unwrap_or_default()
    }

    pub fn clear_progress(&mut self, workspace_id: &str, key: Option<&str>) -> bool {
        let Some(entries) = self.progress.get_mut(workspace_id) else {
            return self.workspaces.contains_key(workspace_id);
        };
        if let Some(key) = key {
            entries.retain(|entry| entry.key != key);
        } else {
            entries.clear();
        }
        true
    }

    pub fn append_log(
        &mut self,
        workspace_id: &str,
        level: LogLevel,
        message: impl Into<String>,
    ) -> Option<LogEntry> {
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let entry = LogEntry {
            id: self.next_log_id(),
            timestamp_ms: now_ms(),
            level,
            message: message.into(),
        };
        let entries = self.logs.entry(workspace_id.to_string()).or_default();
        entries.insert(0, entry.clone());
        entries.truncate(MAX_LOG_ENTRIES);
        Some(entry)
    }

    pub fn list_logs(&self, workspace_id: &str) -> Vec<LogEntry> {
        self.logs.get(workspace_id).cloned().unwrap_or_default()
    }

    pub fn clear_logs(&mut self, workspace_id: &str) -> bool {
        let Some(entries) = self.logs.get_mut(workspace_id) else {
            return self.workspaces.contains_key(workspace_id);
        };
        entries.clear();
        true
    }

    pub fn restore_agent_metadata(
        &mut self,
        workspace_id: &str,
        metadata: ClearedAgentMetadata,
    ) -> bool {
        if !self.workspaces.contains_key(workspace_id) {
            return false;
        }
        for status in metadata.statuses {
            let entry = status.entry;
            let _ = self.set_status_with_hook_metadata(
                workspace_id,
                entry.key,
                entry.label,
                entry.value,
                entry.color,
                status.hook,
            );
        }
        for entry in metadata.progress {
            let _ = self.set_progress(
                workspace_id,
                entry.key,
                entry.label,
                entry.value,
                entry.total,
            );
        }
        true
    }

    fn clear_removed_surface_metadata(&mut self, surface: &Surface) {
        let surface_prefix = format!("surface:{}:", surface.id);
        self.clear_status_keys_matching(&surface.workspace_id, |key| {
            key.starts_with(&surface_prefix)
        });
        self.clear_progress_keys_matching(&surface.workspace_id, |key| {
            key.starts_with(&surface_prefix)
        });
        if let Some(agent_session) = surface.agent_session.as_ref() {
            self.clear_agent_metadata_if_no_active_session(
                &surface.workspace_id,
                agent_session.agent,
            );
        }
    }

    fn clear_agent_metadata_if_no_active_session(
        &mut self,
        workspace_id: &str,
        agent: AgentKind,
    ) -> ClearedAgentMetadata {
        if self.workspace_has_active_agent_metadata_session(workspace_id, agent) {
            return ClearedAgentMetadata::default();
        }
        let metadata = self.agent_metadata_snapshot(workspace_id, agent);
        self.clear_status_keys_matching(workspace_id, |key| {
            agent_metadata_status_key_matches(agent, key)
        });
        self.clear_progress_keys_matching(workspace_id, |key| {
            agent_metadata_progress_key_matches(agent, key)
        });
        metadata
    }

    fn agent_metadata_snapshot(
        &self,
        workspace_id: &str,
        agent: AgentKind,
    ) -> ClearedAgentMetadata {
        let statuses = self
            .statuses
            .get(workspace_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| agent_metadata_status_key_matches(agent, &entry.key))
                    .map(|entry| ClearedStatusEntry {
                        entry: entry.clone(),
                        hook: self
                            .status_hooks
                            .get(workspace_id)
                            .and_then(|hooks| hooks.get(&entry.key))
                            .cloned(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let progress = self
            .progress
            .get(workspace_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| agent_metadata_progress_key_matches(agent, &entry.key))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ClearedAgentMetadata { statuses, progress }
    }

    fn workspace_has_active_agent_metadata_session(
        &self,
        workspace_id: &str,
        agent: AgentKind,
    ) -> bool {
        self.surfaces.values().any(|surface| {
            surface.workspace_id == workspace_id
                && surface.agent_session.as_ref().is_some_and(|session| {
                    session.agent == agent
                        && agent_session_lifecycle_keeps_metadata(session.lifecycle)
                })
        })
    }

    fn clear_status_keys_matching(
        &mut self,
        workspace_id: &str,
        mut should_clear: impl FnMut(&str) -> bool,
    ) {
        let mut keys = BTreeSet::new();
        if let Some(entries) = self.statuses.get(workspace_id) {
            keys.extend(
                entries
                    .iter()
                    .filter(|entry| should_clear(&entry.key))
                    .map(|entry| entry.key.clone()),
            );
        }
        if let Some(hooks) = self.status_hooks.get(workspace_id) {
            keys.extend(hooks.keys().filter(|key| should_clear(key)).cloned());
        }
        for key in keys {
            let _ = self.clear_status(workspace_id, Some(&key));
        }
    }

    fn clear_progress_keys_matching(
        &mut self,
        workspace_id: &str,
        mut should_clear: impl FnMut(&str) -> bool,
    ) {
        let keys = self
            .progress
            .get(workspace_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| should_clear(&entry.key))
                    .map(|entry| entry.key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for key in keys {
            let _ = self.clear_progress(workspace_id, Some(&key));
        }
    }

    fn next_workspace_id(&mut self) -> WorkspaceId {
        loop {
            self.next_workspace = self.next_workspace.checked_add(1).unwrap_or(1);
            let candidate = format!("workspace-{}", self.next_workspace);
            if !self.workspaces.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn next_surface_id(&mut self) -> SurfaceId {
        loop {
            self.next_surface = self.next_surface.checked_add(1).unwrap_or(1);
            let candidate = format!("surface-{}", self.next_surface);
            if !self.surfaces.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn next_notification_id(&mut self) -> String {
        self.next_notification += 1;
        format!("notification-{}", self.next_notification)
    }

    fn mark_notification_target_unread(
        &mut self,
        workspace_id: Option<&str>,
        surface_id: Option<&str>,
    ) {
        if let Some(surface_id) = surface_id {
            if !self.mark_surface_unread(surface_id, true) {
                if let Some(workspace_id) = workspace_id {
                    if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                        workspace.needs_attention = true;
                    }
                }
            }
        } else if let Some(workspace_id) = workspace_id {
            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.needs_attention = true;
            }
        }
    }

    fn recompute_workspace_attention(&mut self, workspace_id: &str) {
        let has_unread_surface = self
            .surfaces
            .values()
            .any(|surface| surface.workspace_id == workspace_id && surface.unread);
        let has_unread_workspace_notification = self.notifications.iter().any(|notification| {
            !notification.read
                && notification.surface_id.is_none()
                && notification.workspace_id.as_deref() == Some(workspace_id)
        });
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            workspace.needs_attention = has_unread_surface || has_unread_workspace_notification;
        }
    }

    fn recompute_notification_attention(&mut self) {
        let unread_surface_ids = self
            .notifications
            .iter()
            .filter(|notification| !notification.read)
            .filter_map(|notification| notification.surface_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        let unread_workspace_ids = self
            .notifications
            .iter()
            .filter(|notification| !notification.read)
            .filter(|notification| notification.surface_id.is_none())
            .filter_map(|notification| notification.workspace_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        for surface in self.surfaces.values_mut() {
            let unread = unread_surface_ids.contains(&surface.id);
            surface.unread = unread;
            surface.needs_attention = unread;
        }
        for workspace in self.workspaces.values_mut() {
            workspace.needs_attention = unread_workspace_ids.contains(&workspace.id)
                || self.surfaces.values().any(|surface| {
                    surface.workspace_id == workspace.id
                        && (surface.unread || surface.needs_attention)
                });
        }
    }

    fn next_log_id(&mut self) -> String {
        self.next_log += 1;
        format!("log-{}", self.next_log)
    }

    /// Resolves a workspace selector to its current id without mutating state.
    /// Returns `None` if no workspace matches.
    pub fn workspace_id_for(&self, selector: WorkspaceSelector<'_>) -> Option<WorkspaceId> {
        match selector {
            WorkspaceSelector::Id(id) => self.workspaces.contains_key(id).then(|| id.to_string()),
            WorkspaceSelector::Name(name) => self.workspace_order.iter().find_map(|id| {
                self.workspaces
                    .get(id)
                    .filter(|workspace| workspace.name == name)
                    .map(|workspace| workspace.id.clone())
            }),
            WorkspaceSelector::WorktreeName(name) => self.workspace_order.iter().find_map(|id| {
                self.workspaces
                    .get(id)
                    .filter(|workspace| workspace.worktree_name.as_deref() == Some(name))
                    .map(|workspace| workspace.id.clone())
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WorkspaceSelector<'a> {
    Id(&'a str),
    Name(&'a str),
    WorktreeName(&'a str),
}

fn replace_leaf_with_split(
    node: &mut PaneNode,
    target_surface_id: &str,
    axis: SplitAxis,
    new_leaf: PaneNode,
) -> bool {
    match node {
        // Match a leaf whose tabs contain the target (splitting on active tab).
        PaneNode::Leaf { tabs, .. } if tabs.iter().any(|id| id == target_surface_id) => {
            // Clone the original leaf (with ALL its tabs) as the left child.
            let original_leaf = node.clone();
            *node = PaneNode::Split {
                axis,
                children: vec![original_leaf, new_leaf],
                sizes: vec![0.5, 0.5],
            };
            true
        }
        PaneNode::Leaf { .. } => false,
        PaneNode::Split {
            axis: split_axis,
            children,
            sizes,
        } => {
            if *split_axis == axis {
                for index in 0..children.len() {
                    if matches!(
                        &children[index],
                        PaneNode::Leaf { tabs, .. } if tabs.iter().any(|id| id == target_surface_id)
                    ) {
                        children.insert(index + 1, new_leaf);
                        rebalance_split_sizes(sizes, children.len());
                        return true;
                    }
                    if replace_leaf_with_split(
                        &mut children[index],
                        target_surface_id,
                        axis,
                        new_leaf.clone(),
                    ) {
                        return true;
                    }
                }
                false
            } else {
                children.iter_mut().any(|child| {
                    replace_leaf_with_split(child, target_surface_id, axis, new_leaf.clone())
                })
            }
        }
    }
}

/// Returns true when splitting `surface_id`'s leaf along `axis` would create
/// a `Split` node deeper than [`MAX_SESSION_SPLIT_DEPTH`].
///
/// Mirrors `replace_leaf_with_split`: when the leaf's direct parent split has
/// the same axis, the new pane is inserted as a sibling without deepening the
/// tree; otherwise the leaf is replaced by a new `Split` whose depth equals
/// the number of `Split` ancestors of the leaf (the path length).
fn split_would_exceed_depth(node: &PaneNode, surface_id: &str, axis: SplitAxis) -> bool {
    let Some(path) = leaf_path_for_surface(node, surface_id) else {
        return false;
    };
    if let Some((_, parent_path)) = path.split_last() {
        if let Some(PaneNode::Split {
            axis: parent_axis, ..
        }) = pane_node_at_path(node, parent_path)
        {
            if *parent_axis == axis {
                // Sibling insertion into the existing split: no new depth.
                return false;
            }
        }
    }
    path.len() > MAX_SESSION_SPLIT_DEPTH
}

fn rebalance_split_sizes(sizes: &mut Vec<f64>, len: usize) {
    if len == 0 {
        sizes.clear();
        return;
    }
    sizes.clear();
    sizes.resize(len, 1.0 / len as f64);
}

fn remove_leaf(node: &mut PaneNode, target_surface_id: &str) -> Option<bool> {
    match node {
        // A leaf is the root — signal "removed root"
        PaneNode::Leaf { tabs, .. } if tabs.len() == 1 && tabs[0] == target_surface_id => {
            Some(true)
        }
        PaneNode::Leaf { .. } => None,
        PaneNode::Split {
            children, sizes, ..
        } => {
            let mut remove_index = None;
            for (index, child) in children.iter_mut().enumerate() {
                match remove_leaf(child, target_surface_id) {
                    Some(true) => {
                        remove_index = Some(index);
                        break;
                    }
                    Some(false) => return Some(false),
                    None => {}
                }
            }
            let index = remove_index?;
            children.remove(index);
            if index < sizes.len() {
                sizes.remove(index);
            }
            if children.len() == 1 {
                *node = children.remove(0);
            } else {
                rebalance_split_sizes(sizes, children.len());
            }
            Some(false)
        }
    }
}

fn update_partition_ratio(
    node: &mut PaneNode,
    left_leaves: &[SurfaceId],
    right_leaves: &[SurfaceId],
    ratio: f64,
) -> bool {
    match node {
        PaneNode::Leaf { .. } => false,
        PaneNode::Split {
            children, sizes, ..
        } => {
            if sizes.len() != children.len() {
                rebalance_split_sizes(sizes, children.len());
            }
            let mut prefix_leaves: Vec<SurfaceId> = Vec::new();
            for split_at in 1..children.len() {
                collect_leaf_surface_ids(&children[split_at - 1], &mut prefix_leaves);
                if prefix_leaves.as_slice() != left_leaves {
                    continue;
                }
                let mut suffix_leaves: Vec<SurfaceId> = Vec::new();
                for child in &children[split_at..] {
                    collect_leaf_surface_ids(child, &mut suffix_leaves);
                }
                if suffix_leaves.as_slice() != right_leaves {
                    continue;
                }
                apply_partition_ratio(sizes, split_at, ratio);
                return true;
            }
            children
                .iter_mut()
                .any(|child| update_partition_ratio(child, left_leaves, right_leaves, ratio))
        }
    }
}

fn move_tab_in_leaf(
    node: &mut PaneNode,
    source_surface_id: &str,
    target_surface_id: &str,
    position: MovePosition,
) -> bool {
    match node {
        PaneNode::Leaf { tabs, active } => {
            let Some(source_index) = tabs.iter().position(|id| id == source_surface_id) else {
                return false;
            };
            if !tabs.iter().any(|id| id == target_surface_id) {
                return false;
            }
            let previous = tabs.clone();
            let active_id = tabs.get(*active).cloned();
            let source = tabs.remove(source_index);
            let Some(mut target_index) = tabs.iter().position(|id| id == target_surface_id) else {
                *tabs = previous;
                return false;
            };
            if position == MovePosition::After {
                target_index += 1;
            }
            let target_index = target_index.min(tabs.len());
            tabs.insert(target_index, source);
            if let Some(active_id) = active_id {
                *active = tabs
                    .iter()
                    .position(|id| id == &active_id)
                    .unwrap_or_else(|| tabs.len().saturating_sub(1));
            } else {
                *active = (*active).min(tabs.len().saturating_sub(1));
            }
            previous != *tabs
        }
        PaneNode::Split { children, .. } => children
            .iter_mut()
            .any(|child| move_tab_in_leaf(child, source_surface_id, target_surface_id, position)),
    }
}

fn move_tab_in_tree(
    node: &mut PaneNode,
    source_surface_id: &str,
    target_surface_id: &str,
    position: MovePosition,
) -> bool {
    let Some(source_path) = leaf_path_for_surface(node, source_surface_id) else {
        return false;
    };
    let Some(target_path) = leaf_path_for_surface(node, target_surface_id) else {
        return false;
    };
    if source_path == target_path {
        return move_tab_in_leaf(node, source_surface_id, target_surface_id, position);
    }
    move_tab_between_leaves(
        node,
        &source_path,
        &target_path,
        source_surface_id,
        target_surface_id,
        position,
    )
}

fn move_tab_between_leaves(
    node: &mut PaneNode,
    source_path: &[usize],
    target_path: &[usize],
    source_surface_id: &str,
    target_surface_id: &str,
    position: MovePosition,
) -> bool {
    let source_can_move = matches!(
        pane_node_at_path(node, source_path),
        Some(PaneNode::Leaf { tabs, .. })
            if tabs.len() > 1 && tabs.iter().any(|id| id == source_surface_id)
    );
    let target_can_receive = matches!(
        pane_node_at_path(node, target_path),
        Some(PaneNode::Leaf { tabs, .. }) if tabs.iter().any(|id| id == target_surface_id)
    );
    if !source_can_move || !target_can_receive {
        return false;
    }

    let (moved_id, source_was_active) = {
        let Some(PaneNode::Leaf { tabs, active }) = pane_node_at_path_mut(node, source_path) else {
            return false;
        };
        let Some(source_index) = tabs.iter().position(|id| id == source_surface_id) else {
            return false;
        };
        let active_id = tabs.get(*active).cloned();
        let moved_id = tabs.remove(source_index);
        let source_was_active = active_id.as_ref() == Some(&moved_id);
        if source_was_active {
            *active = source_index.min(tabs.len().saturating_sub(1));
        } else if let Some(active_id) = active_id {
            *active = tabs
                .iter()
                .position(|id| id == &active_id)
                .unwrap_or_else(|| tabs.len().saturating_sub(1));
        } else {
            *active = (*active).min(tabs.len().saturating_sub(1));
        }
        (moved_id, source_was_active)
    };

    let Some(PaneNode::Leaf { tabs, active }) = pane_node_at_path_mut(node, target_path) else {
        return false;
    };
    let previous = tabs.clone();
    let active_id = tabs.get(*active).cloned();
    let Some(mut target_index) = tabs.iter().position(|id| id == target_surface_id) else {
        return false;
    };
    if position == MovePosition::After {
        target_index += 1;
    }
    let target_index = target_index.min(tabs.len());
    tabs.insert(target_index, moved_id);
    if source_was_active {
        *active = target_index;
    } else if let Some(active_id) = active_id {
        *active = tabs
            .iter()
            .position(|id| id == &active_id)
            .unwrap_or_else(|| tabs.len().saturating_sub(1));
    } else {
        *active = (*active).min(tabs.len().saturating_sub(1));
    }
    previous != *tabs
}

fn swap_pane_leaves(node: &mut PaneNode, source_surface_id: &str, target_surface_id: &str) -> bool {
    let Some(source_path) = leaf_path_for_surface(node, source_surface_id) else {
        return false;
    };
    let Some(target_path) = leaf_path_for_surface(node, target_surface_id) else {
        return false;
    };
    if source_path == target_path {
        return false;
    }
    let Some(source_leaf) = pane_node_at_path(node, &source_path).cloned() else {
        return false;
    };
    let Some(target_leaf) = pane_node_at_path(node, &target_path).cloned() else {
        return false;
    };
    if let Some(target) = pane_node_at_path_mut(node, &source_path) {
        *target = target_leaf;
    } else {
        return false;
    }
    if let Some(target) = pane_node_at_path_mut(node, &target_path) {
        *target = source_leaf;
        true
    } else {
        false
    }
}

fn leaf_path_for_surface(node: &PaneNode, surface_id: &str) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    if collect_leaf_path_for_surface(node, surface_id, &mut path) {
        Some(path)
    } else {
        None
    }
}

fn collect_leaf_path_for_surface(node: &PaneNode, surface_id: &str, path: &mut Vec<usize>) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => tabs.iter().any(|id| id == surface_id),
        PaneNode::Split { children, .. } => {
            for (index, child) in children.iter().enumerate() {
                path.push(index);
                if collect_leaf_path_for_surface(child, surface_id, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
    }
}

fn pane_node_at_path<'a>(node: &'a PaneNode, path: &[usize]) -> Option<&'a PaneNode> {
    let mut current = node;
    for index in path {
        let PaneNode::Split { children, .. } = current else {
            return None;
        };
        current = children.get(*index)?;
    }
    Some(current)
}

fn pane_node_at_path_mut<'a>(node: &'a mut PaneNode, path: &[usize]) -> Option<&'a mut PaneNode> {
    let mut current = node;
    for index in path {
        let PaneNode::Split { children, .. } = current else {
            return None;
        };
        current = children.get_mut(*index)?;
    }
    Some(current)
}

fn apply_partition_ratio(sizes: &mut [f64], split_at: usize, ratio: f64) {
    let len = sizes.len();
    if split_at == 0 || split_at >= len {
        return;
    }
    let total: f64 = sizes.iter().filter(|s| s.is_finite() && **s > 0.0).sum();
    let total = if total > f64::EPSILON {
        total
    } else {
        len as f64
    };
    let left_sum: f64 = sizes[..split_at]
        .iter()
        .filter(|s| s.is_finite() && **s > 0.0)
        .sum();
    let right_sum: f64 = sizes[split_at..]
        .iter()
        .filter(|s| s.is_finite() && **s > 0.0)
        .sum();
    let target_left = total * ratio;
    let target_right = total * (1.0 - ratio);
    let scale_left = if left_sum > f64::EPSILON {
        target_left / left_sum
    } else {
        target_left / split_at as f64
    };
    let scale_right = if right_sum > f64::EPSILON {
        target_right / right_sum
    } else {
        target_right / (len - split_at) as f64
    };
    for (index, size) in sizes.iter_mut().enumerate() {
        let base = if size.is_finite() && *size > 0.0 {
            *size
        } else {
            1.0
        };
        if index < split_at {
            *size = if left_sum > f64::EPSILON {
                base * scale_left
            } else {
                scale_left
            };
        } else {
            *size = if right_sum > f64::EPSILON {
                base * scale_right
            } else {
                scale_right
            };
        }
    }
}

fn repair_pane_tree_structure(node: &mut PaneNode) -> bool {
    let mut changed = false;
    // Repair leaf invariants: tabs must be non-empty; active must be in range.
    if let PaneNode::Leaf { tabs, active } = node {
        if tabs.is_empty() {
            // Caller should drop this leaf; mark changed so the split above prunes it.
            // We leave tabs empty; the prune below in Split handling will discard it.
            return true;
        }
        let clamped = (*active).min(tabs.len().saturating_sub(1));
        if clamped != *active {
            *active = clamped;
            changed = true;
        }
        return changed;
    }
    if let PaneNode::Split {
        children, sizes, ..
    } = node
    {
        for child in children.iter_mut() {
            if repair_pane_tree_structure(child) {
                changed = true;
            }
        }
        let child_count_before_prune = children.len();
        children.retain(|child| first_leaf_surface_id(child).is_some());
        if children.len() != child_count_before_prune {
            changed = true;
        }
        if children.len() == 1 {
            *node = children.remove(0);
            return true;
        }
        let size_mismatch = sizes.len() != children.len();
        let invalid_size = sizes.iter().any(|s| !s.is_finite() || *s <= 0.0);
        if size_mismatch || invalid_size {
            rebalance_split_sizes(sizes, children.len());
            changed = true;
        }
    }
    changed
}

fn rename_leaf(node: &mut PaneNode, old_id: &str, new_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => {
            let mut found = false;
            for tab in tabs.iter_mut() {
                if tab == old_id {
                    *tab = new_id.to_string();
                    found = true;
                    // Each id should appear at most once; stop after the first match.
                    break;
                }
            }
            found
        }
        PaneNode::Split { children, .. } => {
            for child in children {
                if rename_leaf(child, old_id, new_id) {
                    return true;
                }
            }
            false
        }
    }
}

/// The pane that should inherit focus when `surface_id`'s leaf is removed:
/// the sibling immediately before it in its parent split (or after it, for
/// the first child). `None` when the leaf is the root.
fn neighbor_leaf_surface_id(node: &PaneNode, surface_id: &str) -> Option<SurfaceId> {
    let path = leaf_path_for_surface(node, surface_id)?;
    let (&child_index, parent_path) = path.split_last()?;
    let PaneNode::Split { children, .. } = pane_node_at_path(node, parent_path)? else {
        return None;
    };
    let sibling = if child_index > 0 {
        children.get(child_index - 1)
    } else {
        children.get(child_index + 1)
    }?;
    first_leaf_surface_id(sibling)
}

fn first_leaf_surface_id(node: &PaneNode) -> Option<SurfaceId> {
    match node {
        PaneNode::Leaf { tabs, active } => {
            // Return the active tab if valid, else first.
            tabs.get(*active).or_else(|| tabs.first()).cloned()
        }
        PaneNode::Split { children, .. } => children.iter().find_map(first_leaf_surface_id),
    }
}

/// Derive a browser pane title from its URL host, falling back to "browser".
/// Returns true if `s` begins with a valid URI scheme followed by `://`.
///
/// A scheme matches `^[a-zA-Z][a-zA-Z0-9+.-]*://` per RFC 3986. This deliberately
/// only inspects the *leading* portion so a query/path containing `://`
/// (e.g. `example.com/?next=https://x`) is not mistaken for an already-schemed
/// URL.
pub fn has_uri_scheme(s: &str) -> bool {
    let Some(idx) = s.find("://") else {
        return false;
    };
    let scheme = &s[..idx];
    !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Maximum normalized browser URL size accepted for model persistence.
pub const MAX_BROWSER_URL_BYTES: usize = 8_192;

/// Normalize a user-entered browser URL.
///
/// Whitespace-only input is rejected. Bare domains and paths get an `https://`
/// prefix; URLs accepted by `has_uri_scheme` are preserved after trimming.
pub fn normalize_browser_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if has_uri_scheme(trimmed) {
        Some(trimmed.to_string())
    } else {
        Some(format!("https://{trimmed}"))
    }
}

/// Normalize and size-check a browser URL before it is stored or navigated.
pub fn validated_browser_url(input: &str) -> Option<String> {
    let url = normalize_browser_url(input)?;
    if url.len() > MAX_BROWSER_URL_BYTES {
        None
    } else {
        Some(url)
    }
}

/// Size-check a URL already committed by the browser engine.
///
/// Unlike user-entered URLs, committed URLs may be non-hierarchical WebKit
/// values such as `about:blank`, `data:...`, or `blob:...`; preserve them.
fn validated_committed_browser_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_BROWSER_URL_BYTES {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn browser_title_for(url: &str) -> String {
    // Only http(s)-style URLs with an authority get a host-based title;
    // schemes like about:, data:, javascript: fall back.
    let Some((_, after_scheme)) = url.split_once("://") else {
        return "browser".to_string();
    };
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // Strip userinfo (user:pass@) so credentials never appear in the title.
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, h)| h)
        .trim();
    if host.is_empty() {
        "browser".to_string()
    } else {
        host.to_string()
    }
}

fn normalize_workspace_focus(workspace: &mut Workspace) {
    if has_leaf_surface_id(&workspace.pane_tree, &workspace.focused_surface_id) {
        return;
    }
    if let Some(first_leaf) = first_leaf_surface_id(&workspace.pane_tree) {
        workspace.focused_surface_id = first_leaf;
    }
}

fn has_leaf_surface_id(node: &PaneNode, surface_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => tabs.iter().any(|tab| tab == surface_id),
        PaneNode::Split { children, .. } => children
            .iter()
            .any(|child| has_leaf_surface_id(child, surface_id)),
    }
}

fn leaf_surface_ids(node: &PaneNode) -> Vec<SurfaceId> {
    let mut ids = Vec::new();
    collect_leaf_surface_ids(node, &mut ids);
    ids
}

fn collect_leaf_surface_ids(node: &PaneNode, ids: &mut Vec<SurfaceId>) {
    match node {
        PaneNode::Leaf { tabs, .. } => {
            for tab in tabs {
                ids.push(tab.clone());
            }
        }
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_leaf_surface_ids(child, ids);
            }
        }
    }
}

/// Remove `target_surface_id` from a multi-tab leaf.
/// Returns the remaining active tab in that leaf when the id was removed.
/// Returns `None` when the id is not found or it was the only tab, in which
/// case the caller must use the leaf-collapse path.
fn remove_tab_from_leaf(node: &mut PaneNode, target_surface_id: &str) -> Option<SurfaceId> {
    match node {
        PaneNode::Leaf { tabs, active } => {
            let pos = tabs.iter().position(|id| id == target_surface_id)?;
            if tabs.len() == 1 {
                // Last tab — caller must handle leaf collapse.
                return None;
            }
            tabs.remove(pos);
            // Clamp active to the new last index, preferring the previous tab.
            *active = if *active >= pos && *active > 0 {
                *active - 1
            } else {
                (*active).min(tabs.len().saturating_sub(1))
            };
            tabs.get(*active).cloned()
        }
        PaneNode::Split { children, .. } => children
            .iter_mut()
            .find_map(|child| remove_tab_from_leaf(child, target_surface_id)),
    }
}

/// Set the `active` index in the leaf that contains `surface_id`.
/// Returns `true` if found and updated.
fn set_leaf_active_for_surface(node: &mut PaneNode, surface_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, active } => {
            if let Some(pos) = tabs.iter().position(|id| id == surface_id) {
                *active = pos;
                true
            } else {
                false
            }
        }
        PaneNode::Split { children, .. } => children
            .iter_mut()
            .any(|child| set_leaf_active_for_surface(child, surface_id)),
    }
}

/// Push `new_tab_id` to the tabs of the leaf containing `near_surface_id`.
/// Sets `active` to the new tab's index. Returns `true` if found.
fn push_tab_to_leaf(node: &mut PaneNode, near_surface_id: &str, new_tab_id: SurfaceId) -> bool {
    match node {
        PaneNode::Leaf { tabs, active } => {
            if tabs.iter().any(|id| id == near_surface_id) {
                tabs.push(new_tab_id);
                *active = tabs.len() - 1;
                true
            } else {
                false
            }
        }
        PaneNode::Split { children, .. } => children
            .iter_mut()
            .any(|child| push_tab_to_leaf(child, near_surface_id, new_tab_id.clone())),
    }
}

fn numeric_suffix(id: &str) -> u64 {
    id.rsplit_once('-')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}

fn should_ignore_hook_status(
    current: Option<&StatusHookMetadata>,
    incoming: &StatusHookMetadata,
) -> bool {
    let Some(current) = current else {
        return false;
    };
    if let (Some(incoming_order), Some(current_order)) = (incoming.order, current.order) {
        // Orders are only comparable when both sides used the same clock; a
        // stored order from a different clock (e.g. a wall-clock order kept
        // from before an upgrade to boottime ordering) must not drop newer
        // updates forever, so mismatched clocks accept the incoming update.
        if same_order_clock(current, incoming) && incoming_order < current_order {
            return true;
        }
    }
    if incoming.event == "prompt-submit" && is_terminal_hook_event(&current.event) {
        if incoming
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| current.turn_id.as_deref() == Some(turn_id))
        {
            return true;
        }
        if incoming.turn_id.is_none()
            && same_monotonic_clock(current, incoming)
            && incoming
                .order
                .zip(current.order)
                .is_some_and(|(incoming_order, current_order)| {
                    incoming_order >= current_order
                        && incoming_order - current_order <= HOOK_TERMINAL_PROMPT_GUARD_NS
                })
        {
            return true;
        }
    }
    false
}

fn merge_hook_metadata(
    current: Option<&StatusHookMetadata>,
    mut incoming: StatusHookMetadata,
) -> StatusHookMetadata {
    if is_terminal_hook_event(&incoming.event) && incoming.turn_id.is_none() {
        incoming.turn_id = current.and_then(|current| current.turn_id.clone());
    }
    incoming
}

fn is_terminal_hook_event(event: &str) -> bool {
    matches!(event, "stop" | "stop-failure" | "session-end")
}

fn same_order_clock(current: &StatusHookMetadata, incoming: &StatusHookMetadata) -> bool {
    current.clock == incoming.clock
}

fn same_monotonic_clock(current: &StatusHookMetadata, incoming: &StatusHookMetadata) -> bool {
    const MONOTONIC_CLOCKS: &[&str] = &["monotonic-ns", "boottime-ns"];
    match (current.clock.as_deref(), incoming.clock.as_deref()) {
        (Some(current), Some(incoming)) => {
            current == incoming && MONOTONIC_CLOCKS.contains(&current)
        }
        _ => false,
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests;
