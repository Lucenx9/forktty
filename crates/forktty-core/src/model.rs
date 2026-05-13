use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
    next_workspace: u64,
    next_surface: u64,
    next_notification: u64,
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

    pub fn select_workspace(&mut self, selector: WorkspaceSelector<'_>) -> Option<Workspace> {
        let id = self.resolve_workspace_id(selector)?;
        for workspace in self.workspaces.values_mut() {
            workspace.active = workspace.id == id;
        }
        self.workspaces.get(&id).cloned()
    }

    pub fn close_workspace(&mut self, selector: WorkspaceSelector<'_>) -> Option<Workspace> {
        let id = self.resolve_workspace_id(selector)?;
        let removed = self.workspaces.remove(&id)?;
        self.workspace_order.retain(|candidate| candidate != &id);
        self.surfaces
            .retain(|_, surface| surface.workspace_id != removed.id);
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

    pub fn focus_surface(&mut self, surface_id: &str) -> bool {
        let Some(surface) = self.surfaces.get(surface_id) else {
            return false;
        };
        let Some(workspace) = self.workspaces.get_mut(&surface.workspace_id) else {
            return false;
        };
        workspace.focused_surface_id = surface_id.to_string();
        true
    }

    pub fn close_surface(&mut self, surface_id: &str) -> Option<Surface> {
        let surface = self.surfaces.get(surface_id)?.clone();
        let replacement_id = self.next_surface_id();
        let workspace = self.workspaces.get_mut(&surface.workspace_id)?;
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
                workspace_id: workspace.id.clone(),
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
        PaneNode::Split { children, .. } => children
            .iter_mut()
            .any(|child| replace_leaf_with_split(child, target_surface_id, axis, new_leaf.clone())),
    }
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
            sizes.remove(index);
            if children.len() == 1 {
                *node = children.remove(0);
            }
            Some(false)
        }
    }
}

fn first_leaf_surface_id(node: &PaneNode) -> Option<SurfaceId> {
    match node {
        PaneNode::Leaf { surface_id } => Some(surface_id.clone()),
        PaneNode::Split { children, .. } => children.iter().find_map(first_leaf_surface_id),
    }
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
    fn can_update_surface_title() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");

        assert!(model.set_surface_title(&workspace.focused_surface_id, "build"));

        assert_eq!(
            model.surface(&workspace.focused_surface_id).unwrap().title,
            "build"
        );
    }
}
