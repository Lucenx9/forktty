//! Browser and SSH surface-kind model regression tests.

use super::*;

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
fn set_surface_url_rejects_overlong_browser_urls() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let browser = model
        .open_browser(
            &ws.id,
            "https://a.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap();
    let overlong = format!("https://{}", "a".repeat(MAX_BROWSER_URL_BYTES));

    assert!(!model.set_surface_url(&browser.id, &overlong));
    assert_eq!(
        model.surface(&browser.id).unwrap().kind,
        SurfaceKind::Browser {
            url: "https://a.com".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
}

#[test]
fn set_surface_url_preserves_committed_non_hierarchical_urls() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let browser = model
        .open_browser(
            &ws.id,
            "https://a.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap();

    assert!(model.set_surface_url(&browser.id, "about:blank"));
    assert_eq!(
        model.surface(&browser.id).unwrap().kind,
        SurfaceKind::Browser {
            url: "about:blank".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
    assert_eq!(model.surface(&browser.id).unwrap().title, "browser");
}

#[test]
fn browser_url_validation_applies_default_scheme_before_limit() {
    let fits = "a".repeat(MAX_BROWSER_URL_BYTES - "https://".len());
    assert_eq!(
        validated_browser_url(&fits),
        Some(format!("https://{fits}"))
    );

    let overlong = "a".repeat(MAX_BROWSER_URL_BYTES - "https://".len() + 1);
    assert_eq!(validated_browser_url(&overlong), None);
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
    let workspace = model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());

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
    assert_eq!(model.surface_count(Some(&workspace.id)), 2);
}

#[test]
fn ssh_workspace_survives_session_round_trip() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());
    let ssh_id = workspace.focused_surface_id.clone();

    let data = model.to_session_data();
    let mut restored = WorkspaceModel::new();
    restore_model_session(&mut restored, data);

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
    restore_model_session(&mut restored, data);

    let surface = restored.surface(&browser_id).expect("surface restored");
    assert_eq!(
        surface.kind,
        SurfaceKind::Browser {
            url: "https://example.com".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
}
