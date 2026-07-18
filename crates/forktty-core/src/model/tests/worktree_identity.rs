//! Canonical worktree identity and repair regressions.

use super::*;

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
fn repeated_exact_worktree_identity_reuses_workspace_without_allocating_surface() {
    let worktree_dir = tempfile::tempdir().unwrap();
    let canonical_path = std::fs::canonicalize(worktree_dir.path()).unwrap();
    let mut model = WorkspaceModel::new();

    let first: crate::WorktreeWorkspaceResolution = model.find_or_create_worktree_workspace(
        "feature",
        &canonical_path,
        "feature/branch",
        "feature-wt",
        &[],
    );
    let surface_count = model.list_surfaces(None).len();
    let resolved_identities = resolved_worktree_identities(&model);
    let second = model.find_or_create_worktree_workspace(
        "feature",
        &canonical_path,
        "feature/branch",
        "feature-wt",
        &resolved_identities,
    );

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(second.workspace.id, first.workspace.id);
    assert_eq!(model.list_workspaces().len(), 1);
    assert_eq!(model.list_surfaces(None).len(), surface_count);
}

#[test]
fn worktree_resolution_uses_pre_resolved_identity_after_path_disappears() {
    let root = tempfile::tempdir().unwrap();
    let worktree_dir = root.path().join("worktree");
    std::fs::create_dir(&worktree_dir).unwrap();
    let canonical_path = std::fs::canonicalize(&worktree_dir).unwrap();
    let mut model = WorkspaceModel::new();
    let existing =
        model.create_worktree_workspace("feature", &canonical_path, "feature/old", "feature-wt");
    let resolved_identities =
        resolve_worktree_identity_snapshots(model.worktree_identity_snapshots());
    // The resolved snapshot is the transaction input. Removing the directory
    // proves model mutation does not re-enter the filesystem.
    std::fs::remove_dir(&worktree_dir).unwrap();

    let resolution = model.find_or_create_worktree_workspace(
        "replacement",
        &canonical_path,
        "feature/new",
        "feature-wt",
        &resolved_identities,
    );

    assert!(!resolution.created);
    assert_eq!(resolution.workspace.id, existing.id);
}

#[cfg(unix)]
#[test]
fn reusing_exact_worktree_identity_selects_and_refreshes_metadata_without_renaming() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let worktree_dir = root.path().join("worktree");
    std::fs::create_dir(&worktree_dir).unwrap();
    let alias_path = root.path().join("worktree-alias");
    symlink(&worktree_dir, &alias_path).unwrap();
    let canonical_path = std::fs::canonicalize(&worktree_dir).unwrap();
    let mut model = WorkspaceModel::new();
    let workspace =
        model.create_worktree_workspace("feature", &alias_path, "old-branch", "feature-wt");
    model
        .rename_workspace(WorkspaceSelector::Id(&workspace.id), "My custom workspace")
        .unwrap();
    model.workspaces.get_mut(&workspace.id).unwrap().working_dir = root.path().join("stale-path");
    let other = model.create_workspace("other", root.path());
    let surface_count = model.list_surfaces(None).len();
    let resolved_identities = resolved_worktree_identities(&model);

    let resolution = model.find_or_create_worktree_workspace(
        "replacement display name",
        &canonical_path,
        "refreshed-branch",
        "feature-wt",
        &resolved_identities,
    );

    assert!(!resolution.created);
    assert_eq!(resolution.workspace.id, workspace.id);
    assert_eq!(resolution.workspace.name, "My custom workspace");
    assert_eq!(resolution.workspace.git_branch, "refreshed-branch");
    assert_eq!(resolution.workspace.working_dir, canonical_path);
    assert_eq!(
        resolution.workspace.worktree_dir.as_deref(),
        Some(canonical_path.as_path())
    );
    assert!(resolution.workspace.active);
    assert!(!model.workspaces[&other.id].active);
    assert_eq!(model.list_surfaces(None).len(), surface_count);
}

#[test]
fn same_worktree_name_with_different_canonical_paths_stays_distinct() {
    let root = tempfile::tempdir().unwrap();
    let first_dir = root.path().join("first");
    let second_dir = root.path().join("second");
    std::fs::create_dir(&first_dir).unwrap();
    std::fs::create_dir(&second_dir).unwrap();
    let first_path = std::fs::canonicalize(first_dir).unwrap();
    let second_path = std::fs::canonicalize(second_dir).unwrap();
    let mut model = WorkspaceModel::new();

    let first = model.find_or_create_worktree_workspace(
        "feature one",
        &first_path,
        "feature/one",
        "shared-name",
        &[],
    );
    let resolved_identities = resolved_worktree_identities(&model);
    let second = model.find_or_create_worktree_workspace(
        "feature two",
        &second_path,
        "feature/two",
        "shared-name",
        &resolved_identities,
    );
    let resolved_identities = resolved_worktree_identities(&model);

    assert!(first.created);
    assert!(second.created);
    assert_ne!(first.workspace.id, second.workspace.id);
    assert_eq!(
        model
            .find_worktree_workspace("shared-name", &first_path, &resolved_identities)
            .unwrap()
            .id,
        first.workspace.id
    );
    assert_eq!(
        model
            .find_worktree_workspace("shared-name", &second_path, &resolved_identities)
            .unwrap()
            .id,
        second.workspace.id
    );
    assert_eq!(model.list_workspaces().len(), 2);
}

#[cfg(unix)]
#[test]
fn repair_session_invariants_prefers_active_duplicate_worktree_and_cleans_loser_activity() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let worktree_dir = root.path().join("worktree");
    std::fs::create_dir(&worktree_dir).unwrap();
    let alias_path = root.path().join("worktree-alias");
    symlink(&worktree_dir, &alias_path).unwrap();
    let canonical_path = std::fs::canonicalize(&worktree_dir).unwrap();
    let mut model = WorkspaceModel::new();
    let loser = model.create_worktree_workspace(
        "older duplicate",
        &alias_path,
        "stale-branch",
        "feature-wt",
    );
    let loser_split = model
        .split_surface(&loser.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    model
        .set_status_with_hook_metadata(
            &loser.id,
            "agent:claude",
            "Agent",
            "running",
            None,
            Some(StatusHookMetadata::from_order(1)),
        )
        .unwrap();
    model
        .set_progress(&loser.id, "build", "Build", 1.0, Some(10.0))
        .unwrap();
    model
        .append_log(&loser.id, LogLevel::Info, "loser log")
        .unwrap();
    model.create_notification(
        "Loser workspace",
        "workspace activity",
        NotificationKind::Info,
        Some(loser.id.clone()),
        None,
    );
    model.create_notification(
        "Loser surface",
        "surface activity",
        NotificationKind::Info,
        Some(loser.id.clone()),
        Some(loser_split.id.clone()),
    );
    let winner = model.create_worktree_workspace(
        "active duplicate",
        &canonical_path,
        "current-branch",
        "feature-wt",
    );
    let survivor_notification = model.create_notification(
        "Winner",
        "keep this activity",
        NotificationKind::Info,
        Some(winner.id.clone()),
        None,
    );

    assert!(repair_model_session_invariants(&mut model));

    assert!(!model.workspaces.contains_key(&loser.id));
    assert!(model.workspaces.contains_key(&winner.id));
    assert_eq!(model.active_workspace_id(), Some(winner.id.clone()));
    assert!(model.surface(&loser.focused_surface_id).is_none());
    assert!(model.surface(&loser_split.id).is_none());
    assert!(model.surface(&winner.focused_surface_id).is_some());
    assert!(model.list_status(&loser.id).is_empty());
    assert!(!model.status_hooks.contains_key(&loser.id));
    assert!(model.list_progress(&loser.id).is_empty());
    assert!(model.list_logs(&loser.id).is_empty());
    assert_eq!(
        model
            .list_notifications()
            .into_iter()
            .map(|notification| notification.id)
            .collect::<Vec<_>>(),
        vec![survivor_notification.id]
    );
}

#[test]
fn repair_session_invariants_prefers_earliest_duplicate_worktree_when_neither_is_active() {
    let worktree_dir = tempfile::tempdir().unwrap();
    let canonical_path = std::fs::canonicalize(worktree_dir.path()).unwrap();
    let mut model = WorkspaceModel::new();
    let first = model.create_worktree_workspace(
        "first duplicate",
        &canonical_path,
        "first-branch",
        "feature-wt",
    );
    let second = model.create_worktree_workspace(
        "second duplicate",
        &canonical_path,
        "second-branch",
        "feature-wt",
    );
    let unrelated = model.create_workspace("unrelated", worktree_dir.path());

    assert!(repair_model_session_invariants(&mut model));

    assert_eq!(
        model
            .list_workspaces()
            .into_iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>(),
        vec![first.id.clone(), unrelated.id]
    );
    assert!(!model.workspaces.contains_key(&second.id));
    assert!(model.surface(&second.focused_surface_id).is_none());
}

#[test]
fn repair_session_invariants_uses_pre_resolved_identities_after_path_disappears() {
    let root = tempfile::tempdir().unwrap();
    let worktree_dir = root.path().join("worktree");
    std::fs::create_dir(&worktree_dir).unwrap();
    let canonical_path = std::fs::canonicalize(&worktree_dir).unwrap();
    let mut model = WorkspaceModel::new();
    let first = model.create_worktree_workspace(
        "first duplicate",
        &canonical_path,
        "first-branch",
        "feature-wt",
    );
    let second = model.create_worktree_workspace(
        "second duplicate",
        &canonical_path,
        "second-branch",
        "feature-wt",
    );
    let resolved_identities =
        resolve_worktree_identity_snapshots(model.worktree_identity_snapshots());
    // Repair must consume the resolved snapshot without canonicalizing again.
    std::fs::remove_dir(&worktree_dir).unwrap();

    assert!(model.repair_session_invariants(&resolved_identities));
    assert!(!model.workspaces.contains_key(&first.id));
    assert!(model.workspaces.contains_key(&second.id));
}

#[test]
fn repair_session_invariants_keeps_duplicate_worktree_name_at_different_canonical_paths() {
    let root = tempfile::tempdir().unwrap();
    let first_dir = root.path().join("first");
    let second_dir = root.path().join("second");
    std::fs::create_dir(&first_dir).unwrap();
    std::fs::create_dir(&second_dir).unwrap();
    let first_path = std::fs::canonicalize(first_dir).unwrap();
    let second_path = std::fs::canonicalize(second_dir).unwrap();
    let mut model = WorkspaceModel::new();
    let first =
        model.create_worktree_workspace("first", &first_path, "first-branch", "shared-name");
    let second =
        model.create_worktree_workspace("second", &second_path, "second-branch", "shared-name");

    assert!(!repair_model_session_invariants(&mut model));
    assert!(model.workspaces.contains_key(&first.id));
    assert!(model.workspaces.contains_key(&second.id));
    assert_eq!(model.list_workspaces().len(), 2);
}

#[test]
fn repair_session_invariants_keeps_sanitized_duplicate_worktree_names_distinct() {
    let worktree_dir = tempfile::tempdir().unwrap();
    let canonical_path = std::fs::canonicalize(worktree_dir.path()).unwrap();
    let mut model = WorkspaceModel::new();
    let slash =
        model.create_worktree_workspace("slash", &canonical_path, "slash-branch", "topic/x");
    let dash = model.create_worktree_workspace("dash", &canonical_path, "dash-branch", "topic-x");

    assert!(!repair_model_session_invariants(&mut model));
    assert!(model.workspaces.contains_key(&slash.id));
    assert!(model.workspaces.contains_key(&dash.id));
    assert_eq!(model.list_workspaces().len(), 2);
}

#[test]
fn repair_session_invariants_keeps_unresolved_duplicate_worktree_paths_distinct() {
    let root = tempfile::tempdir().unwrap();
    let missing_path = root.path().join("missing-worktree");
    let mut model = WorkspaceModel::new();
    let first = model.create_worktree_workspace(
        "first unresolved",
        &missing_path,
        "first-branch",
        "feature-wt",
    );
    let second = model.create_worktree_workspace(
        "second unresolved",
        &missing_path,
        "second-branch",
        "feature-wt",
    );

    assert!(!repair_model_session_invariants(&mut model));
    assert!(model.workspaces.contains_key(&first.id));
    assert!(model.workspaces.contains_key(&second.id));
    assert_eq!(model.list_workspaces().len(), 2);
}
