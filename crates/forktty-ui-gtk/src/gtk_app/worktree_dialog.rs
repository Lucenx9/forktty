use super::*;

pub(super) struct WorktreeDialogChoice {
    selector: String,
    label: String,
}

pub(super) fn worktree_dialog_choices(state: &SocketAppState) -> Vec<WorktreeDialogChoice> {
    let Ok(cwd) = active_workspace_cwd_string(state) else {
        return Vec::new();
    };
    let Ok(mut worktrees) = worktree::list(&cwd) else {
        return Vec::new();
    };
    worktrees.sort_by(|left, right| {
        left.worktree_name
            .cmp(&right.worktree_name)
            .then(left.branch.cmp(&right.branch))
    });
    worktrees
        .into_iter()
        .map(|info| {
            let path = compact_path(Path::new(&info.path));
            let label = if info.branch == info.worktree_name {
                format!("{} · {path}", info.worktree_name)
            } else {
                format!("{} · {} · {path}", info.worktree_name, info.branch)
            };
            WorktreeDialogChoice {
                selector: info.worktree_name,
                label,
            }
        })
        .collect()
}

pub(super) fn show_worktree_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Worktree")
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .default_height(360)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Worktree").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Choose one worktree action, then enter the branch or worktree name.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let context_text = state
        .model
        .lock()
        .ok()
        .and_then(|model| {
            model.active_workspace().map(|workspace| {
                format!(
                    "Base: {} · {}",
                    workspace.name,
                    compact_path(&workspace.working_dir)
                )
            })
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| format!("Base: {}", compact_path(&path)))
                .unwrap_or_else(|_| "Base: current directory".to_string())
        });
    let context = gtk::Label::builder()
        .label(context_text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    context.add_css_class("worktree-context");

    let mode = Rc::new(Cell::new(WorktreeDialogMode::Create));
    let mode_selector = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mode_selector.add_css_class("worktree-mode-selector");
    mode_selector.add_css_class("linked");
    let create_mode = worktree_mode_button("Create", true);
    let attach_mode = worktree_mode_button("Attach", false);
    let merge_mode = worktree_mode_button("Merge", false);
    let remove_mode = worktree_mode_button("Remove", false);
    attach_mode.set_group(Some(&create_mode));
    merge_mode.set_group(Some(&create_mode));
    remove_mode.set_group(Some(&create_mode));
    mode_selector.append(&create_mode);
    mode_selector.append(&attach_mode);
    mode_selector.append(&merge_mode);
    mode_selector.append(&remove_mode);

    let entry = gtk::Entry::builder()
        .placeholder_text("Branch name (e.g. feature/login)")
        .hexpand(true)
        .build();
    entry.add_css_class("monospace");
    entry.update_property(&[gtk::accessible::Property::Label("Branch or worktree name")]);
    entry.set_tooltip_text(Some(
        "Branch name for Create/Attach, or existing worktree name for Remove/Merge",
    ));
    let existing_worktrees = worktree_dialog_choices(state);
    let existing = gtk::ComboBoxText::new();
    existing.add_css_class("worktree-existing");
    existing.set_tooltip_text(Some("Existing worktree to merge or remove"));
    existing.update_property(&[gtk::accessible::Property::Label("Existing worktree")]);
    for choice in &existing_worktrees {
        existing.append(Some(&choice.selector), &choice.label);
    }
    if !existing_worktrees.is_empty() {
        existing.set_active(Some(0));
    }
    existing.set_visible(false);
    let hint = gtk::Label::builder()
        .label(WorktreeDialogMode::Create.hint())
        .xalign(0.0)
        .wrap(true)
        .build();
    hint.add_css_class("ft-form-hint");

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");

    let (primary, primary_icon, primary_label) =
        labeled_icon_button_parts("forktty-add-symbolic", "Create Worktree");
    primary.add_css_class("suggested-action");
    primary.set_sensitive(false);
    let cancel = gtk::Button::with_label("Cancel");

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    body.add_css_class("ft-dialog-body");
    body.append(&context);
    body.append(&mode_selector);
    body.append(&entry);
    body.append(&existing);
    body.append(&hint);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&primary);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body);
    content.append(&footer);

    dialog.set_default_widget(Some(&primary));
    entry.set_activates_default(true);
    install_escape_close(&dialog);

    let controls = WorktreeDialogControls {
        title: title.clone(),
        subtitle: subtitle.clone(),
        entry: entry.clone(),
        existing: existing.clone(),
        has_existing_worktrees: !existing_worktrees.is_empty(),
        hint: hint.clone(),
        status: status.clone(),
        primary: primary.clone(),
        primary_icon: primary_icon.clone(),
        primary_label: primary_label.clone(),
    };
    let refresh = Rc::new({
        let mode = mode.clone();
        let controls = controls.clone();
        move |validate: bool| {
            refresh_worktree_dialog(mode.get(), &controls, validate);
        }
    });
    refresh(false);

    entry.connect_changed({
        let refresh = refresh.clone();
        move |_| refresh(true)
    });
    existing.connect_changed({
        let entry = entry.clone();
        let refresh = refresh.clone();
        move |combo| {
            if let Some(selector) = combo.active_id() {
                entry.set_text(selector.as_str());
            }
            refresh(true);
        }
    });

    for (button, next_mode) in [
        (create_mode.clone(), WorktreeDialogMode::Create),
        (attach_mode.clone(), WorktreeDialogMode::Attach),
        (merge_mode.clone(), WorktreeDialogMode::Merge),
        (remove_mode.clone(), WorktreeDialogMode::Remove),
    ] {
        let mode = mode.clone();
        let refresh = refresh.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                mode.set(next_mode);
                refresh(false);
            }
        });
    }

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());

    let state_for_action = state.clone();
    let status_for_action = status.clone();
    let entry_for_action = entry.clone();
    let dialog_for_action = dialog.clone();
    let mode_for_action = mode.clone();
    primary.connect_clicked(move |_| {
        let name = entry_for_action.text().trim().to_string();
        if let Err(err) = validate_worktree_name_for_gtk(&name) {
            set_status_message(&status_for_action, &err, StatusKind::Error);
            return;
        }

        match mode_for_action.get() {
            WorktreeDialogMode::Create => {
                match open_worktree_from_gtk(&state_for_action, &name, WorktreeAction::Create) {
                    Ok(()) => dialog_for_action.close(),
                    Err(err) => set_status_message(&status_for_action, &err, StatusKind::Error),
                }
            }
            WorktreeDialogMode::Attach => {
                match open_worktree_from_gtk(&state_for_action, &name, WorktreeAction::Attach) {
                    Ok(()) => dialog_for_action.close(),
                    Err(err) => set_status_message(&status_for_action, &err, StatusKind::Error),
                }
            }
            WorktreeDialogMode::Merge => match merge_worktree_from_gtk(&state_for_action, &name) {
                Ok(message) => set_status_message(&status_for_action, &message, StatusKind::Success),
                Err(err) => set_status_message(&status_for_action, &err, StatusKind::Error),
            },
            WorktreeDialogMode::Remove => {
                let state_confirm = state_for_action.clone();
                let status_confirm = status_for_action.clone();
                let dialog_confirm = dialog_for_action.clone();
                show_destructive_confirmation(
                    &dialog_for_action,
                    "Remove Worktree?",
                    &format!(
                        "Remove worktree '{name}' and close its ForkTTY workspace. The git branch is left intact."
                    ),
                    "Remove Worktree",
                    move || match remove_worktree_from_gtk(&state_confirm, &name) {
                        Ok(()) => dialog_confirm.close(),
                        Err(err) => set_status_message(&status_confirm, &err, StatusKind::Error),
                    },
                );
            }
        }
    });

    dialog.set_child(Some(&content));
    dialog.present();
    entry.grab_focus();
}

#[derive(Clone)]
pub(super) struct WorktreeDialogControls {
    title: gtk::Label,
    subtitle: gtk::Label,
    entry: gtk::Entry,
    existing: gtk::ComboBoxText,
    has_existing_worktrees: bool,
    hint: gtk::Label,
    status: gtk::Label,
    primary: gtk::Button,
    primary_icon: gtk::Image,
    primary_label: gtk::Label,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorktreeDialogMode {
    Create,
    Attach,
    Merge,
    Remove,
}

impl WorktreeDialogMode {
    fn dialog_title(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create Worktree",
            WorktreeDialogMode::Attach => "Attach Worktree",
            WorktreeDialogMode::Merge => "Merge Worktree",
            WorktreeDialogMode::Remove => "Remove Worktree",
        }
    }

    fn dialog_subtitle(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create a new isolated git worktree workspace.",
            WorktreeDialogMode::Attach => "Open an existing branch or linked worktree.",
            WorktreeDialogMode::Merge => {
                "Choose an existing worktree to merge into the base checkout."
            }
            WorktreeDialogMode::Remove => {
                "Choose an existing worktree to remove after dirty-state checks."
            }
        }
    }

    fn action_label(self) -> &'static str {
        // The confirm button reuses the dialog title verbatim across all modes.
        self.dialog_title()
    }

    fn icon_name(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "forktty-add-symbolic",
            WorktreeDialogMode::Attach => "forktty-folder-open-symbolic",
            WorktreeDialogMode::Merge => "forktty-merge-symbolic",
            WorktreeDialogMode::Remove => "forktty-trash-symbolic",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => {
                "Creates a new git worktree from the active workspace repository."
            }
            WorktreeDialogMode::Attach => {
                "Attaches an existing branch or worktree and opens it as a workspace."
            }
            WorktreeDialogMode::Merge => {
                "Merges the named worktree branch back into the repository checkout."
            }
            WorktreeDialogMode::Remove => {
                "Removes the named worktree and closes the matching ForkTTY workspace."
            }
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create | WorktreeDialogMode::Attach => {
                "Branch name (e.g. feature/login)"
            }
            WorktreeDialogMode::Merge | WorktreeDialogMode::Remove => {
                "Existing worktree or branch name"
            }
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create a new worktree branch",
            WorktreeDialogMode::Attach => "Attach an existing worktree branch",
            WorktreeDialogMode::Merge => "Merge the named worktree branch",
            WorktreeDialogMode::Remove => "Remove the named worktree",
        }
    }

    fn destructive(self) -> bool {
        self == WorktreeDialogMode::Remove
    }

    fn uses_existing_chooser(self) -> bool {
        matches!(self, WorktreeDialogMode::Merge | WorktreeDialogMode::Remove)
    }
}

pub(super) fn worktree_mode_button(label: &str, active: bool) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::with_label(label);
    button.add_css_class("worktree-mode-button");
    button.set_hexpand(true);
    button.set_active(active);
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

pub(super) fn refresh_worktree_dialog(
    mode: WorktreeDialogMode,
    controls: &WorktreeDialogControls,
    validate: bool,
) {
    controls.title.set_label(mode.dialog_title());
    controls.subtitle.set_label(mode.dialog_subtitle());
    controls
        .entry
        .set_placeholder_text(Some(mode.placeholder()));
    controls.entry.set_tooltip_text(Some(mode.tooltip()));
    let use_existing_chooser = mode.uses_existing_chooser() && controls.has_existing_worktrees;
    controls.entry.set_visible(!use_existing_chooser);
    controls.existing.set_visible(use_existing_chooser);
    if use_existing_chooser {
        if let Some(selector) = controls.existing.active_id() {
            if controls.entry.text().as_str() != selector.as_str() {
                controls.entry.set_text(selector.as_str());
            }
        }
    }
    controls.hint.set_label(
        if mode.uses_existing_chooser() && !controls.has_existing_worktrees {
            "No linked worktrees were found for this repository. Type a worktree or branch name manually."
        } else {
            mode.hint()
        },
    );
    controls.primary_icon.set_icon_name(Some(mode.icon_name()));
    controls.primary_label.set_text(mode.action_label());
    controls.primary.set_tooltip_text(Some(mode.tooltip()));
    set_accessible_button_text(&controls.primary, mode.action_label(), None);
    controls.primary.remove_css_class("suggested-action");
    controls.primary.remove_css_class("destructive-action");
    if mode.destructive() {
        controls.primary.add_css_class("destructive-action");
    } else {
        controls.primary.add_css_class("suggested-action");
    }

    let name = if use_existing_chooser {
        controls
            .existing
            .active_id()
            .map(|selector| selector.to_string())
            .unwrap_or_default()
    } else {
        controls.entry.text().to_string()
    };
    let trimmed = name.trim();
    let valid = if trimmed.is_empty() {
        false
    } else {
        match validate_worktree_name_for_gtk(trimmed) {
            Ok(_) => true,
            Err(err) => {
                if validate {
                    set_status_message(&controls.status, &err, StatusKind::Error);
                }
                false
            }
        }
    };
    if valid || trimmed.is_empty() || !validate {
        clear_status_message(&controls.status);
    }
    controls.primary.set_sensitive(valid);
}

#[derive(Clone, Copy)]
pub(super) enum WorktreeAction {
    Create,
    Attach,
}

pub(super) fn open_worktree_from_gtk(
    state: &SocketAppState,
    name: &str,
    action: WorktreeAction,
) -> Result<(), String> {
    let name = validate_worktree_name_for_gtk(name)?;
    let cwd = active_workspace_cwd(state).ok_or_else(no_active_workspace_message)?;
    let cwd = cwd.to_string_lossy().to_string();
    let layout = config::load_config()
        .ok()
        .map(|config| config.general.worktree_layout)
        .filter(|layout| !layout.trim().is_empty())
        .unwrap_or_else(|| "nested".to_string());
    let info = match action {
        WorktreeAction::Create => worktree::create(&cwd, name, &layout),
        WorktreeAction::Attach => worktree::attach(&cwd, name, &layout),
    }
    .map_err(|err| err.to_string())?;

    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (
            model.create_worktree_workspace(
                &info.branch,
                PathBuf::from(&info.path),
                &info.branch,
                &info.worktree_name,
            ),
            previous_active_id,
        )
    };
    if let Err(err) = state
        .terminal
        .spawn(SpawnRequest::for_workspace(
            &workspace,
            state.shell.clone(),
            state.socket_path.clone(),
        ))
        .map_err(|err| err.to_string())
    {
        let mut err = err;
        if let Err(rollback_err) =
            rollback_workspace_creation_gtk(state, &workspace.id, previous_active_id)
        {
            err = format!("{err}; workspace rollback failed: {rollback_err}");
        }
        if matches!(action, WorktreeAction::Create) {
            return Err(rollback_created_worktree_after_spawn_failure(
                &cwd, &info, err,
            ));
        }
        return Err(err);
    }
    save_session_from_state(state);
    if let Some(warning) = &info.setup_warning {
        create_global_notification(
            state,
            "Worktree Setup Hook Failed",
            warning,
            NotificationKind::Error,
        );
    }
    Ok(())
}

pub(super) fn rollback_created_worktree_after_spawn_failure(
    cwd: &str,
    info: &worktree::WorktreeInfo,
    spawn_error: String,
) -> String {
    match worktree::remove(cwd, &info.worktree_name, true) {
        Ok(()) => spawn_error,
        Err(rollback_error) => format!(
            "{spawn_error}; created worktree '{}' remains because rollback failed: {rollback_error}",
            info.worktree_name
        ),
    }
}

pub(super) fn remove_worktree_from_gtk(state: &SocketAppState, name: &str) -> Result<(), String> {
    let name = validate_worktree_name_for_gtk(name)?;
    let cwd = active_workspace_cwd_string(state)?;
    let fallback_path = worktree::repository_root(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
    let removal = worktree::prepare_remove(&cwd, name).map_err(|err| err.to_string())?;
    let workspace_worktree_name = removal.worktree_name().to_string();
    finish_prepared_worktree_removal_from_gtk(
        state,
        &workspace_worktree_name,
        fallback_path,
        removal,
    )?;
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep a workspace terminal alive: {err}");
    }
    save_session_from_state(state);
    Ok(())
}

pub(super) fn merge_worktree_from_gtk(
    state: &SocketAppState,
    name: &str,
) -> Result<String, String> {
    let name = validate_worktree_name_for_gtk(name)?;
    let cwd = active_workspace_cwd_string(state)?;
    let result = worktree::merge(&cwd, name).map_err(|err| err.to_string())?;
    Ok(if result.trim().is_empty() {
        "Merged".to_string()
    } else {
        result
    })
}

pub(super) fn validate_worktree_name_for_gtk(name: &str) -> Result<&str, String> {
    validate_worktree_name(name).map_err(|err| match err {
        WorktreeNameError::Empty => "Branch or worktree name is required".to_string(),
        WorktreeNameError::TooLong => {
            "Branch or worktree name must be 255 bytes or fewer".to_string()
        }
        WorktreeNameError::UnsupportedCharacters => {
            "Branch or worktree name contains unsupported characters".to_string()
        }
        WorktreeNameError::UnsafeSegment => {
            "Branch or worktree name contains an unsafe path segment".to_string()
        }
    })
}

pub(super) fn rollback_workspace_creation_gtk(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_workspace(WorkspaceSelector::Id(workspace_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    Ok(())
}

pub(super) fn active_workspace_cwd_string(state: &SocketAppState) -> Result<String, String> {
    active_workspace_cwd(state)
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(no_active_workspace_message)
}

pub(super) fn no_active_workspace_message() -> String {
    "No active workspace is available for worktree operations.".to_string()
}

pub(super) fn active_workspace_cwd(state: &SocketAppState) -> Option<PathBuf> {
    state.model.lock().ok().and_then(|model| {
        model
            .active_workspace()
            .map(|workspace| workspace.working_dir)
    })
}

pub(super) fn finish_prepared_worktree_removal_from_gtk(
    state: &SocketAppState,
    worktree_name: &str,
    fallback_path: PathBuf,
    removal: worktree::PreparedWorktreeRemoval,
) -> Result<(), String> {
    let (workspace, surfaces, is_last_workspace) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return Err(TerminalError::LockPoisoned.to_string()),
        };
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some(worktree_name));
        let surfaces = workspace
            .as_ref()
            .map(|workspace| model.list_surfaces(Some(&workspace.id)))
            .unwrap_or_default();
        let is_last_workspace = workspace.is_some() && model.list_workspaces().len() == 1;
        (workspace, surfaces, is_last_workspace)
    };
    let surface_ids = surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();
    if workspace.is_none() {
        removal.finish(false).map_err(|err| err.to_string())?;
        return Ok(());
    }
    if is_last_workspace {
        let workspace = workspace
            .as_ref()
            .ok_or_else(|| TerminalError::NotFound("workspace".to_string()).to_string())?;
        let (replacement, previous_active_id) = {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return Err(TerminalError::LockPoisoned.to_string()),
            };
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", fallback_path.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal_gtk(state, &replacement) {
            let mut err = err.to_string();
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
            return Err(err);
        }
        if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
            let mut err = err.to_string();
            if let Err(cleanup_err) =
                forget_terminal_surface_gtk(state, &replacement.focused_surface_id)
            {
                err = format!("{err}; replacement cleanup failed: {cleanup_err}");
            }
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
            return Err(err);
        }
        if let Err(err) = removal.finish(false) {
            let mut err = err.to_string();
            if let Err(cleanup_err) =
                forget_terminal_surface_gtk(state, &replacement.focused_surface_id)
            {
                err = format!("{err}; replacement cleanup failed: {cleanup_err}");
            }
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
            if let Err(respawn_err) = spawn_terminal_surfaces_gtk(state, &surfaces) {
                err = format!("{err}; terminal restore failed: {respawn_err}");
            }
            return Err(err);
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return Err(TerminalError::LockPoisoned.to_string()),
            };
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        return Ok(());
    }
    close_terminal_surfaces(state, &surface_ids).map_err(|err| err.to_string())?;
    if let Err(err) = removal.finish(false) {
        let mut err = err.to_string();
        if let Err(respawn_err) = spawn_terminal_surfaces_gtk(state, &surfaces) {
            err = format!("{err}; terminal restore failed: {respawn_err}");
        }
        return Err(err);
    }
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return Err(TerminalError::LockPoisoned.to_string()),
        };
        if let Some(workspace) = workspace {
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        if model.list_workspaces().is_empty() {
            model.create_workspace("main", fallback_path);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn close_workspace_by_worktree_name(
    state: &SocketAppState,
    worktree_name: &str,
    fallback_path: PathBuf,
) -> Result<(), TerminalError> {
    let (workspace, surface_ids, is_last_workspace) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return Err(TerminalError::LockPoisoned),
        };
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some(worktree_name));
        let Some(workspace) = workspace else {
            return Ok(());
        };
        let surface_ids = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        let is_last_workspace = model.list_workspaces().len() == 1;
        (workspace, surface_ids, is_last_workspace)
    };
    if is_last_workspace {
        let (replacement, previous_active_id) = {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return Err(TerminalError::LockPoisoned),
            };
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", fallback_path.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal_gtk(state, &replacement) {
            let mut err = err;
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = TerminalError::Backend(format!(
                    "{err}; workspace rollback failed: {rollback_err}"
                ));
            }
            return Err(err);
        }
        if let Err(err) = close_terminal_surfaces(state, &surface_ids) {
            let mut err = err;
            if let Err(cleanup_err) =
                forget_terminal_surface_gtk(state, &replacement.focused_surface_id)
            {
                err = TerminalError::Backend(format!(
                    "{err}; replacement cleanup failed: {cleanup_err}"
                ));
            }
            if let Err(rollback_err) =
                rollback_workspace_creation_gtk(state, &replacement.id, previous_active_id)
            {
                err = TerminalError::Backend(format!(
                    "{err}; workspace rollback failed: {rollback_err}"
                ));
            }
            return Err(err);
        }
        {
            let mut model = match state.model.lock() {
                Ok(model) => model,
                Err(_) => return Err(TerminalError::LockPoisoned),
            };
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        return Ok(());
    }
    close_terminal_surfaces(state, &surface_ids)?;
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return Err(TerminalError::LockPoisoned),
        };
        let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        if model.list_workspaces().is_empty() {
            model.create_workspace("main", fallback_path);
        }
    }
    Ok(())
}

pub(super) fn spawn_terminal_surfaces_gtk(
    state: &SocketAppState,
    surfaces: &[Surface],
) -> Result<(), TerminalError> {
    for surface in surfaces {
        let base =
            SpawnRequest::for_surface(surface, state.shell.clone(), state.socket_path.clone());
        let Some(request) = forktty_socket::spawn_request_for_surface_kind(base, &surface.kind)
        else {
            continue;
        };
        state.terminal.spawn(request)?;
    }
    Ok(())
}
