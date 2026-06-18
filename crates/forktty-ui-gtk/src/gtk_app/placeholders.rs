use super::*;

pub(super) struct SurfacePlaceholderDetails {
    icon_name: &'static str,
    title: &'static str,
    description: String,
    can_restart: bool,
}

#[cfg(not(feature = "browser"))]
pub(super) fn browser_unavailable_placeholder(surface_id: &str) -> gtk::Box {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.add_css_class("terminal-pane");
    pane.add_css_class("missing");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("terminal-pane-header");
    let title = gtk::Label::builder()
        .label("Browser unavailable")
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("terminal-pane-title");
    header.append(&title);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("terminal-placeholder");
    body.set_hexpand(true);
    body.set_vexpand(true);
    let description = format!(
        "Surface {surface_id} was restored as a browser pane, but this build was compiled without browser support."
    );
    body.append(&compact_status_page(
        "forktty-browser-symbolic",
        "Browser Feature Unavailable",
        &description,
    ));

    pane.append(&header);
    pane.append(&body);
    pane
}

pub(super) fn missing_surface_placeholder(
    surface_id: &str,
    state: Option<&SocketAppState>,
    model: Option<&Arc<Mutex<WorkspaceModel>>>,
) -> gtk::Box {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.add_css_class("terminal-pane");
    pane.add_css_class("missing");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("terminal-pane-header");
    let title = gtk::Label::builder()
        .label("Terminal unavailable")
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("terminal-pane-title");
    header.append(&title);

    let details = surface_placeholder_details(model, surface_id);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("terminal-placeholder");
    body.set_hexpand(true);
    body.set_vexpand(true);

    let status = compact_status_page(details.icon_name, details.title, &details.description);
    body.append(&status);

    if let Some(state) = state {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("terminal-placeholder-actions");
        actions.set_halign(gtk::Align::Center);
        if details.can_restart {
            let (restart, _, _) =
                labeled_icon_button_parts("forktty-refresh-symbolic", "Restart Pane");
            restart.add_css_class("suggested-action");
            let state_for_restart = state.clone();
            let surface_id_for_restart = surface_id.to_string();
            restart.connect_clicked(move |_| {
                restart_surface(&state_for_restart, &surface_id_for_restart);
            });
            actions.append(&restart);
        }
        let (copy_id, _, _) = labeled_icon_button_parts("forktty-copy-symbolic", "Copy Surface ID");
        let surface_id_for_copy = surface_id.to_string();
        copy_id.connect_clicked(move |_| copy_to_clipboard(&surface_id_for_copy));
        actions.append(&copy_id);
        body.append(&actions);
    }

    pane.append(&header);
    pane.append(&body);
    pane
}

pub(super) fn empty_terminal_stage(
    state: Option<&SocketAppState>,
    parent: Option<&adw::ApplicationWindow>,
) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 10);
    container.add_css_class("terminal-empty-stage");
    container.set_hexpand(true);
    container.set_vexpand(true);

    let status = compact_status_page(
        "forktty-terminal-symbolic",
        "No Workspace Open",
        "Create a workspace to start a terminal session.",
    );
    container.append(&status);

    if let Some(state) = state {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("terminal-placeholder-actions");
        actions.set_halign(gtk::Align::Center);
        let (create, _, _) = labeled_icon_button_parts("forktty-add-symbolic", "New Workspace");
        create.add_css_class("suggested-action");
        let state_for_create = state.clone();
        create.connect_clicked(move |_| create_plain_workspace(&state_for_create));
        actions.append(&create);
        if let Some(parent) = parent {
            let (open, _, _) =
                labeled_icon_button_parts("forktty-folder-open-symbolic", "Open Workspace");
            let state_for_open = state.clone();
            let parent_for_open = parent.clone();
            open.connect_clicked(move |_| open_workspace_dialog(&parent_for_open, &state_for_open));
            actions.append(&open);
        }
        container.append(&actions);

        let hints = gtk::Box::new(gtk::Orientation::Vertical, 6);
        hints.add_css_class("terminal-empty-shortcuts");
        hints.set_halign(gtk::Align::Center);
        hints.append(&shortcut_hint("Ctrl+Shift+N", "New Workspace"));
        hints.append(&shortcut_hint("Ctrl+Shift+O", "Open Workspace"));
        hints.append(&shortcut_hint("Ctrl+Shift+P", "Command Palette"));
        container.append(&hints);
    }

    container
}

pub(super) fn shortcut_hint(shortcut: &str, label: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("shortcut-hint");
    let key = gtk::Label::new(Some(shortcut));
    key.add_css_class("keycap");
    key.add_css_class("monospace");
    let text = gtk::Label::builder().label(label).xalign(0.0).build();
    text.add_css_class("ft-form-hint");
    row.append(&key);
    row.append(&text);
    row
}

pub(super) fn surface_placeholder_details(
    model: Option<&Arc<Mutex<WorkspaceModel>>>,
    surface_id: &str,
) -> SurfacePlaceholderDetails {
    let status = model.and_then(|model| model.lock().ok()).and_then(|model| {
        let surface = model.surface(surface_id)?;
        model
            .list_status(&surface.workspace_id)
            .into_iter()
            .find(|entry| entry.key == surface_status_key(surface_id))
    });

    if let Some(status) = status {
        if status_entry_suggests_error(&status) {
            return SurfacePlaceholderDetails {
                icon_name: "forktty-error-symbolic",
                title: "Terminal Failed to Start",
                description: terminal_failure_guidance(&status.value),
                can_restart: true,
            };
        }
        if status_entry_suggests_exited(&status) {
            return SurfacePlaceholderDetails {
                icon_name: "forktty-warning-symbolic",
                title: "Terminal Exited",
                description: format!(
                    "{}. Restart this pane to open a new shell in the same directory.",
                    truncate_single_line(&status.value, 180)
                ),
                can_restart: true,
            };
        }
        if status.value.to_ascii_lowercase().contains("restarting") {
            return SurfacePlaceholderDetails {
                icon_name: "forktty-refresh-symbolic",
                title: "Starting Terminal",
                description: "ForkTTY is starting this pane.".to_string(),
                can_restart: false,
            };
        }
    }

    SurfacePlaceholderDetails {
        icon_name: "forktty-terminal-symbolic",
        title: "Terminal Waiting to Start",
        description: format!(
            "This pane is waiting for its terminal process. Restart the pane if it stays here. Diagnostic ID: {surface_id}."
        ),
        can_restart: true,
    }
}

pub(super) fn terminal_failure_guidance(message: &str) -> String {
    let summary = truncate_single_line(message, 180);
    let lower = message.to_ascii_lowercase();
    let hint = if lower.contains("no such file or directory")
        || lower.contains("not found")
        || lower.contains("permission denied")
    {
        "Check the configured shell path and permissions, then restart this pane."
    } else if lower.contains("pty") {
        "The Ghostty/PTY backend failed. Restart this pane after checking the terminal backend."
    } else {
        "Check the terminal configuration, then restart this pane."
    };
    format!("{summary}. {hint}")
}

pub(super) fn compact_status_page(
    icon_name: &str,
    title: &str,
    description: &str,
) -> adw::StatusPage {
    let page = adw::StatusPage::builder()
        .icon_name(icon_name)
        .title(title)
        .description(description)
        .build();
    page.add_css_class("compact");
    page
}

pub(super) fn surface_title(surface: &Surface) -> String {
    let title = surface.title.trim();
    if !title.is_empty() && title != "shell" {
        return title.to_string();
    }
    if let Some(name) = surface
        .cwd
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
    {
        return name.to_string();
    }
    "Terminal".to_string()
}

pub(super) fn compact_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim_end_matches('/');
        if !home.is_empty() && raw == home {
            return "~".to_string();
        }
        if let Some(rest) = raw.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    raw.to_string()
}
