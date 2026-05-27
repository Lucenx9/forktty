use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::{SessionData, SESSION_FORMAT_VERSION};

pub type WorkspaceId = String;
pub type SurfaceId = String;

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
const HOOK_TERMINAL_PROMPT_GUARD_NS: u128 = 2_000_000_000;

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
                    },
                };
                self.next_surface = self.next_surface.max(numeric_suffix(&surface.id));
                self.surfaces.insert(surface.id.clone(), surface);
            }
            self.next_workspace = self.next_workspace.max(numeric_suffix(&workspace.id));
            self.workspace_order.push(workspace.id.clone());
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
        // Only non-terminal surfaces need to be persisted explicitly: terminal
        // surfaces are the default and are reconstructed from the pane tree.
        // This keeps sessions written without browser panes byte-identical.
        let surfaces = self
            .surfaces
            .values()
            .filter(|surface| !matches!(surface.kind, SurfaceKind::Terminal))
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
            for surface_id in &canonical_leaf_ids {
                valid_surface_ids.insert(surface_id.clone());
                match self.surfaces.get_mut(surface_id) {
                    Some(existing) => {
                        if existing.workspace_id != workspace_id_owned {
                            existing.workspace_id = workspace_id_owned.clone();
                            changed = true;
                        }
                    }
                    None => {
                        self.surfaces.insert(
                            surface_id.clone(),
                            Surface {
                                id: surface_id.clone(),
                                workspace_id: workspace_id_owned.clone(),
                                cwd: working_dir.clone(),
                                title: String::from("shell"),
                                unread: false,
                                needs_attention: false,
                                kind: SurfaceKind::Terminal,
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
        if !leaf_surface_ids(&workspace_ref.pane_tree)
            .iter()
            .any(|id| id == surface_id)
        {
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
        if left_leaves.is_empty() || right_leaves.is_empty() {
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
        if !leaf_surface_ids(&workspace.pane_tree).contains(&surface_id.to_string()) {
            return false;
        }
        workspace.focused_surface_id = surface_id.to_string();
        // If the surface is a non-active tab in its leaf, update active.
        set_leaf_active_for_surface(&mut workspace.pane_tree, surface_id);
        true
    }

    /// Add a new terminal tab to the pane whose tabs contain `near_surface_id`.
    /// The new tab becomes the active tab of that pane and the workspace focus.
    /// Returns the newly created Surface on success.
    pub fn add_tab(&mut self, near_surface_id: &str) -> Option<Surface> {
        let source = self.surfaces.get(near_surface_id)?.clone();
        let workspace_id = source.workspace_id.clone();
        // Verify the surface lives in a leaf of its workspace.
        if !leaf_surface_ids(&self.workspaces.get(&workspace_id)?.pane_tree)
            .contains(&near_surface_id.to_string())
        {
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

    /// Update a browser surface's URL. Same-URL navigation is a successful no-op.
    /// Returns false for terminals, SSH surfaces, or missing ids.
    pub fn set_surface_url(&mut self, surface_id: &str, url: &str) -> bool {
        match self.surfaces.get_mut(surface_id) {
            Some(surface) => match &mut surface.kind {
                SurfaceKind::Browser { url: current, .. } => {
                    if current == url {
                        return true;
                    }
                    *current = url.to_string();
                    surface.title = browser_title_for(url);
                    true
                }
                SurfaceKind::Terminal | SurfaceKind::Ssh { .. } => false,
            },
            None => false,
        }
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
                } else if !leaf_surface_ids(&workspace.pane_tree)
                    .contains(&workspace.focused_surface_id)
                {
                    if let Some(first) = first_leaf_surface_id(&workspace.pane_tree) {
                        workspace.focused_surface_id = first;
                    }
                }
                let removed = self.surfaces.remove(surface_id)?;
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
                }
            }
        };
        let workspace = self.workspaces.get_mut(&workspace_id)?;
        let removed_root = remove_leaf(&mut workspace.pane_tree, surface_id)?;
        let removed = self.surfaces.remove(surface_id)?;
        let next_focus = if removed_root {
            None
        } else {
            first_leaf_surface_id(&workspace.pane_tree)
        };
        if let Some(next_focus) = next_focus {
            workspace.focused_surface_id = next_focus;
        } else {
            workspace.pane_tree = PaneNode::single_leaf(replacement.id.clone());
            workspace.focused_surface_id = replacement.id.clone();
            self.surfaces.insert(replacement.id.clone(), replacement);
        }
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
        };
        if let Some(surface_id) = &item.surface_id {
            if !self.mark_surface_unread(surface_id, true) {
                if let Some(workspace_id) = &item.workspace_id {
                    if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                        workspace.needs_attention = true;
                    }
                }
            }
        } else if let Some(workspace_id) = &item.workspace_id {
            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.needs_attention = true;
            }
        }
        self.notifications.push(item.clone());
        item
    }

    pub fn list_notifications(&self) -> Vec<NotificationItem> {
        self.notifications.clone()
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
                    .or(Some(entry));
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
        if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            *existing = entry.clone();
        } else {
            entries.push(entry.clone());
        }
        Some(entry)
    }

    pub fn list_status(&self, workspace_id: &str) -> Vec<StatusEntry> {
        self.statuses.get(workspace_id).cloned().unwrap_or_default()
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
            entries.push(entry.clone());
        }
        Some(entry)
    }

    pub fn list_progress(&self, workspace_id: &str) -> Vec<ProgressEntry> {
        self.progress.get(workspace_id).cloned().unwrap_or_default()
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

    fn next_workspace_id(&mut self) -> WorkspaceId {
        loop {
            self.next_workspace += 1;
            let candidate = format!("workspace-{}", self.next_workspace);
            if !self.workspaces.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn next_surface_id(&mut self) -> SurfaceId {
        loop {
            self.next_surface += 1;
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
    let leaf_ids = leaf_surface_ids(&workspace.pane_tree);
    if !leaf_ids.contains(&workspace.focused_surface_id) {
        if let Some(first_leaf) = leaf_ids.first() {
            workspace.focused_surface_id = first_leaf.clone();
        }
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
        if incoming_order < current_order {
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

fn same_monotonic_clock(current: &StatusHookMetadata, incoming: &StatusHookMetadata) -> bool {
    current.clock.as_deref() == Some("monotonic-ns")
        && incoming.clock.as_deref() == Some("monotonic-ns")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_workspace_has_one_surface_and_leaf() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        assert_eq!(workspace.id, "workspace-1");
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 1);
        assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
    }

    #[test]
    fn set_listening_ports_sorts_dedupes_and_reports_change() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        assert!(model.set_listening_ports(&workspace.id, vec![8080, 3000, 3000]));
        assert_eq!(
            model.workspaces[&workspace.id].listening_ports,
            vec![3000, 8080]
        );
        // Same set in a different order is not a change.
        assert!(!model.set_listening_ports(&workspace.id, vec![8080, 3000]));
        // Unknown workspace is a no-op.
        assert!(!model.set_listening_ports("workspace-404", vec![1234]));
    }

    #[test]
    fn set_pr_reports_change_only_on_difference() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let pr = crate::pr::PrInfo {
            number: 42,
            state: crate::pr::PrState::Open,
            url: "u".to_string(),
        };

        assert!(model.set_pr(&workspace.id, Some(pr.clone())));
        assert!(!model.set_pr(&workspace.id, Some(pr)));
        assert!(model.set_pr(&workspace.id, None));
        assert!(!model.set_pr("workspace-404", None));
    }

    #[test]
    fn session_data_drops_runtime_sidebar_hints() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model.set_listening_ports(&workspace.id, vec![8080]);
        model.set_pr(
            &workspace.id,
            Some(crate::pr::PrInfo {
                number: 42,
                state: crate::pr::PrState::Open,
                url: "https://github.com/o/r/pull/42".to_string(),
            }),
        );

        let data = model.to_session_data();

        assert!(data.workspaces[0].listening_ports.is_empty());
        assert!(data.workspaces[0].pr.is_none());
        let json = serde_json::to_string(&data).unwrap();
        assert!(!json.contains("listening_ports"));
        assert!(!json.contains("\"pr\""));
    }

    #[test]
    fn split_surface_adds_second_surface_and_focuses_it() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let new_surface = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, new_surface.id);
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 2);
        assert!(matches!(workspace.pane_tree, PaneNode::Split { .. }));
    }

    #[test]
    fn split_surface_does_not_leak_id_when_source_is_not_a_leaf_in_workspace() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        // Detach the second surface from the pane tree but keep it in the
        // surface map — emulating a corrupted in-memory state.
        let first = workspace.focused_surface_id;
        model.workspaces.get_mut(&workspace.id).unwrap().pane_tree =
            PaneNode::single_leaf(first.clone());
        let before = model.next_surface;

        assert!(model
            .split_surface(&second.id, SplitAxis::Horizontal)
            .is_none());
        assert_eq!(
            model.next_surface, before,
            "failed split must not advance the surface id counter"
        );
    }

    #[test]
    fn focus_surface_rejects_surface_outside_workspace_pane_tree() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let first = workspace.focused_surface_id;
        model.workspaces.get_mut(&workspace.id).unwrap().pane_tree = PaneNode::single_leaf(first);

        assert!(!model.focus_surface(&second.id));
    }

    #[test]
    fn repair_session_invariants_restores_missing_focus() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let first = workspace.focused_surface_id;
        {
            let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
            workspace.pane_tree = PaneNode::single_leaf(first.clone());
            workspace.focused_surface_id = second.id.clone();
        }

        assert!(model.repair_session_invariants());
        let repaired = model.list_workspaces().remove(0);
        assert_eq!(repaired.focused_surface_id, first);
        assert!(model.surface(&second.id).is_none());
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_advances_id_counters_for_repaired_leaves() {
        let mut model = WorkspaceModel::new();
        let workspace = Workspace {
            id: "workspace-1".to_string(),
            name: "main".to_string(),
            active: true,
            working_dir: PathBuf::from("/tmp"),
            git_branch: String::new(),
            worktree_dir: None,
            worktree_name: None,
            pane_tree: PaneNode::single_leaf("surface-1".to_string()),
            focused_surface_id: "surface-1".to_string(),
            needs_attention: false,
            listening_ports: Vec::new(),
            pr: None,
        };
        model.workspace_order.push(workspace.id.clone());
        model.workspaces.insert(workspace.id.clone(), workspace);

        assert!(model.repair_session_invariants());
        let split = model
            .split_surface("surface-1", SplitAxis::Horizontal)
            .unwrap();

        assert_eq!(split.id, "surface-2");
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn restore_session_clears_stale_workspace_attention() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model.create_notification(
            "Prompt",
            "Needs input",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        assert!(model.list_workspaces()[0].needs_attention);
        let data = model.to_session_data();

        let mut fresh = WorkspaceModel::new();
        fresh.restore_session(data);

        // Notifications are not persisted, so the restored workspace must not
        // keep the saved attention badge.
        assert!(!fresh.list_workspaces()[0].needs_attention);
        assert!(
            !fresh
                .surface(&fresh.list_workspaces()[0].focused_surface_id)
                .unwrap()
                .unread
        );
    }

    #[test]
    fn repeated_same_axis_splits_are_siblings() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let third = model
            .split_surface(&second.id, SplitAxis::Horizontal)
            .unwrap();

        let workspace = model.list_workspaces().remove(0);

        let PaneNode::Split {
            axis,
            children,
            sizes,
        } = workspace.pane_tree
        else {
            panic!("expected split pane tree");
        };
        assert_eq!(axis, SplitAxis::Horizontal);
        assert_eq!(children.len(), 3);
        assert_eq!(sizes.len(), 3);
        assert_eq!(workspace.focused_surface_id, third.id);
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 3);
    }

    #[test]
    fn closing_split_surface_rebalances_sizes() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let _third = model
            .split_surface(&second.id, SplitAxis::Horizontal)
            .unwrap();

        model.close_surface(&second.id).unwrap();
        let workspace = model.list_workspaces().remove(0);

        let PaneNode::Split {
            children, sizes, ..
        } = workspace.pane_tree
        else {
            panic!("expected split pane tree");
        };
        assert_eq!(children.len(), 2);
        assert_eq!(sizes, vec![0.5, 0.5]);
    }

    #[test]
    fn root_surface_close_can_use_prepared_replacement() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp/project");
        let old_surface_id = workspace.focused_surface_id.clone();
        let replacement = model
            .prepare_root_surface_replacement(&old_surface_id)
            .unwrap();

        let removed =
            model.close_surface_with_replacement(&old_surface_id, Some(replacement.clone()));

        assert_eq!(removed.unwrap().id, old_surface_id);
        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, replacement.id);
        assert_eq!(model.list_surfaces(Some(&workspace.id)), vec![replacement]);
    }

    #[test]
    fn update_split_partition_ratio_persists_drag() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let third = model
            .split_surface(&second.id, SplitAxis::Horizontal)
            .unwrap();

        let left = vec![workspace.focused_surface_id.clone(), second.id.clone()];
        let right = vec![third.id.clone()];
        assert!(model.update_split_partition_ratio(&workspace.id, &left, &right, 0.8));

        let workspace = model.list_workspaces().remove(0);
        let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
            panic!("expected split pane tree");
        };
        let total: f64 = sizes.iter().sum();
        let left_sum: f64 = sizes[..2].iter().sum();
        assert!((total - 1.0).abs() < 1e-6);
        assert!((left_sum / total - 0.8).abs() < 1e-6);
        assert!((sizes[0] - sizes[1]).abs() < 1e-6);
    }

    #[test]
    fn update_split_partition_ratio_clamps_out_of_range_input() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();

        let left = vec![workspace.focused_surface_id.clone()];
        let right = vec![second.id.clone()];
        // Caller passes a wildly out-of-range value (negative). The model must
        // clamp into the valid (0.01, 0.99) band rather than corrupt sizes.
        assert!(model.update_split_partition_ratio(&workspace.id, &left, &right, -5.0));

        let workspace = model.list_workspaces().remove(0);
        let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
            panic!("expected split pane tree");
        };
        let total: f64 = sizes.iter().sum();
        assert!((total - 1.0).abs() < 1e-6);
        assert!(sizes.iter().all(|size| size.is_finite() && *size > 0.0));
        assert!(sizes[0] < sizes[1]);
        // Validation requires every size to be strictly positive and finite.
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn update_split_partition_ratio_rejects_unknown_partition() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();

        assert!(!model.update_split_partition_ratio(
            &workspace.id,
            std::slice::from_ref(&second.id),
            &["bogus".into()],
            0.5,
        ));
    }

    #[test]
    fn can_select_and_close_workspaces() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("main", "/tmp/main");
        let second = model.create_workspace("feature", "/tmp/feature");

        assert!(model.list_workspaces()[1].active);
        model
            .select_workspace(WorkspaceSelector::Id(&first.id))
            .unwrap();
        assert!(model.list_workspaces()[0].active);

        let removed = model
            .close_workspace(WorkspaceSelector::Name(&second.name))
            .unwrap();
        assert_eq!(removed.id, second.id);
        assert_eq!(model.list_workspaces().len(), 1);
    }

    #[test]
    fn can_rename_workspace() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp/main");

        let renamed = model
            .rename_workspace(WorkspaceSelector::Id(&workspace.id), "prod")
            .unwrap();

        assert_eq!(renamed.name, "prod");
        assert_eq!(model.list_workspaces()[0].name, "prod");
    }

    #[test]
    fn can_close_a_split_surface() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let new_surface = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();

        model.close_surface(&new_surface.id).unwrap();

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 1);
        assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
    }

    #[test]
    fn can_close_surface_nested_after_unrelated_split() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let third = model
            .split_surface(&second.id, SplitAxis::Vertical)
            .unwrap();
        let fourth = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Vertical)
            .unwrap();

        let removed = model.close_surface(&third.id).unwrap();

        assert_eq!(removed.id, third.id);
        assert!(model.surface(&third.id).is_none());
        assert!(model.surface(&fourth.id).is_some());
        let workspace = model.list_workspaces().remove(0);
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 3);
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn worktree_workspace_keeps_branch_and_worktree_metadata() {
        let mut model = WorkspaceModel::new();
        let workspace =
            model.create_worktree_workspace("feature", "/tmp/feature", "feature", "feature");

        assert_eq!(workspace.git_branch, "feature");
        assert_eq!(workspace.worktree_name.as_deref(), Some("feature"));
        assert_eq!(workspace.worktree_dir, Some(PathBuf::from("/tmp/feature")));
    }

    #[test]
    fn notification_marks_surface_unread() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model.create_notification(
            "Prompt",
            "Needs input",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );

        let surface = model.surface(&workspace.focused_surface_id).unwrap();
        assert!(surface.unread);
        assert!(model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn clear_notifications_resets_attention_state() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model.create_notification(
            "Prompt",
            "Ready",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );

        model.clear_notifications();

        assert!(model.list_notifications().is_empty());
        assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
        assert!(!model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn mark_notifications_read_keeps_items_and_clears_attention() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model.create_notification(
            "Prompt",
            "Ready",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );

        model.mark_notifications_read();

        let notifications = model.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].read);
        assert_eq!(model.unread_notification_count(), 0);
        assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
        assert!(!model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn dismiss_notification_keeps_attention_when_unread_target_remains() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first = model.create_notification(
            "Prompt",
            "Ready",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        model.create_notification(
            "Prompt",
            "Still ready",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );

        assert!(model.dismiss_notification(&first.id));

        assert_eq!(model.list_notifications().len(), 1);
        assert_eq!(model.unread_notification_count(), 1);
        assert!(model.surface(&workspace.focused_surface_id).unwrap().unread);
        assert!(model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn close_surface_clears_workspace_attention_when_unread_pane_is_removed() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        model.create_notification(
            "Prompt",
            "Needs input",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(split.id.clone()),
        );
        assert!(model.list_workspaces()[0].needs_attention);

        model.close_surface(&split.id).unwrap();

        assert!(!model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn close_surface_preserves_workspace_only_notification_attention() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        model.create_notification(
            "Workspace",
            "Needs input",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );

        model.close_surface(&split.id).unwrap();

        assert!(model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn closed_surface_notification_does_not_revive_workspace_attention() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        model.create_notification(
            "Prompt",
            "Needs input",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(split.id.clone()),
        );

        model.close_surface(&split.id).unwrap();
        assert!(!model.list_workspaces()[0].needs_attention);

        let focused_surface_id = model.list_workspaces()[0].focused_surface_id.clone();
        assert!(model.mark_surface_unread(&focused_surface_id, false));

        assert!(!model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn can_update_surface_title() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        assert!(model.set_surface_title(&workspace.focused_surface_id, "build"));

        assert_eq!(
            model.surface(&workspace.focused_surface_id).unwrap().title,
            "build"
        );
    }

    #[test]
    fn can_set_list_and_clear_workspace_status() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        let status = model
            .set_status(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
            )
            .unwrap();

        assert_eq!(status.value, "Running");
        assert_eq!(model.list_status(&workspace.id), vec![status]);

        model
            .set_status(&workspace.id, "agent:codex", "Codex", "Ready", None)
            .unwrap();
        assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

        assert!(model.clear_status(&workspace.id, Some("agent:codex")));
        assert!(model.list_status(&workspace.id).is_empty());
    }

    #[test]
    fn ordered_status_updates_ignore_stale_hook_events() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        model
            .set_status_ordered(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(10),
            )
            .unwrap();
        model
            .set_status_ordered(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Ready",
                Some("green".to_string()),
                Some(20),
            )
            .unwrap();
        model
            .set_status_ordered(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(10),
            )
            .unwrap();

        assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

        assert!(model.clear_status_ordered(&workspace.id, Some("agent:codex"), Some(30)));
        assert!(model.list_status(&workspace.id).is_empty());

        model
            .set_status_ordered(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(20),
            )
            .unwrap();
        assert!(
            model.list_status(&workspace.id).is_empty(),
            "stale prompt-submit must not revive a status cleared by a newer session-end",
        );

        model
            .set_status_ordered(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(40),
            )
            .unwrap();
        assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
    }

    #[test]
    fn hook_status_state_ignores_late_prompt_for_completed_turn() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(StatusHookMetadata {
                    event: "prompt-submit".to_string(),
                    order: Some(10),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: Some("prompt:one".to_string()),
                }),
            )
            .unwrap();
        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Ready",
                Some("green".to_string()),
                Some(StatusHookMetadata {
                    event: "stop".to_string(),
                    order: Some(20),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: None,
                }),
            )
            .unwrap();

        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(StatusHookMetadata {
                    event: "prompt-submit".to_string(),
                    order: Some(30),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: Some("prompt:one".to_string()),
                }),
            )
            .unwrap();
        assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(StatusHookMetadata {
                    event: "prompt-submit".to_string(),
                    order: Some(40),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: Some("prompt:two".to_string()),
                }),
            )
            .unwrap();
        assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
    }

    #[test]
    fn hook_status_state_briefly_guards_prompt_after_terminal_without_turn_id() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Ready",
                Some("green".to_string()),
                Some(StatusHookMetadata {
                    event: "stop".to_string(),
                    order: Some(20),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: None,
                }),
            )
            .unwrap();
        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(StatusHookMetadata {
                    event: "prompt-submit".to_string(),
                    order: Some(20 + HOOK_TERMINAL_PROMPT_GUARD_NS),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: None,
                }),
            )
            .unwrap();
        assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

        model
            .set_status_with_hook_metadata(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".to_string()),
                Some(StatusHookMetadata {
                    event: "prompt-submit".to_string(),
                    order: Some(21 + HOOK_TERMINAL_PROMPT_GUARD_NS),
                    clock: Some("monotonic-ns".to_string()),
                    turn_id: None,
                }),
            )
            .unwrap();
        assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
    }

    #[test]
    fn can_set_clear_progress_and_append_logs() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        let progress = model
            .set_progress(&workspace.id, "task", "Task", 150.0, Some(100.0))
            .unwrap();
        assert_eq!(progress.value, 100.0);
        assert_eq!(model.list_progress(&workspace.id), vec![progress]);

        let log = model
            .append_log(&workspace.id, LogLevel::Warn, "waiting for input")
            .unwrap();
        assert_eq!(log.level, LogLevel::Warn);
        assert_eq!(
            model.list_logs(&workspace.id)[0].message,
            "waiting for input"
        );

        assert!(model.clear_progress(&workspace.id, Some("task")));
        assert!(model.list_progress(&workspace.id).is_empty());
        assert!(model.clear_logs(&workspace.id));
        assert!(model.list_logs(&workspace.id).is_empty());
    }

    #[test]
    fn restore_session_collapses_multiple_active_flags_to_active_workspace_id() {
        let mut source = WorkspaceModel::new();
        source.create_workspace("first", "/tmp/a");
        let second = source.create_workspace("second", "/tmp/b");
        let mut data = source.to_session_data();
        // Two workspaces flagged active simultaneously must be reduced to a
        // single active by restore_session, following active_workspace_id.
        for workspace in &mut data.workspaces {
            workspace.active = true;
        }
        data.active_workspace_id = Some(second.id.clone());

        let mut restored = WorkspaceModel::new();
        restored.restore_session(data);

        let actives: Vec<_> = restored
            .list_workspaces()
            .into_iter()
            .filter(|workspace| workspace.active)
            .map(|workspace| workspace.id)
            .collect();
        assert_eq!(actives, vec![second.id]);
    }

    #[test]
    fn restore_session_assigns_first_workspace_active_when_id_is_missing() {
        let mut source = WorkspaceModel::new();
        source.create_workspace("first", "/tmp/a");
        source.create_workspace("second", "/tmp/b");
        let mut data = source.to_session_data();
        for workspace in &mut data.workspaces {
            workspace.active = false;
        }
        data.active_workspace_id = None;

        let mut restored = WorkspaceModel::new();
        restored.restore_session(data);

        let workspaces = restored.list_workspaces();
        assert!(workspaces[0].active);
        assert!(!workspaces[1].active);
    }

    #[test]
    fn restore_session_repairs_focused_surface_id_pointing_outside_pane_tree() {
        let mut source = WorkspaceModel::new();
        let workspace = source.create_workspace("main", "/tmp");
        let mut data = source.to_session_data();
        data.workspaces[0].focused_surface_id = "surface-99".to_string();

        let mut restored = WorkspaceModel::new();
        restored.restore_session(data);

        let restored_workspace = &restored.list_workspaces()[0];
        // The focus is normalised to a leaf that actually exists in the pane
        // tree, not the bogus id we forged.
        assert_ne!(restored_workspace.focused_surface_id, "surface-99");
        assert_eq!(
            restored_workspace.focused_surface_id,
            workspace.focused_surface_id
        );
        assert!(restored
            .surface(&restored_workspace.focused_surface_id)
            .is_some());
    }

    #[test]
    fn restore_session_repairs_duplicate_leaf_ids() {
        let mut source = WorkspaceModel::new();
        let first = source.create_workspace("first", "/tmp/a");
        let second = source.create_workspace("second", "/tmp/b");
        let mut data = source.to_session_data();
        data.workspaces[1].pane_tree = PaneNode::single_leaf(first.focused_surface_id.clone());
        data.workspaces[1].focused_surface_id = first.focused_surface_id.clone();

        let mut restored = WorkspaceModel::new();
        restored.restore_session(data);

        let workspaces = restored.list_workspaces();
        let first_leaves = leaf_surface_ids(&workspaces[0].pane_tree);
        let second_leaves = leaf_surface_ids(&workspaces[1].pane_tree);
        assert_eq!(first_leaves.len(), 1);
        assert_eq!(second_leaves.len(), 1);
        assert_ne!(first_leaves[0], second_leaves[0]);
        assert_eq!(
            restored.surface(&first_leaves[0]).unwrap().workspace_id,
            first.id
        );
        assert_eq!(
            restored.surface(&second_leaves[0]).unwrap().workspace_id,
            second.id
        );
        crate::session::validate_session_data(&restored.to_session_data()).unwrap();
    }

    #[test]
    fn close_workspace_keeps_single_active_workspace_invariant() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("first", "/tmp/a");
        let second = model.create_workspace("second", "/tmp/b");
        let third = model.create_workspace("third", "/tmp/c");
        // Third is active because create_workspace always activates the new one.
        assert_eq!(
            model.active_workspace_id().as_deref(),
            Some(third.id.as_str())
        );

        model
            .close_workspace(WorkspaceSelector::Id(&third.id))
            .unwrap();

        let actives: Vec<_> = model
            .list_workspaces()
            .into_iter()
            .filter(|workspace| workspace.active)
            .map(|workspace| workspace.id)
            .collect();
        assert_eq!(actives.len(), 1);
        // Falls back to the first workspace in insertion order.
        assert_eq!(actives[0], first.id);
        let _ = second;
    }

    #[test]
    fn active_workspace_returns_active_or_first_workspace() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("first", "/tmp/a");
        let second = model.create_workspace("second", "/tmp/b");

        assert_eq!(
            model.active_workspace().map(|workspace| workspace.id),
            Some(second.id.clone())
        );

        model.workspaces.get_mut(&first.id).unwrap().active = false;
        model.workspaces.get_mut(&second.id).unwrap().active = false;

        assert_eq!(
            model.active_workspace().map(|workspace| workspace.id),
            Some(first.id)
        );
    }

    #[test]
    fn dismissing_only_notification_for_surface_clears_workspace_attention() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let notification = model.create_notification(
            "Prompt",
            "Ready",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        assert!(model.list_workspaces()[0].needs_attention);

        assert!(model.dismiss_notification(&notification.id));

        assert!(!model.list_workspaces()[0].needs_attention);
        assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
    }

    #[test]
    fn clearing_surface_unread_preserves_workspace_only_notification_attention() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model.create_notification(
            "Workspace",
            "Needs input",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );

        assert!(model.mark_surface_unread(&workspace.focused_surface_id, false));
        assert!(model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn workspace_notification_marks_workspace_attention_without_surface() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let _ = model.create_notification(
            "Workspace",
            "Needs input",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );

        assert!(model.list_workspaces()[0].needs_attention);
        assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);

        model.mark_notifications_read();

        assert!(!model.list_workspaces()[0].needs_attention);

        let notification = model.create_notification(
            "Workspace",
            "Needs input again",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );
        assert!(model.list_workspaces()[0].needs_attention);

        assert!(model.dismiss_notification(&notification.id));

        assert!(!model.list_workspaces()[0].needs_attention);
    }

    #[test]
    fn workspace_id_for_resolves_each_selector_variant() {
        let mut model = WorkspaceModel::new();
        let main = model.create_workspace("main", "/tmp/main");
        let feature =
            model.create_worktree_workspace("feature", "/tmp/feature", "feature", "feature-wt");

        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::Id(&main.id)),
            Some(main.id.clone())
        );
        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::Name("feature")),
            Some(feature.id.clone())
        );
        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::WorktreeName("feature-wt")),
            Some(feature.id)
        );
        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::Id("workspace-missing")),
            None
        );
        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::Name("missing")),
            None
        );
        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::WorktreeName("missing")),
            None
        );
    }

    #[test]
    fn workspace_id_for_prefers_first_workspace_in_list_order_for_duplicate_names() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("seed", "/tmp/seed");
        let first_dup = model.create_workspace("dup", "/tmp/dup-1");
        for idx in 3..=9 {
            model.create_workspace(format!("w{idx}"), format!("/tmp/w{idx}"));
        }
        let second_dup = model.create_workspace("dup", "/tmp/dup-2");

        assert_eq!(
            model.workspace_id_for(WorkspaceSelector::Name("dup")),
            Some(first_dup.id),
        );
        assert_ne!(
            model.workspace_id_for(WorkspaceSelector::Name("dup")),
            Some(second_dup.id),
        );
    }

    #[test]
    fn next_surface_id_skips_collisions_with_non_numeric_ids() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        // Inject a surface keyed with the next monotonic id we would
        // otherwise hand out, emulating a restore from a session that
        // pre-allocated that name through some external route.
        let blocker_id = format!("surface-{}", model.next_surface + 1);
        model.surfaces.insert(
            blocker_id.clone(),
            Surface {
                id: blocker_id.clone(),
                workspace_id: workspace.id.clone(),
                cwd: PathBuf::from("/tmp"),
                title: String::from("shell"),
                unread: false,
                needs_attention: false,
                kind: SurfaceKind::Terminal,
            },
        );

        let new_surface = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        assert_ne!(new_surface.id, blocker_id);
    }

    #[test]
    fn repair_session_invariants_collapses_single_child_splits() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let leaf_id = workspace.focused_surface_id.clone();
        {
            let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
            workspace.pane_tree = PaneNode::Split {
                axis: SplitAxis::Horizontal,
                children: vec![PaneNode::single_leaf(leaf_id.clone())],
                sizes: vec![1.0],
            };
        }

        assert!(model.repair_session_invariants());

        let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
        assert!(matches!(
            repaired.pane_tree,
            PaneNode::Leaf { ref tabs, .. } if tabs.len() == 1 && tabs[0] == leaf_id
        ));
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_rebalances_non_finite_split_sizes() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        {
            let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
            if let PaneNode::Split { sizes, .. } = &mut workspace.pane_tree {
                sizes[0] = f64::NAN;
                sizes[1] = -1.0;
            } else {
                panic!("expected split pane tree");
            }
        }

        assert!(model.repair_session_invariants());

        let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
        match repaired.pane_tree {
            PaneNode::Split { sizes, .. } => {
                assert!(sizes.iter().all(|s| s.is_finite() && *s > 0.0));
            }
            _ => panic!("expected split"),
        }
        // Ensure the split surface is still reachable.
        assert!(model.surface(&split.id).is_some());
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_renames_duplicate_leaf_ids_across_workspaces() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("first", "/tmp/a");
        let second = model.create_workspace("second", "/tmp/b");
        // Force both workspaces to reference the same leaf surface id.
        let shared_id = first.focused_surface_id.clone();
        {
            let workspace = model.workspaces.get_mut(&second.id).unwrap();
            workspace.pane_tree = PaneNode::single_leaf(shared_id.clone());
            workspace.focused_surface_id = shared_id.clone();
        }
        model.surfaces.insert(
            shared_id.clone(),
            Surface {
                id: shared_id.clone(),
                workspace_id: second.id.clone(),
                cwd: PathBuf::from("/tmp/b"),
                title: String::from("shell"),
                unread: false,
                needs_attention: false,
                kind: SurfaceKind::Terminal,
            },
        );

        assert!(model.repair_session_invariants());

        let first_after = model.workspaces.get(&first.id).unwrap().clone();
        let second_after = model.workspaces.get(&second.id).unwrap().clone();
        let first_leaves = leaf_surface_ids(&first_after.pane_tree);
        let second_leaves = leaf_surface_ids(&second_after.pane_tree);
        assert_eq!(first_leaves.len(), 1);
        assert_eq!(second_leaves.len(), 1);
        assert_ne!(
            first_leaves[0], second_leaves[0],
            "duplicate leaf ids must be split between workspaces"
        );
        assert_eq!(first_after.focused_surface_id, first_leaves[0]);
        assert_eq!(second_after.focused_surface_id, second_leaves[0]);
        // Both surfaces should now resolve back to their owning workspace.
        assert_eq!(
            model.surface(&first_leaves[0]).unwrap().workspace_id,
            first.id
        );
        assert_eq!(
            model.surface(&second_leaves[0]).unwrap().workspace_id,
            second.id
        );
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_renames_duplicate_leaf_ids_within_workspace() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let shared_id = workspace.focused_surface_id.clone();
        {
            let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
            workspace.pane_tree = PaneNode::Split {
                axis: SplitAxis::Horizontal,
                children: vec![
                    PaneNode::single_leaf(shared_id.clone()),
                    PaneNode::single_leaf(shared_id.clone()),
                ],
                sizes: vec![0.5, 0.5],
            };
        }

        assert!(model.repair_session_invariants());

        let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
        let leaves = leaf_surface_ids(&repaired.pane_tree);
        assert_eq!(leaves.len(), 2);
        assert_ne!(
            leaves[0], leaves[1],
            "duplicate leaf ids must be split within a workspace"
        );
        assert_eq!(repaired.focused_surface_id, leaves[0]);
        assert_eq!(
            model.surface(&leaves[0]).unwrap().workspace_id,
            workspace.id
        );
        assert_eq!(
            model.surface(&leaves[1]).unwrap().workspace_id,
            workspace.id
        );
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_replaces_leafless_pane_tree() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let old_surface_id = workspace.focused_surface_id.clone();
        {
            let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
            workspace.pane_tree = PaneNode::Split {
                axis: SplitAxis::Horizontal,
                children: Vec::new(),
                sizes: Vec::new(),
            };
            workspace.focused_surface_id = "missing-surface".to_string();
        }

        assert!(model.repair_session_invariants());

        let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
        let leaves = leaf_surface_ids(&repaired.pane_tree);
        assert_eq!(leaves.len(), 1);
        assert_eq!(repaired.focused_surface_id, leaves[0]);
        assert_ne!(leaves[0], old_surface_id);
        assert!(model.surface(&old_surface_id).is_none());
        assert_eq!(
            model.surface(&leaves[0]).unwrap().workspace_id,
            workspace.id
        );
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_prunes_nested_leafless_splits() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let leaf_id = workspace.focused_surface_id.clone();
        {
            let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
            workspace.pane_tree = PaneNode::Split {
                axis: SplitAxis::Horizontal,
                children: vec![
                    PaneNode::Split {
                        axis: SplitAxis::Vertical,
                        children: Vec::new(),
                        sizes: Vec::new(),
                    },
                    PaneNode::single_leaf(leaf_id.clone()),
                ],
                sizes: vec![0.5, 0.5],
            };
        }

        assert!(model.repair_session_invariants());

        let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
        assert!(matches!(
            repaired.pane_tree,
            PaneNode::Leaf { ref tabs, .. } if tabs.len() == 1 && tabs[0] == leaf_id
        ));
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn repair_session_invariants_drops_orphan_surfaces() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        // Inject an orphan surface that no pane leaf references.
        let orphan_id = "surface-orphan".to_string();
        model.surfaces.insert(
            orphan_id.clone(),
            Surface {
                id: orphan_id.clone(),
                workspace_id: workspace.id.clone(),
                cwd: PathBuf::from("/tmp"),
                title: String::from("shell"),
                unread: false,
                needs_attention: false,
                kind: SurfaceKind::Terminal,
            },
        );
        // Also inject a surface tied to a no-longer-present workspace.
        let dangling_id = "surface-dangling".to_string();
        model.surfaces.insert(
            dangling_id.clone(),
            Surface {
                id: dangling_id.clone(),
                workspace_id: "workspace-missing".to_string(),
                cwd: PathBuf::from("/tmp"),
                title: String::from("shell"),
                unread: false,
                needs_attention: false,
                kind: SurfaceKind::Terminal,
            },
        );

        assert!(model.repair_session_invariants());

        assert!(model.surface(&orphan_id).is_none());
        assert!(model.surface(&dangling_id).is_none());
        // The original workspace surface must remain reachable.
        assert!(model.surface(&workspace.focused_surface_id).is_some());
    }

    #[test]
    fn can_restore_model_from_session_data() {
        let mut source = WorkspaceModel::new();
        let workspace = source.create_workspace("main", "/tmp");
        let split = source
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let mut session = source.to_session_data();
        session.active_workspace_id = Some("missing-workspace".to_string());

        let mut restored = WorkspaceModel::new();
        restored.restore_session(session);

        assert_eq!(restored.list_workspaces().len(), 1);
        assert!(restored.list_workspaces()[0].active);
        assert_eq!(restored.list_surfaces(None).len(), 2);
        assert!(restored.surface(&split.id).is_some());
        assert_eq!(restored.to_session_data().workspaces[0].name, "main");
    }

    #[test]
    fn surface_without_kind_field_deserializes_as_terminal() {
        // Sessions persisted before SurfaceKind existed have no `kind` key.
        let json = r#"{
            "id": "s1",
            "workspace_id": "w1",
            "cwd": "/tmp",
            "title": "shell",
            "unread": false,
            "needs_attention": false
        }"#;
        let surface: Surface = serde_json::from_str(json).unwrap();
        assert_eq!(surface.kind, SurfaceKind::Terminal);
    }

    #[test]
    fn open_browser_adds_browser_surface_splits_and_focuses() {
        let mut model = WorkspaceModel::default();
        let ws = model.create_workspace("w", PathBuf::from("/tmp"));
        let first = first_leaf_surface_id(&model.workspaces[&ws.id].pane_tree).unwrap();

        let browser = model
            .open_browser(
                &ws.id,
                "https://example.com",
                crate::profile::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .expect("browser surface created");

        assert_eq!(
            browser.kind,
            SurfaceKind::Browser {
                url: "https://example.com".to_string(),
                profile: crate::profile::ProfileId::default(),
            }
        );
        assert_eq!(browser.title, "example.com");
        assert_eq!(model.workspaces[&ws.id].focused_surface_id, browser.id);
        let leaves = leaf_surface_ids(&model.workspaces[&ws.id].pane_tree);
        assert!(leaves.contains(&first));
        assert!(leaves.contains(&browser.id));
    }

    #[test]
    fn open_browser_records_the_requested_profile() {
        use crate::profile::ProfileId;
        let mut model = WorkspaceModel::default();
        let ws = model.create_workspace("w", PathBuf::from("/tmp"));

        let custom = ProfileId::new();
        let surface = model
            .open_browser(&ws.id, "https://example.com", custom, SplitAxis::Horizontal)
            .expect("opens");
        match surface.kind {
            SurfaceKind::Browser { url, profile } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(profile, custom);
            }
            _ => panic!("expected a browser surface"),
        }
    }

    #[test]
    fn legacy_browser_surface_without_profile_loads_as_default() {
        use crate::profile::ProfileId;
        let json = r#"{"type":"browser","url":"https://example.com"}"#;
        let kind: SurfaceKind = serde_json::from_str(json).unwrap();
        match kind {
            SurfaceKind::Browser { url, profile } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(profile, ProfileId::default());
            }
            _ => panic!("expected browser"),
        }
    }

    #[test]
    fn set_surface_url_updates_only_browser_surfaces() {
        let mut model = WorkspaceModel::default();
        let ws = model.create_workspace("w", PathBuf::from("/tmp"));
        let terminal = first_leaf_surface_id(&model.workspaces[&ws.id].pane_tree).unwrap();
        let browser = model
            .open_browser(
                &ws.id,
                "https://a.com",
                crate::profile::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();

        assert!(model.set_surface_url(&browser.id, "https://b.com"));
        assert!(model.set_surface_url(&browser.id, "https://b.com"));
        assert_eq!(
            model.surface(&browser.id).unwrap().kind,
            SurfaceKind::Browser {
                url: "https://b.com".to_string(),
                profile: crate::profile::ProfileId::default(),
            }
        );
        // title also refreshes
        assert_eq!(model.surface(&browser.id).unwrap().title, "b.com");
        // terminal + missing rejected
        assert!(!model.set_surface_url(&terminal, "https://b.com"));
        assert!(!model.set_surface_url("nope", "https://b.com"));
    }

    #[test]
    fn browser_title_for_extracts_host_and_falls_back() {
        assert_eq!(browser_title_for("https://example.com"), "example.com");
        assert_eq!(
            browser_title_for("https://example.com/path?q=1#frag"),
            "example.com"
        );
        assert_eq!(
            browser_title_for("http://user:pass@example.com/"),
            "example.com"
        );
        assert_eq!(browser_title_for("about:blank"), "browser");
        assert_eq!(browser_title_for("data:text/html,hi"), "browser");
        assert_eq!(browser_title_for("https://"), "browser");
    }

    #[test]
    fn has_uri_scheme_detects_only_leading_scheme() {
        assert!(!has_uri_scheme("example.com"));
        assert!(has_uri_scheme("https://x"));
        // A `://` inside the query must not be mistaken for a scheme.
        assert!(!has_uri_scheme("example.com/?next=https://x"));
        assert!(has_uri_scheme("ftp://h"));
        assert!(has_uri_scheme("custom+scheme.1-2://h"));
        // Empty scheme, non-alpha leading char, and no `://` are all rejected.
        assert!(!has_uri_scheme("://x"));
        assert!(!has_uri_scheme("1http://x"));
        assert!(!has_uri_scheme("noscheme"));
    }

    #[test]
    fn normalize_browser_url_trims_and_defaults_to_https() {
        assert_eq!(
            normalize_browser_url(" example.com/path "),
            Some("https://example.com/path".to_string())
        );
        assert_eq!(
            normalize_browser_url("https://example.com"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            normalize_browser_url("custom+scheme.1-2://host"),
            Some("custom+scheme.1-2://host".to_string())
        );
        assert_eq!(normalize_browser_url(" \t\n "), None);
    }

    #[test]
    fn create_ssh_workspace_produces_ssh_surface() {
        let mut model = WorkspaceModel::new();
        let workspace =
            model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());

        assert_eq!(workspace.name, "remote");
        let surfaces = model.list_surfaces(Some(&workspace.id));
        assert_eq!(surfaces.len(), 1);
        let surface = &surfaces[0];
        assert_eq!(
            surface.kind,
            SurfaceKind::Ssh {
                host: "user@example.com".to_string()
            }
        );
        assert_eq!(surface.title, "ssh:user@example.com");
    }

    #[test]
    fn open_ssh_splits_into_ssh_surface() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let new_surface = model
            .open_ssh(
                &workspace.id,
                "server.local".to_string(),
                SplitAxis::Horizontal,
            )
            .expect("open_ssh succeeds");

        assert_eq!(
            new_surface.kind,
            SurfaceKind::Ssh {
                host: "server.local".to_string()
            }
        );
        assert_eq!(new_surface.title, "ssh:server.local");
        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, new_surface.id);
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 2);
    }

    #[test]
    fn ssh_workspace_survives_session_round_trip() {
        let mut model = WorkspaceModel::new();
        let workspace =
            model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());
        let ssh_id = workspace.focused_surface_id.clone();

        let data = model.to_session_data();
        let mut restored = WorkspaceModel::new();
        restored.restore_session(data);

        let surface = restored.surface(&ssh_id).expect("ssh surface restored");
        assert_eq!(
            surface.kind,
            SurfaceKind::Ssh {
                host: "user@example.com".to_string()
            }
        );
    }

    #[test]
    fn restore_session_preserves_browser_surface_kind() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("ws", "/tmp");
        model.open_browser(
            &workspace.id,
            "https://example.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Vertical,
        );
        let browser_id = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .find(|s| matches!(s.kind, SurfaceKind::Browser { .. }))
            .map(|s| s.id)
            .expect("browser surface present");

        let data = model.to_session_data();
        let mut restored = WorkspaceModel::new();
        restored.restore_session(data);

        let surface = restored.surface(&browser_id).expect("surface restored");
        assert_eq!(
            surface.kind,
            SurfaceKind::Browser {
                url: "https://example.com".to_string(),
                profile: crate::profile::ProfileId::default(),
            }
        );
    }

    // ── per-pane tab group tests ───────────────────────────────────────────────

    #[test]
    fn add_tab_creates_new_surface_in_same_leaf_and_focuses_it() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();

        let new_surface = model.add_tab(&first_id).expect("add_tab succeeds");

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, new_surface.id);
        // Both surfaces exist.
        assert!(model.surface(&first_id).is_some());
        assert!(model.surface(&new_surface.id).is_some());
        // The pane tree is still a single leaf with 2 tabs.
        let tabs = workspace.pane_tree.leaf_tabs().expect("root is leaf");
        assert_eq!(tabs.len(), 2);
        assert!(tabs.contains(&first_id));
        assert!(tabs.contains(&new_surface.id));
        // Active points to the newly added tab.
        assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&new_surface.id));
        // Surface count should be 2.
        assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 2);
    }

    #[test]
    fn select_tab_switches_active_and_focus() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        let tab2 = model.add_tab(&first_id).expect("add_tab");
        // Now tab2 is active. Switch back to first.
        assert!(model.select_tab(&first_id));

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, first_id);
        assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&first_id));
        // tab2 is still present.
        assert!(model.surface(&tab2.id).is_some());
    }

    #[test]
    fn close_tab_non_last_removes_only_from_leaf_without_collapse() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        let tab2 = model.add_tab(&first_id).expect("add_tab");

        // Close the second tab — the leaf must remain (still has first_id).
        let removed = model.close_surface(&tab2.id).expect("close succeeds");
        assert_eq!(removed.id, tab2.id);

        let workspace = model.list_workspaces().remove(0);
        // The pane tree must still be a leaf.
        assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
        // The leaf must now have exactly one tab.
        let tabs = workspace.pane_tree.leaf_tabs().expect("leaf");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0], first_id);
        // Focus reverted to the remaining tab.
        assert_eq!(workspace.focused_surface_id, first_id);
        // tab2 is gone from the surfaces map.
        assert!(model.surface(&tab2.id).is_none());
    }

    #[test]
    fn close_focused_tab_in_later_pane_keeps_focus_in_that_pane() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        let split_surface = model
            .split_surface(&first_id, SplitAxis::Horizontal)
            .expect("split succeeds");
        let second_tab = model.add_tab(&split_surface.id).expect("add_tab");

        let removed = model.close_surface(&second_tab.id).expect("close succeeds");
        assert_eq!(removed.id, second_tab.id);

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, split_surface.id);
        let PaneNode::Split { children, .. } = workspace.pane_tree else {
            panic!("expected split");
        };
        assert_eq!(children[1].leaf_active_id(), Some(&split_surface.id));
    }

    #[test]
    fn close_background_pane_tab_preserves_current_focus() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        let focused_pane = model
            .split_surface(&first_id, SplitAxis::Horizontal)
            .expect("split succeeds");
        let background_tab = model.add_tab(&first_id).expect("add_tab");
        assert!(model.focus_surface(&focused_pane.id));

        let removed = model
            .close_surface(&background_tab.id)
            .expect("close succeeds");
        assert_eq!(removed.id, background_tab.id);

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, focused_pane.id);
        let PaneNode::Split { children, .. } = workspace.pane_tree else {
            panic!("expected split");
        };
        assert_eq!(children[0].leaf_active_id(), Some(&first_id));
    }

    #[test]
    fn close_last_tab_collapses_leaf_and_replaces_with_new_terminal() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let only_id = workspace.focused_surface_id.clone();

        let removed = model.close_surface(&only_id).expect("close succeeds");
        assert_eq!(removed.id, only_id);

        let workspace = model.list_workspaces().remove(0);
        // A replacement leaf was created.
        assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
        let new_id = workspace.focused_surface_id.clone();
        assert_ne!(new_id, only_id);
        assert!(model.surface(&new_id).is_some());
        assert!(model.surface(&only_id).is_none());
    }

    #[test]
    fn split_surface_on_multi_tab_leaf_preserves_all_tabs_in_original_pane() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        let tab2 = model.add_tab(&first_id).expect("add_tab");

        // Now split the pane (the active tab is tab2).
        let new_split = model
            .split_surface(&tab2.id, SplitAxis::Horizontal)
            .expect("split succeeds");

        let workspace = model.list_workspaces().remove(0);
        // The pane tree must now be a split with 2 leaves.
        let PaneNode::Split { ref children, .. } = workspace.pane_tree else {
            panic!("expected split");
        };
        assert_eq!(children.len(), 2);
        // The first child keeps BOTH original tabs.
        let orig_tabs = children[0].leaf_tabs().expect("first child is leaf");
        assert_eq!(orig_tabs.len(), 2);
        assert!(orig_tabs.contains(&first_id));
        assert!(orig_tabs.contains(&tab2.id));
        // The second child is the new split surface.
        let new_tabs = children[1].leaf_tabs().expect("second child is leaf");
        assert_eq!(new_tabs, std::slice::from_ref(&new_split.id));
        // Focus is on the new split surface.
        assert_eq!(workspace.focused_surface_id, new_split.id);
    }

    #[test]
    fn update_split_partition_ratio_works_with_multi_tab_leaf() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        // Add a second tab to the initial leaf.
        let tab2 = model.add_tab(&first_id).expect("add_tab");
        // Split on tab2 to get two panes.
        let right_pane = model
            .split_surface(&tab2.id, SplitAxis::Horizontal)
            .expect("split");

        // The left leaf now has [first_id, tab2.id]; right leaf has [right_pane.id].
        let left_leaves = vec![first_id.clone(), tab2.id.clone()];
        let right_leaves = vec![right_pane.id.clone()];
        assert!(model.update_split_partition_ratio(
            &workspace.id,
            &left_leaves,
            &right_leaves,
            0.7,
        ));

        let workspace = model.list_workspaces().remove(0);
        let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
            panic!("expected split");
        };
        let total: f64 = sizes.iter().sum();
        assert!((total - 1.0).abs() < 1e-6);
        assert!((sizes[0] / total - 0.7).abs() < 1e-6);
    }

    #[test]
    fn focus_surface_on_non_active_tab_updates_leaf_active() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        let _tab2 = model.add_tab(&first_id).expect("add_tab");
        // tab2 is now active. Focus back to first.
        assert!(model.focus_surface(&first_id));

        let workspace = model.list_workspaces().remove(0);
        assert_eq!(workspace.focused_surface_id, first_id);
        assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&first_id));
    }

    #[test]
    fn repair_session_invariants_clamps_out_of_range_active_index() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let first_id = workspace.focused_surface_id.clone();
        {
            let ws = model.workspaces.get_mut(&workspace.id).unwrap();
            // Force an invalid active index.
            ws.pane_tree = PaneNode::Leaf {
                tabs: vec![first_id.clone()],
                active: 99,
            };
        }

        assert!(model.repair_session_invariants());

        let ws = model.workspaces.get(&workspace.id).unwrap();
        let PaneNode::Leaf { active, .. } = ws.pane_tree else {
            panic!("expected leaf");
        };
        assert_eq!(active, 0);
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
    }

    #[test]
    fn single_leaf_helper_creates_leaf_with_one_tab() {
        let leaf = PaneNode::single_leaf("surface-42".to_string());
        let tabs = leaf.leaf_tabs().expect("leaf");
        assert_eq!(tabs, &["surface-42"]);
        assert_eq!(leaf.leaf_active_id(), Some(&"surface-42".to_string()));
    }
}
