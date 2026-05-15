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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PaneNode {
    #[serde(rename = "leaf")]
    Leaf { surface_id: SurfaceId },
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
    progress: BTreeMap<WorkspaceId, Vec<ProgressEntry>>,
    logs: BTreeMap<WorkspaceId, Vec<LogEntry>>,
    next_workspace: u64,
    next_surface: u64,
    next_notification: u64,
    next_log: u64,
}

const MAX_LOG_ENTRIES: usize = 200;

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
        };
        let workspace = Workspace {
            id: id.clone(),
            name: name.into(),
            active: true,
            working_dir,
            git_branch: String::new(),
            worktree_dir: None,
            worktree_name: None,
            pane_tree: PaneNode::Leaf {
                surface_id: surface_id.clone(),
            },
            focused_surface_id: surface_id,
            needs_attention: false,
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
        let mut workspace = self.create_workspace(name, working_dir);
        workspace.git_branch = git_branch.into();
        workspace.worktree_dir = Some(workspace.working_dir.clone());
        workspace.worktree_name = Some(worktree_name.into());
        self.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        workspace
    }

    pub fn restore_session(&mut self, data: SessionData) {
        *self = WorkspaceModel::new();
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
                let surface = Surface {
                    id: surface_id,
                    workspace_id: workspace.id.clone(),
                    cwd: workspace.working_dir.clone(),
                    title: String::from("shell"),
                    unread: false,
                    needs_attention: false,
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
    }

    pub fn to_session_data(&self) -> SessionData {
        let mut workspaces = self.list_workspaces();
        for workspace in &mut workspaces {
            normalize_workspace_focus(workspace);
        }
        SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces,
            active_workspace_id: self.active_workspace_id(),
        }
    }

    pub fn repair_session_invariants(&mut self) -> bool {
        let mut changed = false;
        let mut valid_surface_ids = BTreeSet::new();
        let workspace_ids = self.workspace_order.clone();

        for workspace_id in workspace_ids {
            let Some(workspace) = self.workspaces.get_mut(&workspace_id) else {
                continue;
            };
            let leaf_ids = leaf_surface_ids(&workspace.pane_tree);
            for surface_id in &leaf_ids {
                valid_surface_ids.insert(surface_id.clone());
            }
            if !leaf_ids.contains(&workspace.focused_surface_id) {
                if let Some(first_leaf) = leaf_ids.first() {
                    workspace.focused_surface_id = first_leaf.clone();
                    changed = true;
                }
            }
            for surface_id in leaf_ids {
                if !self.surfaces.contains_key(&surface_id) {
                    self.surfaces.insert(
                        surface_id.clone(),
                        Surface {
                            id: surface_id,
                            workspace_id: workspace.id.clone(),
                            cwd: workspace.working_dir.clone(),
                            title: String::from("shell"),
                            unread: false,
                            needs_attention: false,
                        },
                    );
                    changed = true;
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
        let id = self.resolve_workspace_id(selector)?;
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
        let id = self.resolve_workspace_id(selector)?;
        let workspace = self.workspaces.get_mut(&id)?;
        workspace.name = name.into();
        Some(workspace.clone())
    }

    pub fn close_workspace(&mut self, selector: WorkspaceSelector<'_>) -> Option<Workspace> {
        let id = self.resolve_workspace_id(selector)?;
        let removed = self.workspaces.remove(&id)?;
        self.workspace_order.retain(|candidate| candidate != &id);
        self.surfaces
            .retain(|_, surface| surface.workspace_id != removed.id);
        self.statuses.remove(&removed.id);
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
        let source = self.surfaces.get(surface_id)?.clone();
        let new_id = self.next_surface_id();
        let new_surface = Surface {
            id: new_id.clone(),
            workspace_id: source.workspace_id.clone(),
            cwd: source.cwd.clone(),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
        };
        let workspace = self.workspaces.get_mut(&source.workspace_id)?;
        if replace_leaf_with_split(
            &mut workspace.pane_tree,
            surface_id,
            axis,
            PaneNode::Leaf {
                surface_id: new_id.clone(),
            },
        ) {
            workspace.focused_surface_id = new_id;
            self.surfaces
                .insert(new_surface.id.clone(), new_surface.clone());
            Some(new_surface)
        } else {
            None
        }
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
        true
    }

    pub fn close_surface(&mut self, surface_id: &str) -> Option<Surface> {
        let surface = self.surfaces.get(surface_id)?.clone();
        let replacement_id = self.next_surface_id();
        let workspace_id = surface.workspace_id.clone();
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
            let replacement = Surface {
                id: replacement_id.clone(),
                workspace_id: workspace_id.clone(),
                cwd: workspace.working_dir.clone(),
                title: String::from("shell"),
                unread: false,
                needs_attention: false,
            };
            workspace.pane_tree = PaneNode::Leaf {
                surface_id: replacement_id.clone(),
            };
            workspace.focused_surface_id = replacement_id.clone();
            self.surfaces.insert(replacement_id, replacement);
        }
        let workspace_needs_attention = self
            .surfaces
            .values()
            .any(|candidate| candidate.workspace_id == workspace_id && candidate.unread);
        if let Some(workspace) = self.workspaces.get_mut(&workspace_id) {
            workspace.needs_attention = workspace_needs_attention;
        }
        Some(removed)
    }

    pub fn mark_surface_unread(&mut self, surface_id: &str, unread: bool) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        surface.unread = unread;
        surface.needs_attention = unread;
        if let Some(workspace) = self.workspaces.get_mut(&surface.workspace_id) {
            workspace.needs_attention = self
                .surfaces
                .values()
                .any(|candidate| candidate.workspace_id == workspace.id && candidate.unread);
        }
        true
    }

    pub fn set_surface_title(&mut self, surface_id: &str, title: impl Into<String>) -> bool {
        let Some(surface) = self.surfaces.get_mut(surface_id) else {
            return false;
        };
        surface.title = title.into();
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
            let _ = self.mark_surface_unread(surface_id, true);
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
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let entry = StatusEntry {
            key: key.into(),
            label: label.into(),
            value: value.into(),
            color,
        };
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
        let Some(entries) = self.statuses.get_mut(workspace_id) else {
            return self.workspaces.contains_key(workspace_id);
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
        self.next_workspace += 1;
        format!("workspace-{}", self.next_workspace)
    }

    fn next_surface_id(&mut self) -> SurfaceId {
        self.next_surface += 1;
        format!("surface-{}", self.next_surface)
    }

    fn next_notification_id(&mut self) -> String {
        self.next_notification += 1;
        format!("notification-{}", self.next_notification)
    }

    fn recompute_notification_attention(&mut self) {
        let unread_surface_ids = self
            .notifications
            .iter()
            .filter(|notification| !notification.read)
            .filter_map(|notification| notification.surface_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        for surface in self.surfaces.values_mut() {
            let unread = unread_surface_ids.contains(&surface.id);
            surface.unread = unread;
            surface.needs_attention = unread;
        }
        for workspace in self.workspaces.values_mut() {
            workspace.needs_attention = self.surfaces.values().any(|surface| {
                surface.workspace_id == workspace.id && (surface.unread || surface.needs_attention)
            });
        }
    }

    fn next_log_id(&mut self) -> String {
        self.next_log += 1;
        format!("log-{}", self.next_log)
    }

    fn resolve_workspace_id(&self, selector: WorkspaceSelector<'_>) -> Option<WorkspaceId> {
        match selector {
            WorkspaceSelector::Id(id) => self.workspaces.contains_key(id).then(|| id.to_string()),
            WorkspaceSelector::Name(name) => self
                .workspaces
                .values()
                .find(|workspace| workspace.name == name)
                .map(|workspace| workspace.id.clone()),
            WorkspaceSelector::WorktreeName(name) => self
                .workspaces
                .values()
                .find(|workspace| workspace.worktree_name.as_deref() == Some(name))
                .map(|workspace| workspace.id.clone()),
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
        PaneNode::Leaf { surface_id } if surface_id == target_surface_id => {
            *node = PaneNode::Split {
                axis,
                children: vec![
                    PaneNode::Leaf {
                        surface_id: target_surface_id.to_string(),
                    },
                    new_leaf,
                ],
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
                        PaneNode::Leaf { surface_id } if surface_id == target_surface_id
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
        PaneNode::Leaf { surface_id } => Some(surface_id == target_surface_id),
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
                    Some(false) => {}
                    None => return None,
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

fn first_leaf_surface_id(node: &PaneNode) -> Option<SurfaceId> {
    match node {
        PaneNode::Leaf { surface_id } => Some(surface_id.clone()),
        PaneNode::Split { children, .. } => children.iter().find_map(first_leaf_surface_id),
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
        PaneNode::Leaf { surface_id } => ids.push(surface_id.clone()),
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_leaf_surface_ids(child, ids);
            }
        }
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
    fn focus_surface_rejects_surface_outside_workspace_pane_tree() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let second = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let first = workspace.focused_surface_id;
        model.workspaces.get_mut(&workspace.id).unwrap().pane_tree =
            PaneNode::Leaf { surface_id: first };

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
            workspace.pane_tree = PaneNode::Leaf {
                surface_id: first.clone(),
            };
            workspace.focused_surface_id = second.id.clone();
        }

        assert!(model.repair_session_invariants());
        let repaired = model.list_workspaces().remove(0);
        assert_eq!(repaired.focused_surface_id, first);
        assert!(model.surface(&second.id).is_none());
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
}
