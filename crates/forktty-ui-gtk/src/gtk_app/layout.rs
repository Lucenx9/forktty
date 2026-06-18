use super::*;

pub(super) fn build_split_widget<F>(
    orientation: gtk::Orientation,
    children: &[PaneNode],
    sizes: &[f64],
    on_resize: SplitResizeCallback,
    build: F,
) -> gtk::Widget
where
    F: Fn(&PaneNode) -> gtk::Widget + Clone,
{
    if children.is_empty() {
        return missing_surface_placeholder("unknown", None, None).upcast();
    }
    if children.len() == 1 {
        return build(&children[0]);
    }

    let weights = normalized_split_sizes(sizes, children.len());
    let split_at = weighted_split_index(&weights);
    let start_weight: f64 = weights[..split_at].iter().sum();
    let end_weight: f64 = weights[split_at..].iter().sum();
    let total_weight = start_weight + end_weight;
    let ratio = if total_weight > f64::EPSILON {
        start_weight / total_weight
    } else {
        0.5
    };

    let paned = gtk::Paned::new(orientation);
    configure_terminal_paned(&paned);

    let start = build_split_widget(
        orientation,
        &children[..split_at],
        &weights[..split_at],
        on_resize.clone(),
        build.clone(),
    );
    let end = build_split_widget(
        orientation,
        &children[split_at..],
        &weights[split_at..],
        on_resize.clone(),
        build,
    );
    paned.set_start_child(Some(&start));
    paned.set_end_child(Some(&end));

    let left_leaves: Vec<String> = children[..split_at]
        .iter()
        .flat_map(collect_leaves)
        .collect();
    let right_leaves: Vec<String> = children[split_at..]
        .iter()
        .flat_map(collect_leaves)
        .collect();
    let ready = Rc::new(Cell::new(false));
    schedule_paned_ratio(&paned, orientation, ratio, ready.clone());
    let resize_cb = on_resize;
    let ready_for_notify = ready;
    paned.connect_position_notify(move |paned| {
        if !ready_for_notify.get() {
            return;
        }
        let min = paned.min_position();
        let max = paned.max_position();
        if max <= min {
            return;
        }
        let pos = paned.position();
        let new_ratio = ((pos - min) as f64 / (max - min) as f64).clamp(0.01, 0.99);
        resize_cb(&left_leaves, &right_leaves, new_ratio);
    });

    paned.upcast()
}

pub(super) fn collect_leaves(node: &PaneNode) -> Vec<String> {
    let mut ids = Vec::new();
    collect_leaves_into(node, &mut ids);
    ids
}

pub(super) fn collect_leaves_into(node: &PaneNode, ids: &mut Vec<String>) {
    match node {
        PaneNode::Leaf { tabs, .. } => {
            ids.extend(tabs.iter().cloned());
        }
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_leaves_into(child, ids);
            }
        }
    }
}

/// One representative surface id per pane (leaf), using the leaf's active tab.
/// Unlike `collect_leaves`, this counts panes — not the tabs inside them — so
/// pane navigation and "Pane X/Y" labels are not skewed by multi-tab leaves.
pub(super) fn collect_panes(node: &PaneNode) -> Vec<String> {
    let mut ids = Vec::new();
    collect_panes_into(node, &mut ids);
    ids
}

pub(super) fn collect_panes_into(node: &PaneNode, ids: &mut Vec<String>) {
    match node {
        PaneNode::Leaf { tabs, active } => {
            if let Some(id) = tabs.get(*active).or_else(|| tabs.first()) {
                ids.push(id.clone());
            }
        }
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_panes_into(child, ids);
            }
        }
    }
}

pub(super) fn configure_terminal_paned(paned: &gtk::Paned) {
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_wide_handle(true);
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(true);
    paned.set_shrink_start_child(true);
    paned.set_shrink_end_child(true);
    paned.set_overflow(gtk::Overflow::Hidden);
}

pub(super) fn normalized_split_sizes(sizes: &[f64], len: usize) -> Vec<f64> {
    if len == 0 {
        return Vec::new();
    }

    let mut weights: Vec<f64> = sizes
        .iter()
        .take(len)
        .map(|size| {
            if size.is_finite() && *size > 0.0 {
                *size
            } else {
                0.0
            }
        })
        .collect();
    if weights.len() < len {
        weights.resize(len, 0.0);
    }

    let positive_total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    let missing = weights.iter().filter(|w| **w <= 0.0).count();
    if positive_total <= f64::EPSILON {
        let share = 1.0 / len as f64;
        weights.fill(share);
        return weights;
    }

    if missing > 0 {
        let positive_count = len - missing;
        let avg_positive = positive_total / positive_count as f64;
        for weight in &mut weights {
            if *weight <= 0.0 {
                *weight = avg_positive;
            }
        }
    }

    let total: f64 = weights.iter().sum();
    if total > f64::EPSILON {
        for weight in &mut weights {
            *weight /= total;
        }
    } else {
        weights.fill(1.0 / len as f64);
    }
    weights
}

pub(super) fn weighted_split_index(weights: &[f64]) -> usize {
    if weights.len() <= 1 {
        return 1;
    }
    let middle = weights.len() as f64 / 2.0;
    let mut best_index = 1usize;
    let mut best_delta = f64::MAX;
    let mut best_distance_to_middle = f64::MAX;
    let target = weights.iter().sum::<f64>() / 2.0;
    let mut prefix = 0.0;

    for index in 1..weights.len() {
        prefix += weights[index - 1];
        let delta = (prefix - target).abs();
        let distance = (index as f64 - middle).abs();
        let better_delta = delta + 1e-9 < best_delta;
        let tied_delta = (delta - best_delta).abs() <= 1e-9;
        if better_delta || (tied_delta && distance < best_distance_to_middle) {
            best_delta = delta;
            best_index = index;
            best_distance_to_middle = distance;
        }
    }

    best_index
}

pub(super) fn schedule_paned_ratio(
    paned: &gtk::Paned,
    orientation: gtk::Orientation,
    ratio: f64,
    ready: Rc<Cell<bool>>,
) {
    let ratio = ratio.clamp(0.05, 0.95);
    let attempts = Rc::new(Cell::new(0_u8));

    paned.add_tick_callback(move |paned, _| {
        let attempt = attempts.get().saturating_add(1);
        attempts.set(attempt);
        if paned.root().is_none() {
            ready.set(true);
            return glib::ControlFlow::Break;
        }
        let applied = apply_paned_ratio(paned, orientation, ratio);
        let done =
            (applied && attempt >= PANED_RATIO_APPLY_FRAMES) || attempt >= PANED_RATIO_MAX_FRAMES;
        if done {
            ready.set(true);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

pub(super) fn apply_paned_ratio(
    paned: &gtk::Paned,
    orientation: gtk::Orientation,
    ratio: f64,
) -> bool {
    let span = match orientation {
        gtk::Orientation::Horizontal => paned.allocated_width(),
        gtk::Orientation::Vertical => paned.allocated_height(),
        _ => 0,
    };
    if span <= 1 {
        return false;
    }

    let min = paned.min_position();
    let max = paned.max_position();
    let position = if max > min {
        min + ((max - min) as f64 * ratio).round() as i32
    } else {
        (span as f64 * ratio).round() as i32
    };
    paned.set_position(position.max(1));
    true
}

pub(super) fn queue_widget_focus(widget: gtk::Widget) {
    glib::idle_add_local_once(move || {
        if widget.root().is_some() {
            widget.grab_focus();
        }
    });
}

pub(super) fn queue_focusable_descendant_focus_when(
    widget: gtk::Widget,
    should_focus: Rc<dyn Fn() -> bool>,
) {
    for delay_ms in [0, 16, 50, 150] {
        let widget = widget.downgrade();
        let should_focus = Rc::clone(&should_focus);
        glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
            let Some(widget) = widget.upgrade() else {
                return;
            };
            if widget.root().is_some() && should_focus() {
                let _ = grab_focusable_descendant(&widget);
            }
        });
    }
}

fn grab_focusable_descendant(widget: &gtk::Widget) -> bool {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if grab_focusable_descendant(&current) {
            return true;
        }
        child = current.next_sibling();
    }
    widget.grab_focus()
}

pub(super) fn detach_widget(widget: &gtk::Widget) {
    let Some(parent) = widget.parent() else {
        return;
    };
    if let Ok(paned) = parent.clone().downcast::<gtk::Paned>() {
        if paned
            .start_child()
            .as_ref()
            .is_some_and(|child| child == widget)
        {
            paned.set_start_child(None::<&gtk::Widget>);
        }
        if paned
            .end_child()
            .as_ref()
            .is_some_and(|child| child == widget)
        {
            paned.set_end_child(None::<&gtk::Widget>);
        }
    } else if let Ok(container) = parent.clone().downcast::<gtk::Box>() {
        container.remove(widget);
    } else if let Ok(stack) = parent.clone().downcast::<gtk::Stack>() {
        stack.remove(widget);
    } else {
        widget.unparent();
    }
}
pub(super) fn active_layout_snapshot(
    model: &Arc<Mutex<WorkspaceModel>>,
) -> Option<(String, PaneNode, String, String)> {
    let model = model.lock().ok()?;
    let workspace = model.active_workspace()?;
    let mut structure = String::new();
    layout_structure_signature(&workspace.pane_tree, &mut structure);
    let signature = format!("{}:{structure}", workspace.id);
    Some((
        signature,
        workspace.pane_tree,
        workspace.focused_surface_id,
        workspace.id,
    ))
}

pub(super) fn layout_structure_signature(node: &PaneNode, out: &mut String) {
    match node {
        PaneNode::Leaf { tabs, .. } => {
            out.push_str("L(");
            for (i, id) in tabs.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                out.push_str(id);
            }
            out.push(')');
        }
        PaneNode::Split { axis, children, .. } => {
            out.push_str("S(");
            out.push_str(match axis {
                SplitAxis::Horizontal => "h",
                SplitAxis::Vertical => "v",
            });
            for child in children {
                out.push(',');
                layout_structure_signature(child, out);
            }
            out.push(')');
        }
    }
}

pub(super) fn active_tab_for_tabs(node: &PaneNode, target_tabs: &[String]) -> Option<String> {
    match node {
        PaneNode::Leaf { tabs, active } if tabs.as_slice() == target_tabs => {
            tabs.get(*active).or_else(|| tabs.first()).cloned()
        }
        PaneNode::Leaf { .. } => None,
        PaneNode::Split { children, .. } => children
            .iter()
            .find_map(|child| active_tab_for_tabs(child, target_tabs)),
    }
}

pub(super) fn active_tab_index_for_leaf(tabs: &[String], active: usize) -> Option<usize> {
    if tabs.is_empty() {
        None
    } else {
        Some(active.min(tabs.len() - 1))
    }
}
