//! Pane tree operations for split panes and tabbed leaves.

use super::{MovePosition, PaneNode, SplitAxis, SurfaceId, Workspace, MAX_SESSION_SPLIT_DEPTH};

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

pub(super) fn replace_leaf_with_split(
    node: &mut PaneNode,
    target_surface_id: &str,
    axis: SplitAxis,
    new_leaf: PaneNode,
) -> Result<(), PaneNode> {
    let mut new_leaf = new_leaf;
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
            Ok(())
        }
        PaneNode::Leaf { .. } => Err(new_leaf),
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
                        return Ok(());
                    }
                    match replace_leaf_with_split(
                        &mut children[index],
                        target_surface_id,
                        axis,
                        new_leaf,
                    ) {
                        Ok(()) => return Ok(()),
                        Err(returned_leaf) => new_leaf = returned_leaf,
                    }
                }
                Err(new_leaf)
            } else {
                for child in children.iter_mut() {
                    match replace_leaf_with_split(child, target_surface_id, axis, new_leaf) {
                        Ok(()) => return Ok(()),
                        Err(returned_leaf) => new_leaf = returned_leaf,
                    }
                }
                Err(new_leaf)
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
pub(super) fn split_would_exceed_depth(node: &PaneNode, surface_id: &str, axis: SplitAxis) -> bool {
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

pub(super) fn remove_leaf(node: &mut PaneNode, target_surface_id: &str) -> Option<bool> {
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

pub(super) fn update_partition_ratio(
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

pub(super) fn move_tab_in_tree(
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

pub(super) fn swap_pane_leaves(
    node: &mut PaneNode,
    source_surface_id: &str,
    target_surface_id: &str,
) -> bool {
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

pub(super) fn repair_pane_tree_structure(node: &mut PaneNode) -> bool {
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

pub(super) fn rename_leaf(node: &mut PaneNode, old_id: &str, new_id: &str) -> bool {
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
pub(super) fn neighbor_leaf_surface_id(node: &PaneNode, surface_id: &str) -> Option<SurfaceId> {
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

pub(super) fn first_leaf_surface_id(node: &PaneNode) -> Option<SurfaceId> {
    match node {
        PaneNode::Leaf { tabs, active } => {
            // Return the active tab if valid, else first.
            tabs.get(*active).or_else(|| tabs.first()).cloned()
        }
        PaneNode::Split { children, .. } => children.iter().find_map(first_leaf_surface_id),
    }
}

pub(super) fn normalize_workspace_focus(workspace: &mut Workspace) {
    if has_leaf_surface_id(&workspace.pane_tree, &workspace.focused_surface_id) {
        return;
    }
    if let Some(first_leaf) = first_leaf_surface_id(&workspace.pane_tree) {
        workspace.focused_surface_id = first_leaf;
    }
}

pub(super) fn has_leaf_surface_id(node: &PaneNode, surface_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => tabs.iter().any(|tab| tab == surface_id),
        PaneNode::Split { children, .. } => children
            .iter()
            .any(|child| has_leaf_surface_id(child, surface_id)),
    }
}

pub(super) fn leaf_surface_ids(node: &PaneNode) -> Vec<SurfaceId> {
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
pub(super) fn remove_tab_from_leaf(
    node: &mut PaneNode,
    target_surface_id: &str,
) -> Option<SurfaceId> {
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
pub(super) fn set_leaf_active_for_surface(node: &mut PaneNode, surface_id: &str) -> bool {
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
/// Sets `active` to the new tab's index. Returns `Ok(())` if found, or `Err(new_tab_id)` if not.
pub(super) fn push_tab_to_leaf(
    node: &mut PaneNode,
    near_surface_id: &str,
    new_tab_id: SurfaceId,
) -> Result<(), SurfaceId> {
    let mut new_tab_id = new_tab_id;
    match node {
        PaneNode::Leaf { tabs, active } => {
            if tabs.iter().any(|id| id == near_surface_id) {
                tabs.push(new_tab_id);
                *active = tabs.len() - 1;
                Ok(())
            } else {
                Err(new_tab_id)
            }
        }
        PaneNode::Split { children, .. } => {
            for child in children.iter_mut() {
                match push_tab_to_leaf(child, near_surface_id, new_tab_id) {
                    Ok(()) => return Ok(()),
                    Err(returned_id) => new_tab_id = returned_id,
                }
            }
            Err(new_tab_id)
        }
    }
}
