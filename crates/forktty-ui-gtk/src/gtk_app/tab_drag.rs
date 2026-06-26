//! Tab strip drag/drop behavior and pure tab-drop target helpers.

use super::*;

pub(super) fn install_tabstrip_drop_target(
    tabstrip: &gtk::Box,
    tab_widgets: &[gtk::Box],
    surface_ids: &[String],
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
) {
    let strip_for_drop = tabstrip.clone().upcast::<gtk::Widget>();
    let tab_targets = surface_ids
        .iter()
        .cloned()
        .zip(
            tab_widgets
                .iter()
                .map(|tab| tab.clone().upcast::<gtk::Widget>()),
        )
        .collect::<Vec<_>>();

    install_tab_drop_target_on(&strip_for_drop, &strip_for_drop, &tab_targets, model, state);
}

fn install_tab_drop_target_on(
    handle: &gtk::Widget,
    tabstrip: &gtk::Widget,
    tab_targets: &[(String, gtk::Widget)],
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
) {
    let handle_for_drop = handle.clone();
    let strip_for_drop = tabstrip.clone();
    let tab_targets = tab_targets.to_vec();
    let tab_order = tab_targets
        .iter()
        .map(|(surface_id, _)| surface_id.clone())
        .collect::<Vec<_>>();
    let tab_targets_for_drop = tab_targets.clone();
    let tab_order_for_drop = tab_order.clone();
    let tab_targets_for_motion = tab_targets.clone();
    let tab_order_for_motion = tab_order.clone();
    let handle_for_motion = handle.clone();
    let strip_for_motion = tabstrip.clone();
    let target = tab_drop_target(move |source_id, x, y| {
        clear_tab_drop_indicators(&tab_targets_for_drop);
        let Some((strip_x, _)) = handle_for_drop.translate_coordinates(&strip_for_drop, x, y)
        else {
            return false;
        };
        let midpoints = tab_drop_midpoints(&strip_for_drop, &tab_targets_for_drop);
        let Some((target_id, position)) =
            tab_drop_target_at_x(&midpoints, strip_x).filter(|(target_id, position)| {
                !tab_move_would_keep_order(&tab_order_for_drop, &source_id, target_id, *position)
            })
        else {
            return false;
        };
        let moved = model
            .lock()
            .ok()
            .is_some_and(|mut model| model.move_tab(&source_id, target_id, position));
        if moved {
            if let Some(state) = state.as_ref() {
                save_session_from_state(state);
            }
        }
        moved
    });
    target.set_preload(true);
    target.connect_motion(move |target, x, y| {
        let Some(source_id) = target
            .value()
            .and_then(|value| tab_dnd_id_from_value(&value))
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::MOVE;
        };
        let Some((strip_x, _)) = handle_for_motion.translate_coordinates(&strip_for_motion, x, y)
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::empty();
        };
        let midpoints = tab_drop_midpoints(&strip_for_motion, &tab_targets_for_motion);
        let Some((target_id, position)) =
            tab_drop_target_at_x(&midpoints, strip_x).filter(|(target_id, position)| {
                !tab_move_would_keep_order(&tab_order_for_motion, &source_id, target_id, *position)
            })
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::empty();
        };
        set_tab_drop_indicator(&tab_targets_for_motion, target_id, position);
        gdk::DragAction::MOVE
    });
    let tab_targets_for_leave = tab_targets.clone();
    target.connect_leave(move |_| {
        clear_tab_drop_indicators(&tab_targets_for_leave);
    });
    handle.add_controller(target);
}

fn tab_drop_midpoints(
    tabstrip: &gtk::Widget,
    tab_targets: &[(String, gtk::Widget)],
) -> Vec<(String, f64)> {
    tab_targets
        .iter()
        .filter_map(|(surface_id, tab)| {
            let (tab_x, _) = tab.translate_coordinates(tabstrip, 0.0, 0.0)?;
            Some((
                surface_id.clone(),
                tab_x + f64::from(tab.allocated_width()) / 2.0,
            ))
        })
        .collect()
}

pub(super) fn tab_drop_target_at_x(
    tab_midpoints: &[(String, f64)],
    x: f64,
) -> Option<(&str, forktty_core::MovePosition)> {
    for (surface_id, midpoint) in tab_midpoints {
        if x < *midpoint {
            return Some((surface_id.as_str(), forktty_core::MovePosition::Before));
        }
    }
    tab_midpoints
        .last()
        .map(|(surface_id, _)| (surface_id.as_str(), forktty_core::MovePosition::After))
}

pub(super) fn tab_move_would_keep_order(
    tab_order: &[String],
    source_id: &str,
    target_id: &str,
    position: forktty_core::MovePosition,
) -> bool {
    if source_id == target_id {
        return true;
    }
    let source_index = tab_order
        .iter()
        .position(|surface_id| surface_id == source_id);
    let target_index = tab_order
        .iter()
        .position(|surface_id| surface_id == target_id);
    matches!(
        (source_index, target_index, position),
        (Some(source), Some(target), forktty_core::MovePosition::Before)
            if source + 1 == target
    ) || matches!(
        (source_index, target_index, position),
        (Some(source), Some(target), forktty_core::MovePosition::After)
            if target + 1 == source
    )
}

fn clear_tab_drop_indicators(tab_targets: &[(String, gtk::Widget)]) {
    for (_, tab) in tab_targets {
        tab.remove_css_class("drop-before");
        tab.remove_css_class("drop-after");
    }
}

fn set_tab_drop_indicator(
    tab_targets: &[(String, gtk::Widget)],
    target_id: &str,
    position: forktty_core::MovePosition,
) {
    clear_tab_drop_indicators(tab_targets);
    let Some((_, tab)) = tab_targets
        .iter()
        .find(|(surface_id, _)| surface_id == target_id)
    else {
        return;
    };
    match position {
        forktty_core::MovePosition::Before => tab.add_css_class("drop-before"),
        forktty_core::MovePosition::After => tab.add_css_class("drop-after"),
    }
}
