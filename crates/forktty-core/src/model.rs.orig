//! In-memory workspace, pane, surface, and per-surface runtime metadata model.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agents::AgentKind;
use crate::session::{SessionData, SESSION_FORMAT_VERSION};

mod activity;
mod browser_url;
mod hook_status;
mod pane_tree;
mod surface;

use activity::agent_session_lifecycle_keeps_metadata;
pub use activity::{ClearedAgentMetadata, ClearedAgentSession};
#[cfg(test)]
use activity::{MAX_NOTIFICATIONS, MAX_PROGRESS_ENTRIES, MAX_STATUS_ENTRIES};
use browser_url::{browser_title_for, validated_committed_browser_url};
pub use browser_url::{
    has_uri_scheme, normalize_browser_url, validated_browser_url, MAX_BROWSER_URL_BYTES,
};
#[cfg(test)]
use hook_status::HOOK_TERMINAL_PROMPT_GUARD_NS;
use pane_tree::{
    first_leaf_surface_id, has_leaf_surface_id, leaf_surface_ids, move_tab_in_tree,
    neighbor_leaf_surface_id, normalize_workspace_focus, push_tab_to_leaf, remove_leaf,
    remove_tab_from_leaf, rename_leaf, repair_pane_tree_structure, replace_leaf_with_split,
    set_leaf_active_for_surface, split_would_exceed_depth, swap_pane_leaves,
    update_partition_ratio,
};
use surface::normalize_persisted_scrollback;
pub use surface::{
    AgentSession, AgentSessionLifecycle, Surface, SurfaceKind, MAX_PERSISTED_SCROLLBACK_BYTES,
};

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

/// Outcome of resolving a canonical worktree identity in the workspace model.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeWorkspaceResolution {
    /// The selected existing workspace or the newly allocated workspace.
    pub workspace: Workspace,
    /// Whether this resolution allocated the workspace and its initial surface.
    pub created: bool,
}

/// Filesystem-independent snapshot of a modeled worktree identity.
///
/// Capture these while holding a model mutex, then resolve them after releasing
/// the mutex with [`resolve_worktree_identity_snapshots`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentitySnapshot {
    workspace_id: WorkspaceId,
    worktree_name: String,
    worktree_dir: PathBuf,
}

impl WorktreeIdentitySnapshot {
    /// Capture worktree identity inputs from persisted workspace data.
    pub fn from_workspaces(workspaces: &[Workspace]) -> Vec<Self> {
        workspaces.iter().filter_map(Self::from_workspace).collect()
    }

    fn from_workspace(workspace: &Workspace) -> Option<Self> {
        Some(Self {
            workspace_id: workspace.id.clone(),
            worktree_name: workspace.worktree_name.clone()?,
            worktree_dir: workspace.worktree_dir.clone()?,
        })
    }
}

/// Canonical worktree identity resolved before acquiring a model mutex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorktreeIdentity {
    snapshot: WorktreeIdentitySnapshot,
    canonical_path: PathBuf,
}

impl ResolvedWorktreeIdentity {
    fn matches_workspace(&self, workspace: &Workspace) -> bool {
        self.snapshot.workspace_id == workspace.id
            && workspace.worktree_name.as_ref() == Some(&self.snapshot.worktree_name)
            && workspace.worktree_dir.as_ref() == Some(&self.snapshot.worktree_dir)
    }

    fn matches_target(&self, worktree_name: &str, canonical_path: &Path) -> bool {
        self.snapshot.worktree_name == worktree_name && self.canonical_path == canonical_path
    }
}

/// Resolve worktree snapshots without requiring access to the workspace model.
///
/// Snapshots whose path cannot be canonicalized are deliberately omitted so
/// unresolved session records cannot alias a live worktree.
/// Call this only after releasing any mutex that protects the model.
pub fn resolve_worktree_identity_snapshots(
    snapshots: impl IntoIterator<Item = WorktreeIdentitySnapshot>,
) -> Vec<ResolvedWorktreeIdentity> {
    snapshots
        .into_iter()
        .filter_map(|snapshot| {
            let canonical_path = std::fs::canonicalize(&snapshot.worktree_dir).ok()?;
            Some(ResolvedWorktreeIdentity {
                snapshot,
                canonical_path,
            })
        })
        .collect()
}

/// Filesystem-resolved path identity used only by prepared worktree removal.
///
/// This may resolve a missing suffix through its nearest existing ancestor;
/// normal workspace creation and session deduplication keep requiring a fully
/// canonicalizable path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRemovalIdentity(PathBuf);

impl WorktreeRemovalIdentity {
    /// Resolve a removal target without requiring its leaf to still exist.
    pub fn resolve(path: &Path) -> Option<Self> {
        let mut cursor = path;
        let mut missing_components = Vec::<OsString>::new();
        loop {
            match std::fs::canonicalize(cursor) {
                Ok(mut canonical) => {
                    for component in missing_components.iter().rev() {
                        canonical.push(component);
                    }
                    return Some(Self(canonical));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing_components.push(cursor.file_name()?.to_os_string());
                    cursor = cursor.parent()?;
                }
                Err(_) => return None,
            }
        }
    }
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
/// Semantic category used to correlate a hook prompt with its completion.
pub enum HookPromptKind {
    /// A provider is waiting for permission to perform an operation.
    Permission,
    /// A provider is waiting for user-supplied input.
    Elicitation,
    /// A provider reported a condition that needs user attention.
    Attention,
}

/// Provider prompt identity retained alongside an in-app notification.
///
/// Prompt metadata is private correlation state: the notification carries the
/// workspace and surface target, while these fields identify the provider
/// event. A completion can resolve only a prompt from the same provider,
/// session, and kind whose event order is not newer than the completion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookPromptMetadata {
    /// Provider-supplied prompt ID used to reuse repeated prompt notifications.
    pub id: String,
    /// Provider that emitted the prompt.
    pub provider: String,
    /// Provider session that emitted the prompt.
    pub session_id: String,
    /// Semantic category of the prompt.
    pub kind: HookPromptKind,
    /// Provider event order at which the prompt was emitted.
    pub event_order: u128,
    /// Optional provider correlation token for matching a specific result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// Completion identity used to resolve one retained hook prompt.
///
/// Provider, session, kind, workspace, and surface must match exactly. When
/// `correlation_id` is present it must also match exactly; without one, the
/// newest compatible prompt at or before `event_order` is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookPromptResolution {
    /// Provider that emitted the completion.
    pub provider: String,
    /// Provider session that emitted the completion.
    pub session_id: String,
    /// Prompt category completed by the event.
    pub kind: HookPromptKind,
    /// Completion event order, which is the upper bound for matching prompts.
    pub event_order: u128,
    /// Optional provider token that narrows the match to one prompt.
    pub correlation_id: Option<String>,
    /// Exact workspace target recorded on the prompt notification.
    pub workspace_id: Option<WorkspaceId>,
    /// Exact surface target recorded on the prompt notification.
    pub surface_id: Option<SurfaceId>,
}

/// One retained in-app notification and its optional workspace/surface target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationItem {
    /// Stable model-generated notification ID.
    pub id: String,
    /// Short text displayed as the notification heading.
    pub title: String,
    /// Detailed notification text.
    pub body: String,
    /// Semantic presentation category.
    pub kind: NotificationKind,
    /// Unix timestamp in milliseconds; updates advance it to preserve ordering.
    pub created_at_ms: u128,
    /// Whether the retained notification no longer contributes unread state.
    #[serde(default)]
    pub read: bool,
    /// Optional workspace to which the notification belongs.
    #[serde(default)]
    pub workspace_id: Option<WorkspaceId>,
    /// Optional surface to which the notification belongs.
    #[serde(default)]
    pub surface_id: Option<SurfaceId>,
    /// Optional terminal-origin metadata used for actions and desktop hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_metadata: Option<TerminalNotificationMetadata>,
}

/// Result of inserting a notification into the bounded model history.
///
/// Callers dispatch `notification` and close any desktop notifications whose
/// IDs appear in `evicted_desktop_notification_ids`. Every notification removed
/// by the retention limit appears there in chronological order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationCreation {
    /// Newly created notification, or the refreshed existing prompt record.
    pub notification: NotificationItem,
    /// Notification IDs evicted from retained history, oldest first.
    pub evicted_desktop_notification_ids: Vec<String>,
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
    pub session_id: Option<String>,
    pub event: String,
    pub order: Option<u128>,
    pub clock: Option<String>,
    pub turn_id: Option<String>,
}

impl StatusHookMetadata {
    pub fn from_order(order: u128) -> Self {
        Self {
            session_id: None,
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
    hook_prompt_correlations: BTreeMap<String, HookPromptMetadata>,
    statuses: BTreeMap<WorkspaceId, Vec<StatusEntry>>,
    status_hooks: BTreeMap<WorkspaceId, BTreeMap<String, StatusHookMetadata>>,
    progress: BTreeMap<WorkspaceId, Vec<ProgressEntry>>,
    logs: BTreeMap<WorkspaceId, Vec<LogEntry>>,
    output_unread_surface_ids: BTreeSet<SurfaceId>,
    notification_unread_surface_ids: BTreeSet<SurfaceId>,
    next_workspace: u64,
    next_surface: u64,
    next_notification: u64,
    next_log: u64,
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
        self.allocate_worktree_workspace(
            name.into(),
            working_dir.into(),
            git_branch.into(),
            worktree_name.into(),
        )
    }

    /// Capture worktree identity inputs without accessing the filesystem.
    pub fn worktree_identity_snapshots(&self) -> Vec<WorktreeIdentitySnapshot> {
        self.workspace_order
            .iter()
            .filter_map(|workspace_id| self.workspaces.get(workspace_id))
            .filter_map(WorktreeIdentitySnapshot::from_workspace)
            .collect()
    }

    /// Finds the earliest workspace with this exact worktree name and canonical path.
    ///
    /// Only workspaces represented by a successfully pre-resolved identity are
    /// eligible, so unverifiable session records cannot alias a live worktree.
    pub fn find_worktree_workspace(
        &self,
        worktree_name: &str,
        canonical_path: &Path,
        resolved_identities: &[ResolvedWorktreeIdentity],
    ) -> Option<Workspace> {
        self.workspace_order
            .iter()
            .filter_map(|workspace_id| self.workspaces.get(workspace_id))
            .find(|workspace| {
                resolved_identities.iter().any(|identity| {
                    identity.matches_workspace(workspace)
                        && identity.matches_target(worktree_name, canonical_path)
                })
            })
            .cloned()
    }

    /// Selects and refreshes an exact worktree match, or allocates one.
    ///
    /// Reuse preserves the workspace's display name and existing surfaces.
    pub fn find_or_create_worktree_workspace(
        &mut self,
        name: impl Into<String>,
        canonical_path: &Path,
        git_branch: impl Into<String>,
        worktree_name: impl Into<String>,
        resolved_identities: &[ResolvedWorktreeIdentity],
    ) -> WorktreeWorkspaceResolution {
        let git_branch = git_branch.into();
        let worktree_name = worktree_name.into();
        if let Some(existing) =
            self.find_worktree_workspace(&worktree_name, canonical_path, resolved_identities)
        {
            for workspace in self.workspaces.values_mut() {
                workspace.active = workspace.id == existing.id;
            }
            let workspace = self
                .workspaces
                .get_mut(&existing.id)
                .expect("workspace was found immediately above");
            workspace.working_dir = canonical_path.to_path_buf();
            workspace.git_branch = git_branch;
            workspace.worktree_dir = Some(canonical_path.to_path_buf());
            return WorktreeWorkspaceResolution {
                workspace: workspace.clone(),
                created: false,
            };
        }

        WorktreeWorkspaceResolution {
            workspace: self.allocate_worktree_workspace(
                name.into(),
                canonical_path.to_path_buf(),
                git_branch,
                worktree_name,
            ),
            created: true,
        }
    }

    fn allocate_worktree_workspace(
        &mut self,
        name: String,
        working_dir: PathBuf,
        git_branch: String,
        worktree_name: String,
    ) -> Workspace {
        let id = self.create_workspace(name, working_dir).id;
        // `create_workspace` just inserted this id, so the lookup cannot fail.
        let workspace = self
            .workspaces
            .get_mut(&id)
            .expect("workspace just created");
        workspace.git_branch = git_branch;
        workspace.worktree_dir = Some(workspace.working_dir.clone());
        workspace.worktree_name = Some(worktree_name);
        workspace.clone()
    }

    /// Restore a session using worktree identities resolved before model access.
    pub fn restore_session(
        &mut self,
        data: SessionData,
        resolved_worktree_identities: &[ResolvedWorktreeIdentity],
    ) {
        *self = WorkspaceModel::new();
        self.next_workspace = data.next_workspace;
        self.next_surface = data.next_surface;
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
            let _ = ensure_focused_surface_active(&mut workspace);
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
        let _ = self.repair_session_invariants(resolved_worktree_identities);
    }

    pub fn to_session_data(&self) -> SessionData {
        let mut workspaces = self.list_workspaces();
        for workspace in &mut workspaces {
            normalize_workspace_focus(workspace);
            workspace.listening_ports.clear();
            workspace.pr = None;
        }
        // Plain terminals at their workspace launch directory are reconstructed
        // from the pane tree. Persist terminals that moved elsewhere so each
        // pane can resume in its last live working directory.
        let surfaces = self
            .surfaces
            .values()
            .filter(|surface| {
                !matches!(surface.kind, SurfaceKind::Terminal)
                    || surface.agent_session.is_some()
                    || surface.persisted_scrollback.is_some()
                    || self
                        .workspaces
                        .get(&surface.workspace_id)
                        .is_some_and(|workspace| surface.cwd != workspace.working_dir)
            })
            .cloned()
            .collect();
        SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces,
            active_workspace_id: self.active_workspace_id(),
            surfaces,
            next_workspace: self.next_workspace,
            next_surface: self.next_surface,
        }
    }

    /// Repair structural invariants using canonical identities resolved before
    /// mutable model access.
    pub fn repair_session_invariants(
        &mut self,
        resolved_worktree_identities: &[ResolvedWorktreeIdentity],
    ) -> bool {
        let mut changed = self.collapse_duplicate_worktree_workspaces(resolved_worktree_identities);
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
                if ensure_focused_surface_active(workspace) {
                    changed = true;
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

    fn collapse_duplicate_worktree_workspaces(
        &mut self,
        resolved_worktree_identities: &[ResolvedWorktreeIdentity],
    ) -> bool {
        let mut winners: BTreeMap<(String, PathBuf), (WorkspaceId, bool)> = BTreeMap::new();
        let mut loser_ids = Vec::new();

        for workspace_id in &self.workspace_order {
            let Some(workspace) = self.workspaces.get(workspace_id) else {
                continue;
            };
            let Some(identity) = resolved_worktree_identities
                .iter()
                .find(|identity| identity.matches_workspace(workspace))
            else {
                continue;
            };
            let canonical_identity = (
                identity.snapshot.worktree_name.clone(),
                identity.canonical_path.clone(),
            );
            if let Some((winner_id, winner_active)) = winners.get_mut(&canonical_identity) {
                if winner_id == workspace_id {
                    continue;
                }
                if workspace.active && !*winner_active {
                    loser_ids.push(winner_id.clone());
                    *winner_id = workspace_id.clone();
                    *winner_active = true;
                } else {
                    loser_ids.push(workspace_id.clone());
                }
            } else {
                winners.insert(canonical_identity, (workspace_id.clone(), workspace.active));
            }
        }

        let mut changed = false;
        for loser_id in loser_ids {
            let removed_surface_ids = self
                .surfaces
                .values()
                .filter(|surface| surface.workspace_id == loser_id)
                .map(|surface| surface.id.clone())
                .collect::<BTreeSet<_>>();
            if self
                .close_workspace(WorkspaceSelector::Id(&loser_id))
                .is_none()
            {
                continue;
            }
            self.output_unread_surface_ids
                .retain(|surface_id| !removed_surface_ids.contains(surface_id));
            self.notification_unread_surface_ids
                .retain(|surface_id| !removed_surface_ids.contains(surface_id));
            self.notifications.retain(|notification| {
                notification.workspace_id.as_deref() != Some(loser_id.as_str())
                    && notification
                        .surface_id
                        .as_ref()
                        .is_none_or(|surface_id| !removed_surface_ids.contains(surface_id))
            });
            changed = true;
        }
        changed
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

    /// Reserve a generated workspace id that may still be referenced outside
    /// the current model snapshot.
    ///
    /// This does not create a workspace. It only advances the monotonic id
    /// high-water mark so a later `create_workspace` cannot recycle the same
    /// `workspace-N` id for an unrelated workspace.
    pub fn reserve_workspace_id(&mut self, workspace_id: &str) {
        self.next_workspace = self.next_workspace.max(numeric_suffix(workspace_id));
    }

    /// Reserve a generated surface id that may still be referenced outside the
    /// GTK session.
    ///
    /// See [`WorkspaceModel::reserve_workspace_id`] for the durable-record
    /// collision this avoids.
    pub fn reserve_surface_id(&mut self, surface_id: &str) {
        self.next_surface = self.next_surface.max(numeric_suffix(surface_id));
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
        self.open_browser_near(&focused, url, profile, axis)
    }

    /// Split an exact source surface into a new browser pane.
    pub fn open_browser_near(
        &mut self,
        surface_id: &str,
        url: &str,
        profile: crate::profile::ProfileId,
        axis: SplitAxis,
    ) -> Option<Surface> {
        let title = browser_title_for(url);
        self.split_with(
            surface_id,
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

    /// Restore an exact pane tree after a failed runtime topology change.
    ///
    /// The supplied tree must reference only surfaces that still belong to the
    /// workspace, and the focused surface must be one of those leaves.
    pub fn restore_workspace_pane_layout(
        &mut self,
        workspace_id: &str,
        pane_tree: PaneNode,
        focused_surface_id: SurfaceId,
    ) -> bool {
        let leaf_ids = leaf_surface_ids(&pane_tree);
        if !leaf_ids
            .iter()
            .any(|surface_id| surface_id == &focused_surface_id)
            || leaf_ids.iter().any(|surface_id| {
                self.surfaces
                    .get(surface_id)
                    .is_none_or(|surface| surface.workspace_id != workspace_id)
            })
        {
            return false;
        }
        let Some(workspace) = self.workspaces.get_mut(workspace_id) else {
            return false;
        };
        workspace.pane_tree = pane_tree;
        workspace.focused_surface_id = focused_surface_id;
        true
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

fn ensure_focused_surface_active(workspace: &mut Workspace) -> bool {
    let before = workspace.pane_tree.clone();
    if set_leaf_active_for_surface(&mut workspace.pane_tree, &workspace.focused_surface_id) {
        workspace.pane_tree != before
    } else {
        false
    }
}

fn numeric_suffix(id: &str) -> u64 {
    id.rsplit_once('-')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests;
