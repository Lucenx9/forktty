//! Model change events for the socket `events.subscribe` stream.
//!
//! The socket server keeps a [`Snapshot`] of the parts of the
//! [`WorkspaceModel`](crate::model::WorkspaceModel) that external clients care
//! about, and on a fixed tick diffs the new snapshot against the previous one
//! with [`diff`]. Each difference becomes one [`ModelEvent`], serialized as a
//! single newline-delimited JSON object on the wire.
//!
//! [`diff`] is pure and source-agnostic: it does not care whether a change came
//! from the GTK UI or from a socket request, which is why it captures both
//! without instrumenting any mutation site.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::WorkspaceModel;

/// A single observed change to the workspace model.
///
/// Serializes with an `"event"` tag, e.g. `{"event":"workspace_added","id":"w1","name":"main"}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ModelEvent {
    WorkspaceAdded {
        id: String,
        name: String,
    },
    WorkspaceRemoved {
        id: String,
    },
    /// The active workspace changed. `id` is `None` when nothing is active.
    WorkspaceSelected {
        id: Option<String>,
    },
    SurfaceAdded {
        id: String,
        workspace_id: String,
    },
    SurfaceRemoved {
        id: String,
    },
    /// A workspace's focused surface changed.
    SurfaceFocused {
        workspace_id: String,
        surface_id: String,
    },
    SurfaceTitleChanged {
        id: String,
        title: String,
    },
    /// A status entry changed. `value` is `None` when the key was cleared.
    StatusChanged {
        workspace_id: String,
        key: String,
        value: Option<String>,
    },
    /// A progress entry changed. `value` is `None` when the key was cleared.
    ProgressChanged {
        workspace_id: String,
        key: String,
        value: Option<f64>,
        total: Option<f64>,
    },
    /// A notification id not seen in the previous snapshot.
    NotificationAdded {
        id: String,
        workspace_id: Option<String>,
        title: String,
        body: String,
    },
    PortsChanged {
        workspace_id: String,
        ports: Vec<u16>,
    },
    /// Linked PR summary changed. `pr` is `None` when there is no linked PR.
    PrChanged {
        workspace_id: String,
        pr: Option<String>,
    },
}

/// Per-workspace fields tracked for diffing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WsSnap {
    pub name: String,
    pub focused_surface_id: String,
    pub status: BTreeMap<String, String>,
    pub progress: BTreeMap<String, (f64, Option<f64>)>,
    pub ports: Vec<u16>,
    pub pr: Option<String>,
}

/// Per-surface fields tracked for diffing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfSnap {
    pub workspace_id: String,
    pub title: String,
}

/// Per-notification fields tracked for edge-triggered emission.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NotifSnap {
    pub workspace_id: Option<String>,
    pub title: String,
    pub body: String,
}

/// Compact, owned view of the model used to compute [`ModelEvent`]s.
///
/// Holds only the fields the events depend on, keyed by id, so diffing is cheap
/// and the lock held while building it (in [`snapshot`]) is short.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub active_workspace_id: Option<String>,
    pub workspaces: BTreeMap<String, WsSnap>,
    pub surfaces: BTreeMap<String, SurfSnap>,
    pub notifications: BTreeMap<String, NotifSnap>,
}

/// Build a [`Snapshot`] from the current model. Call under the model lock.
pub fn snapshot(model: &WorkspaceModel) -> Snapshot {
    let mut workspaces = BTreeMap::new();
    let mut surfaces = BTreeMap::new();
    for workspace in model.list_workspaces() {
        let mut status = BTreeMap::new();
        for entry in model.list_status(&workspace.id) {
            status.insert(entry.key, entry.value);
        }
        let mut progress = BTreeMap::new();
        for entry in model.list_progress(&workspace.id) {
            progress.insert(entry.key, (entry.value, entry.total));
        }
        for surface in model.list_surfaces(Some(&workspace.id)) {
            surfaces.insert(
                surface.id,
                SurfSnap {
                    workspace_id: surface.workspace_id,
                    title: surface.title,
                },
            );
        }
        workspaces.insert(
            workspace.id,
            WsSnap {
                name: workspace.name,
                focused_surface_id: workspace.focused_surface_id,
                status,
                progress,
                ports: workspace.listening_ports,
                pr: workspace.pr.map(|pr| pr.summary()),
            },
        );
    }
    let notifications = model
        .list_notifications()
        .into_iter()
        .map(|item| {
            (
                item.id,
                NotifSnap {
                    workspace_id: item.workspace_id,
                    title: item.title,
                    body: item.body,
                },
            )
        })
        .collect();
    Snapshot {
        active_workspace_id: model.active_workspace_id(),
        workspaces,
        surfaces,
        notifications,
    }
}

/// Compute the events that take `prev` to `next`.
///
/// Order is deterministic: workspace removes, workspace adds, surface removes,
/// surface adds, then per-workspace field changes (selected, focus, title,
/// status, progress, ports, pr), then new notifications.
pub fn diff(prev: &Snapshot, next: &Snapshot) -> Vec<ModelEvent> {
    let mut events = Vec::new();

    // Workspace removes, then adds.
    for id in prev.workspaces.keys() {
        if !next.workspaces.contains_key(id) {
            events.push(ModelEvent::WorkspaceRemoved { id: id.clone() });
        }
    }
    for (id, ws) in &next.workspaces {
        if !prev.workspaces.contains_key(id) {
            events.push(ModelEvent::WorkspaceAdded {
                id: id.clone(),
                name: ws.name.clone(),
            });
        }
    }

    // Surface removes, then adds.
    for id in prev.surfaces.keys() {
        if !next.surfaces.contains_key(id) {
            events.push(ModelEvent::SurfaceRemoved { id: id.clone() });
        }
    }
    for (id, surf) in &next.surfaces {
        if !prev.surfaces.contains_key(id) {
            events.push(ModelEvent::SurfaceAdded {
                id: id.clone(),
                workspace_id: surf.workspace_id.clone(),
            });
        }
    }

    // Active workspace selection.
    if prev.active_workspace_id != next.active_workspace_id {
        events.push(ModelEvent::WorkspaceSelected {
            id: next.active_workspace_id.clone(),
        });
    }

    // Per-workspace field changes. Workspaces new in `next` are diffed against a
    // default so replay (and same-tick creation) still advertises their metadata.
    let default_ws = WsSnap::default();
    for (id, next_ws) in &next.workspaces {
        let prev_ws = prev.workspaces.get(id).unwrap_or(&default_ws);
        if prev_ws.focused_surface_id != next_ws.focused_surface_id {
            events.push(ModelEvent::SurfaceFocused {
                workspace_id: id.clone(),
                surface_id: next_ws.focused_surface_id.clone(),
            });
        }
        for event in diff_status(id, &prev_ws.status, &next_ws.status) {
            events.push(event);
        }
        for event in diff_progress(id, &prev_ws.progress, &next_ws.progress) {
            events.push(event);
        }
        if prev_ws.ports != next_ws.ports {
            events.push(ModelEvent::PortsChanged {
                workspace_id: id.clone(),
                ports: next_ws.ports.clone(),
            });
        }
        if prev_ws.pr != next_ws.pr {
            events.push(ModelEvent::PrChanged {
                workspace_id: id.clone(),
                pr: next_ws.pr.clone(),
            });
        }
    }

    // Surface title changes. New surfaces are diffed against an empty title so a
    // titled surface advertises its title on replay too.
    let default_surf = SurfSnap::default();
    for (id, next_surf) in &next.surfaces {
        let prev_surf = prev.surfaces.get(id).unwrap_or(&default_surf);
        if prev_surf.title != next_surf.title {
            events.push(ModelEvent::SurfaceTitleChanged {
                id: id.clone(),
                title: next_surf.title.clone(),
            });
        }
    }

    // New notifications (edge-triggered on id).
    for (id, notif) in &next.notifications {
        if !prev.notifications.contains_key(id) {
            events.push(ModelEvent::NotificationAdded {
                id: id.clone(),
                workspace_id: notif.workspace_id.clone(),
                title: notif.title.clone(),
                body: notif.body.clone(),
            });
        }
    }

    events
}

fn diff_status(
    workspace_id: &str,
    prev: &BTreeMap<String, String>,
    next: &BTreeMap<String, String>,
) -> Vec<ModelEvent> {
    let mut events = Vec::new();
    for (key, value) in next {
        if prev.get(key) != Some(value) {
            events.push(ModelEvent::StatusChanged {
                workspace_id: workspace_id.to_string(),
                key: key.clone(),
                value: Some(value.clone()),
            });
        }
    }
    for key in prev.keys() {
        if !next.contains_key(key) {
            events.push(ModelEvent::StatusChanged {
                workspace_id: workspace_id.to_string(),
                key: key.clone(),
                value: None,
            });
        }
    }
    events
}

fn diff_progress(
    workspace_id: &str,
    prev: &BTreeMap<String, (f64, Option<f64>)>,
    next: &BTreeMap<String, (f64, Option<f64>)>,
) -> Vec<ModelEvent> {
    let mut events = Vec::new();
    for (key, value) in next {
        if prev.get(key) != Some(value) {
            events.push(ModelEvent::ProgressChanged {
                workspace_id: workspace_id.to_string(),
                key: key.clone(),
                value: Some(value.0),
                total: value.1,
            });
        }
    }
    for key in prev.keys() {
        if !next.contains_key(key) {
            events.push(ModelEvent::ProgressChanged {
                workspace_id: workspace_id.to_string(),
                key: key.clone(),
                value: None,
                total: None,
            });
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(name: &str, focus: &str) -> WsSnap {
        WsSnap {
            name: name.to_string(),
            focused_surface_id: focus.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn no_change_yields_no_events() {
        let mut snap = Snapshot::default();
        snap.workspaces.insert("w1".into(), ws("main", "s1"));
        assert_eq!(diff(&snap, &snap.clone()), vec![]);
    }

    #[test]
    fn workspace_added_and_removed() {
        let mut prev = Snapshot::default();
        prev.workspaces.insert("w1".into(), ws("main", "s1"));
        let mut next = Snapshot::default();
        next.workspaces.insert("w2".into(), ws("other", "s2"));
        let events = diff(&prev, &next);
        assert!(events.contains(&ModelEvent::WorkspaceRemoved { id: "w1".into() }));
        assert!(events.contains(&ModelEvent::WorkspaceAdded {
            id: "w2".into(),
            name: "other".into(),
        }));
        // Removes precede adds.
        let remove_idx = events
            .iter()
            .position(|e| matches!(e, ModelEvent::WorkspaceRemoved { .. }))
            .unwrap();
        let add_idx = events
            .iter()
            .position(|e| matches!(e, ModelEvent::WorkspaceAdded { .. }))
            .unwrap();
        assert!(remove_idx < add_idx);
    }

    #[test]
    fn workspace_selected_changes() {
        let mut prev = Snapshot::default();
        prev.active_workspace_id = Some("w1".into());
        let mut next = prev.clone();
        next.active_workspace_id = Some("w2".into());
        assert_eq!(
            diff(&prev, &next),
            vec![ModelEvent::WorkspaceSelected {
                id: Some("w2".into())
            }]
        );
    }

    #[test]
    fn surface_added_removed_focused_titled() {
        let mut prev = Snapshot::default();
        prev.workspaces.insert("w1".into(), ws("main", "s1"));
        prev.surfaces.insert(
            "s1".into(),
            SurfSnap {
                workspace_id: "w1".into(),
                title: "old".into(),
            },
        );
        let mut next = prev.clone();
        // focus moves to s2, s1 retitled, s2 added, (s1 stays)
        next.workspaces.get_mut("w1").unwrap().focused_surface_id = "s2".into();
        next.surfaces.get_mut("s1").unwrap().title = "new".into();
        next.surfaces.insert(
            "s2".into(),
            SurfSnap {
                workspace_id: "w1".into(),
                title: "two".into(),
            },
        );
        let events = diff(&prev, &next);
        assert!(events.contains(&ModelEvent::SurfaceAdded {
            id: "s2".into(),
            workspace_id: "w1".into(),
        }));
        assert!(events.contains(&ModelEvent::SurfaceFocused {
            workspace_id: "w1".into(),
            surface_id: "s2".into(),
        }));
        assert!(events.contains(&ModelEvent::SurfaceTitleChanged {
            id: "s1".into(),
            title: "new".into(),
        }));
    }

    #[test]
    fn surface_removed() {
        let mut prev = Snapshot::default();
        prev.surfaces.insert(
            "s1".into(),
            SurfSnap {
                workspace_id: "w1".into(),
                title: "t".into(),
            },
        );
        let next = Snapshot::default();
        assert_eq!(
            diff(&prev, &next),
            vec![ModelEvent::SurfaceRemoved { id: "s1".into() }]
        );
    }

    #[test]
    fn status_set_changed_and_cleared() {
        let mut prev = Snapshot::default();
        prev.workspaces.insert("w1".into(), ws("main", "s1"));
        let mut next = prev.clone();
        // add key "a", change nothing else
        next.workspaces
            .get_mut("w1")
            .unwrap()
            .status
            .insert("a".into(), "1".into());
        assert_eq!(
            diff(&prev, &next),
            vec![ModelEvent::StatusChanged {
                workspace_id: "w1".into(),
                key: "a".into(),
                value: Some("1".into()),
            }]
        );
        // now clear it
        let cleared = diff(&next, &prev);
        assert_eq!(
            cleared,
            vec![ModelEvent::StatusChanged {
                workspace_id: "w1".into(),
                key: "a".into(),
                value: None,
            }]
        );
    }

    #[test]
    fn progress_changed() {
        let mut prev = Snapshot::default();
        prev.workspaces.insert("w1".into(), ws("main", "s1"));
        let mut next = prev.clone();
        next.workspaces
            .get_mut("w1")
            .unwrap()
            .progress
            .insert("build".into(), (0.5, Some(1.0)));
        assert_eq!(
            diff(&prev, &next),
            vec![ModelEvent::ProgressChanged {
                workspace_id: "w1".into(),
                key: "build".into(),
                value: Some(0.5),
                total: Some(1.0),
            }]
        );
    }

    #[test]
    fn new_workspace_replay_includes_metadata() {
        // Replay diffs against an empty snapshot; a freshly observed workspace
        // must still advertise its focus/status/progress/ports/pr, not just the
        // bare workspace_added.
        let prev = Snapshot::default();
        let mut next = Snapshot::default();
        let mut w = ws("main", "s1");
        w.status.insert("build".into(), "ok".into());
        w.progress.insert("dl".into(), (0.5, Some(1.0)));
        w.ports = vec![3000];
        w.pr = Some("#1 OPEN".into());
        next.workspaces.insert("w1".into(), w);
        let events = diff(&prev, &next);
        assert!(events.contains(&ModelEvent::WorkspaceAdded {
            id: "w1".into(),
            name: "main".into(),
        }));
        assert!(events.contains(&ModelEvent::SurfaceFocused {
            workspace_id: "w1".into(),
            surface_id: "s1".into(),
        }));
        assert!(events.contains(&ModelEvent::StatusChanged {
            workspace_id: "w1".into(),
            key: "build".into(),
            value: Some("ok".into()),
        }));
        assert!(events.contains(&ModelEvent::ProgressChanged {
            workspace_id: "w1".into(),
            key: "dl".into(),
            value: Some(0.5),
            total: Some(1.0),
        }));
        assert!(events.contains(&ModelEvent::PortsChanged {
            workspace_id: "w1".into(),
            ports: vec![3000],
        }));
        assert!(events.contains(&ModelEvent::PrChanged {
            workspace_id: "w1".into(),
            pr: Some("#1 OPEN".into()),
        }));
    }

    #[test]
    fn new_surface_replay_includes_title() {
        let prev = Snapshot::default();
        let mut next = Snapshot::default();
        next.surfaces.insert(
            "s1".into(),
            SurfSnap {
                workspace_id: "w1".into(),
                title: "editor".into(),
            },
        );
        let events = diff(&prev, &next);
        assert!(events.contains(&ModelEvent::SurfaceAdded {
            id: "s1".into(),
            workspace_id: "w1".into(),
        }));
        assert!(events.contains(&ModelEvent::SurfaceTitleChanged {
            id: "s1".into(),
            title: "editor".into(),
        }));
    }

    #[test]
    fn ports_and_pr_changed() {
        let mut prev = Snapshot::default();
        prev.workspaces.insert("w1".into(), ws("main", "s1"));
        let mut next = prev.clone();
        next.workspaces.get_mut("w1").unwrap().ports = vec![8080];
        next.workspaces.get_mut("w1").unwrap().pr = Some("#7 OPEN".into());
        let events = diff(&prev, &next);
        assert!(events.contains(&ModelEvent::PortsChanged {
            workspace_id: "w1".into(),
            ports: vec![8080],
        }));
        assert!(events.contains(&ModelEvent::PrChanged {
            workspace_id: "w1".into(),
            pr: Some("#7 OPEN".into()),
        }));
    }

    #[test]
    fn new_notification_emitted_once() {
        let mut prev = Snapshot::default();
        prev.notifications.insert(
            "n1".into(),
            NotifSnap {
                workspace_id: Some("w1".into()),
                title: "hi".into(),
                body: "there".into(),
            },
        );
        let mut next = prev.clone();
        next.notifications.insert(
            "n2".into(),
            NotifSnap {
                workspace_id: None,
                title: "new".into(),
                body: "body".into(),
            },
        );
        assert_eq!(
            diff(&prev, &next),
            vec![ModelEvent::NotificationAdded {
                id: "n2".into(),
                workspace_id: None,
                title: "new".into(),
                body: "body".into(),
            }]
        );
        // Removing a notification yields nothing.
        assert_eq!(diff(&next, &prev), vec![]);
    }
}
