//! Pane chrome and tab-strip refresh signatures.

use super::*;

pub(super) struct TabStripRefresh {
    pub(super) active_id: Option<String>,
    pub(super) tabs: Vec<TabRefresh>,
}

pub(super) struct TabRefresh {
    pub(super) title: String,
    pub(super) active: bool,
}

pub(super) fn tab_strip_refreshes(
    model: &WorkspaceModel,
    strip_tabs: &[Vec<String>],
) -> Vec<TabStripRefresh> {
    let workspace = model.active_workspace();
    strip_tabs
        .iter()
        .map(|tabs| {
            let active_id = workspace
                .as_ref()
                .and_then(|workspace| active_tab_for_tabs(&workspace.pane_tree, tabs));
            let tabs = tabs
                .iter()
                .map(|surface_id| {
                    let title = model
                        .surface(surface_id)
                        .map(surface_title)
                        .unwrap_or_else(|| "Terminal".to_string());
                    let active = active_id.as_deref() == Some(surface_id.as_str());
                    TabRefresh { title, active }
                })
                .collect();
            TabStripRefresh { active_id, tabs }
        })
        .collect()
}

pub(super) fn chrome_refresh_signature(
    model: &WorkspaceModel,
    chrome_surface_ids: &[String],
    strip_tabs: &[Vec<String>],
) -> String {
    let focused_surface_id = model
        .active_workspace()
        .map(|workspace| workspace.focused_surface_id);
    let mut signature = String::new();
    signature.push_str("focus=");
    if let Some(focused_surface_id) = focused_surface_id.as_deref() {
        signature.push_str(focused_surface_id);
    }
    signature.push('\n');

    for surface_id in chrome_surface_ids {
        signature.push_str("chrome=");
        signature.push_str(surface_id);
        if let Some(surface) = model.surface(surface_id) {
            signature.push('|');
            signature.push_str(&surface_title(surface));
            signature.push('|');
            signature.push_str(&surface.cwd.to_string_lossy());
            signature.push('|');
            signature.push_str(if surface.unread { "u1" } else { "u0" });
            signature.push('|');
            signature.push_str(if surface.needs_attention { "a1" } else { "a0" });
            signature.push('|');
            signature.push_str(if surface.agent_session.is_some() {
                "agent1"
            } else {
                "agent0"
            });
            if let forktty_core::SurfaceKind::Browser { url, .. } = &surface.kind {
                signature.push_str("|browser=");
                signature.push_str(url);
            }
        }
        signature.push('\n');
    }

    let browser_surfaces =
        model
            .list_surfaces(None)
            .into_iter()
            .filter_map(|surface| match surface.kind {
                forktty_core::SurfaceKind::Browser { url, .. } => Some((surface.id, url)),
                _ => None,
            });
    for (surface_id, url) in browser_surfaces {
        signature.push_str("browser=");
        signature.push_str(&surface_id);
        signature.push('|');
        signature.push_str(&url);
        signature.push('\n');
    }

    let workspace = model.active_workspace();
    for tabs in strip_tabs {
        signature.push_str("tabs=");
        if let Some(active_id) = workspace
            .as_ref()
            .and_then(|workspace| active_tab_for_tabs(&workspace.pane_tree, tabs))
        {
            signature.push_str(&active_id);
        }
        for surface_id in tabs {
            signature.push('|');
            signature.push_str(surface_id);
            signature.push(':');
            if let Some(surface) = model.surface(surface_id) {
                signature.push_str(&surface_title(surface));
            }
        }
        signature.push('\n');
    }

    signature
}

// In maximize mode only the focused pane is rendered, so the layout signature
// must change when focus moves between panes even though the tree is the same.
pub(super) fn effective_layout_signature(
    signature: &str,
    maximized: bool,
    focused_surface_id: &str,
) -> String {
    if maximized {
        format!("{signature}#max:{focused_surface_id}")
    } else {
        signature.to_string()
    }
}
