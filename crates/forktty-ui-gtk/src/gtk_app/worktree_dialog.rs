//! GTK worktree dialogs and background git task plumbing.

use super::*;

pub(super) struct WorktreeDialogChoice {
    selector: String,
    label: String,
}

pub(super) fn worktree_dialog_choices_from_list(
    mut worktrees: Vec<worktree::WorktreeInfo>,
) -> Vec<WorktreeDialogChoice> {
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

/// Repo discovery base for Create/Attach: prefer the focused surface's live
/// shell CWD so a pane that has `cd`'d into another checkout can spawn a
/// worktree there. Destructive Remove/Merge must not use this — see
/// [`active_workspace_repo_cwd`].
#[cfg(test)]
fn active_worktree_discovery_base(state: &SocketAppState) -> Option<(String, PathBuf)> {
    let _ = forktty_socket::sync_live_surface_cwds(state);
    let snapshot = active_worktree_snapshot(state)?;
    let cwd = snapshot
        .surface_cwd
        .filter(|cwd| cwd.is_dir())
        .unwrap_or(snapshot.workspace_cwd);
    Some((snapshot.name, cwd))
}

struct ActiveWorktreeSnapshot {
    name: String,
    surface_cwd: Option<PathBuf>,
    workspace_cwd: PathBuf,
}

fn active_worktree_snapshot(state: &SocketAppState) -> Option<ActiveWorktreeSnapshot> {
    let model = state.model.lock().ok()?;
    let workspace = model.active_workspace()?;
    let surface_cwd = model
        .surface(&workspace.focused_surface_id)
        .map(|surface| surface.cwd.clone());
    Some(ActiveWorktreeSnapshot {
        name: workspace.name,
        surface_cwd,
        workspace_cwd: workspace.working_dir,
    })
}

/// Stable modeled identity for destructive-operation context.
///
/// This intentionally does not inspect the focused surface CWD: a pane can
/// leave its workspace checkout without changing which checkout initiated a
/// Merge or Remove operation.
fn active_modeled_workspace(state: &SocketAppState) -> Option<(String, PathBuf)> {
    let snapshot = active_worktree_snapshot(state)?;
    Some((snapshot.name, snapshot.workspace_cwd))
}

/// Stable base-checkout path for Remove/Merge and their chooser.
///
/// Starts from the modeled `working_dir`, never the focused surface's ephemeral
/// shell CWD, then resolves linked worktrees to the repository's common base
/// checkout. Shell CWD can leave the repo entirely (`cd /tmp`), and routing a
/// destructive worktree op through that path hits the wrong repository.
fn active_workspace_repo_cwd(state: &SocketAppState) -> Option<PathBuf> {
    let (_, workspace_cwd) = active_modeled_workspace(state)?;
    workspace_cwd
        .to_str()
        .and_then(|cwd| worktree::repository_root(cwd).ok())
        .or(Some(workspace_cwd))
}

struct WorktreeDialogContext {
    discovery_cwd: Option<PathBuf>,
    repo_cwd: Option<PathBuf>,
    discovery_text: String,
    repo_text: String,
}

fn worktree_dialog_context(state: &SocketAppState) -> WorktreeDialogContext {
    let _ = forktty_socket::sync_live_surface_cwds(state);
    worktree_dialog_context_from_snapshot(active_worktree_snapshot(state))
}

fn worktree_dialog_context_from_snapshot(
    snapshot: Option<ActiveWorktreeSnapshot>,
) -> WorktreeDialogContext {
    let discovery_workspace = snapshot.as_ref().map(|snapshot| {
        let cwd = snapshot
            .surface_cwd
            .as_ref()
            .filter(|cwd| cwd.is_dir())
            .cloned()
            .unwrap_or_else(|| snapshot.workspace_cwd.clone());
        (snapshot.name.clone(), cwd)
    });
    let source_workspace = snapshot
        .as_ref()
        .map(|snapshot| (snapshot.name.clone(), snapshot.workspace_cwd.clone()));
    let discovery_cwd = discovery_workspace.as_ref().map(|(_, cwd)| cwd.clone());
    let repo_cwd = snapshot.as_ref().map(|snapshot| {
        snapshot
            .workspace_cwd
            .to_str()
            .and_then(|cwd| worktree::repository_root(cwd).ok())
            .unwrap_or_else(|| snapshot.workspace_cwd.clone())
    });
    let discovery_text = discovery_workspace
        .as_ref()
        .map(|(name, cwd)| format!("{name}\n{}", compact_path(cwd)))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| compact_path(&path))
                .unwrap_or_else(|_| "Current directory".to_string())
        });
    let repo_text = repository_context_text(
        source_workspace.as_ref(),
        repo_cwd.as_deref(),
        &discovery_text,
    );
    WorktreeDialogContext {
        discovery_cwd,
        repo_cwd,
        discovery_text,
        repo_text,
    }
}

fn repository_context_text(
    source_workspace: Option<&(String, PathBuf)>,
    base_checkout: Option<&Path>,
    fallback: &str,
) -> String {
    match (source_workspace, base_checkout) {
        (Some((name, source_path)), Some(path)) => {
            let base_path = compact_path(path);
            let source_label = compact_path(source_path);
            if source_label == base_path {
                format!("{base_path}\nFrom {name}")
            } else {
                let source_label = source_path
                    .strip_prefix(path)
                    .ok()
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .map(compact_path)
                    .unwrap_or(source_label);
                format!("{base_path}\nFrom {name} · {source_label}")
            }
        }
        (None, Some(path)) => compact_path(path),
        _ => fallback.to_string(),
    }
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
    dialog.add_css_class("worktree-dialog");
    apply_dialog_chrome(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Worktree").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Create, attach, merge, or remove a git worktree.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    // Create/Attach follow the live shell CWD; Remove/Merge resolve the modeled
    // workspace to its common base checkout so a shell that left the repo
    // cannot point destructive git ops at an unrelated tree.
    let WorktreeDialogContext {
        discovery_cwd,
        repo_cwd,
        discovery_text: discovery_context_text,
        repo_text: repo_context_text,
    } = worktree_dialog_context(state);
    let context = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    context.add_css_class("worktree-context");
    context.update_property(&[gtk::accessible::Property::Label(&format!(
        "Starting from {discovery_context_text}"
    ))]);
    let context_icon = gtk::Image::from_icon_name("forktty-folder-symbolic");
    context_icon.add_css_class("worktree-context-icon");
    let context_copy = gtk::Box::new(gtk::Orientation::Vertical, 0);
    context_copy.set_hexpand(true);
    let context_title = gtk::Label::builder()
        .label("Starting from")
        .xalign(0.0)
        .build();
    context_title.add_css_class("worktree-context-title");
    let context_value = gtk::Label::builder()
        .label(&discovery_context_text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    context_value.add_css_class("worktree-context-value");
    context_copy.append(&context_title);
    context_copy.append(&context_value);
    context.append(&context_icon);
    context.append(&context_copy);

    let dialog_state = Rc::new(RefCell::new(WorktreeDialogState::new(
        WorktreeDialogMode::Create,
    )));
    let updating_target = Rc::new(Cell::new(false));
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
        .placeholder_text("feature/login")
        .hexpand(true)
        .build();
    entry.add_css_class("monospace");
    entry.update_property(&[gtk::accessible::Property::Label("Branch or worktree name")]);
    entry.set_tooltip_text(Some(
        "Branch name for Create/Attach, or existing worktree name for Remove/Merge",
    ));
    // Populated asynchronously below: `git worktree list` can be slow on
    // large repositories and must not block the dialog from opening.
    let existing = gtk::ComboBoxText::new();
    existing.add_css_class("worktree-existing");
    existing.set_tooltip_text(Some("Existing worktree to merge or remove"));
    existing.update_property(&[gtk::accessible::Property::Label("Existing worktree")]);
    let hint = gtk::Label::builder()
        .label(WorktreeDialogMode::Create.hint())
        .xalign(0.0)
        .wrap(true)
        .build();
    hint.add_css_class("ft-form-hint");
    let target_label = gtk::Label::builder()
        .label(WorktreeDialogMode::Create.target_label())
        .xalign(0.0)
        .build();
    target_label.add_css_class("worktree-target-label");
    let target = gtk::Box::new(gtk::Orientation::Vertical, 6);
    target.add_css_class("worktree-target");
    target.append(&target_label);
    target.append(&entry);
    target.append(&existing);
    target.append(&hint);

    let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
    status.add_css_class("ft-inline-status");

    let (primary, primary_icon, primary_label) =
        labeled_icon_button_parts("forktty-add-symbolic", "Create");
    primary.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    body.add_css_class("ft-dialog-body");
    body.append(&context);
    body.append(&mode_selector);
    body.append(&target);
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
    install_worktree_dialog_dismissal_shortcuts(&dialog, dialog_state.clone());
    dialog.connect_close_request({
        let dialog_state = dialog_state.clone();
        move |_| {
            if dialog_state
                .borrow()
                .dismissal_allowed(WorktreeDialogDismissal::WindowClose)
            {
                glib::Propagation::Proceed
            } else {
                glib::Propagation::Stop
            }
        }
    });

    let controls = WorktreeDialogControls {
        dialog: dialog.clone(),
        mode_selector: mode_selector.clone(),
        title: title.clone(),
        subtitle: subtitle.clone(),
        target_label: target_label.clone(),
        entry: entry.clone(),
        existing: existing.clone(),
        hint: hint.clone(),
        status: status.clone(),
        primary: primary.clone(),
        primary_icon: primary_icon.clone(),
        primary_label: primary_label.clone(),
        cancel: cancel.clone(),
    };
    let refresh = Rc::new({
        let dialog_state = dialog_state.clone();
        let controls = controls.clone();
        move |status_refresh: WorktreeDialogStatusRefresh| {
            refresh_worktree_dialog(*dialog_state.borrow(), &controls, status_refresh);
        }
    });
    refresh(WorktreeDialogStatusRefresh::Clear);

    {
        let socket_state = state.clone();
        // The chooser is only used by Remove/Merge, so list from the same
        // modeled repository those destructive operations target.
        let base_cwd = repo_cwd.clone();
        let dialog_state = dialog_state.clone();
        let entry = entry.clone();
        let existing = existing.clone();
        let updating_target = updating_target.clone();
        let refresh = refresh.clone();
        glib::spawn_future_local(async move {
            let Some(cwd) = base_cwd else {
                let status_refresh = dialog_state.borrow_mut().discovery_failed();
                refresh(status_refresh);
                return;
            };
            let cwd = cwd.to_string_lossy().to_string();
            let worktrees =
                match forktty_socket::run_guarded_worktree_read(&socket_state, move || {
                    worktree::list(&cwd)
                })
                .await
                {
                    Ok(worktrees) => worktrees,
                    _ => {
                        let status_refresh = dialog_state.borrow_mut().discovery_failed();
                        refresh(status_refresh);
                        return;
                    }
                };
            let choices = worktree_dialog_choices_from_list(worktrees);
            for choice in &choices {
                existing.append(Some(&choice.selector), &choice.label);
            }
            let first_selector = choices.first().map(|choice| choice.selector.as_str());
            let current_target = entry.text();
            let discovery = dialog_state
                .borrow_mut()
                .discovery_ready(current_target.as_str(), first_selector);
            if first_selector.is_some()
                && dialog_state.borrow().target_source != WorktreeTargetSource::UserTyped
            {
                updating_target.set(true);
                existing.set_active(Some(0));
                entry.set_text(&discovery.target);
                updating_target.set(false);
            }
            refresh(discovery.status_refresh);
        });
    }

    entry.connect_changed({
        let dialog_state = dialog_state.clone();
        let updating_target = updating_target.clone();
        let refresh = refresh.clone();
        move |_| {
            if updating_target.get() {
                return;
            }
            dialog_state.borrow_mut().user_typed_target();
            refresh(WorktreeDialogStatusRefresh::Validate);
        }
    });
    existing.connect_changed({
        let entry = entry.clone();
        let dialog_state = dialog_state.clone();
        let updating_target = updating_target.clone();
        let refresh = refresh.clone();
        move |combo| {
            if updating_target.get() {
                return;
            }
            if let Some(selector) = combo.active_id() {
                dialog_state.borrow_mut().select_existing_target();
                updating_target.set(true);
                entry.set_text(selector.as_str());
                updating_target.set(false);
            }
            refresh(WorktreeDialogStatusRefresh::Validate);
        }
    });

    for (button, next_mode) in [
        (create_mode, WorktreeDialogMode::Create),
        (attach_mode, WorktreeDialogMode::Attach),
        (merge_mode, WorktreeDialogMode::Merge),
        (remove_mode, WorktreeDialogMode::Remove),
    ] {
        let dialog_state = dialog_state.clone();
        let entry = entry.clone();
        let existing = existing.clone();
        let context = context.clone();
        let context_title = context_title.clone();
        let context_value = context_value.clone();
        let discovery_context_text = discovery_context_text.clone();
        let repo_context_text = repo_context_text.clone();
        let updating_target = updating_target.clone();
        let refresh = refresh.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                let current_target = entry.text();
                let active_selector = existing.active_id();
                let target = dialog_state.borrow_mut().change_mode(
                    next_mode,
                    current_target.as_str(),
                    active_selector.as_deref(),
                );
                if target.as_str() != current_target.as_str() {
                    updating_target.set(true);
                    entry.set_text(&target);
                    updating_target.set(false);
                }
                let context_text = if next_mode.uses_workspace_repo_base() {
                    &repo_context_text
                } else {
                    &discovery_context_text
                };
                let context_label = next_mode.context_label();
                context_title.set_label(context_label);
                context_value.set_label(context_text);
                context.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{context_label} {context_text}"
                ))]);
                refresh(WorktreeDialogStatusRefresh::Clear);
            }
        });
    }

    let dialog_for_cancel = dialog.clone();
    let dialog_state_for_cancel = dialog_state.clone();
    cancel.connect_clicked(move |_| {
        let allowed = dialog_state_for_cancel
            .borrow()
            .dismissal_allowed(WorktreeDialogDismissal::Cancel);
        if allowed {
            dialog_for_cancel.close();
        }
    });

    let state_for_action = state.clone();
    let status_for_action = status.clone();
    let entry_for_action = entry.clone();
    let existing_for_action = existing.clone();
    let dialog_for_action = dialog.clone();
    let dialog_state_for_action = dialog_state.clone();
    let refresh_for_action = refresh.clone();
    let discovery_cwd_for_action = discovery_cwd.clone();
    let repo_cwd_for_action = repo_cwd.clone();
    primary.connect_clicked(move |_| {
        let state_snapshot = *dialog_state_for_action.borrow();
        if state_snapshot.running.is_some() {
            return;
        }
        let mode = state_snapshot.mode;
        let name = if state_snapshot.uses_existing_chooser() {
            existing_for_action
                .active_id()
                .map(|selector| selector.to_string())
                .unwrap_or_default()
        } else {
            entry_for_action.text().to_string()
        }
        .trim()
        .to_string();
        if let Err(err) = validate_worktree_name_for_gtk(&name) {
            set_status_message(&status_for_action, &err, StatusKind::Error);
            return;
        }
        // Create/Attach need a discovery path; Remove/Merge need the resolved
        // common base checkout. Missing either is "no active workspace".
        let op_cwd = if mode.uses_workspace_repo_base() {
            repo_cwd_for_action.clone()
        } else {
            discovery_cwd_for_action.clone()
        };
        let Some(op_cwd) = op_cwd else {
            set_status_message(
                &status_for_action,
                &no_active_workspace_message(),
                StatusKind::Error,
            );
            return;
        };

        let action = match mode {
            WorktreeDialogMode::Create => Some(WorktreeAction::Create),
            WorktreeDialogMode::Attach => Some(WorktreeAction::Attach),
            WorktreeDialogMode::Merge | WorktreeDialogMode::Remove => None,
        };
        match mode {
            WorktreeDialogMode::Create | WorktreeDialogMode::Attach => {
                if dialog_state_for_action.borrow_mut().begin_operation().is_none() {
                    return;
                }
                refresh_for_action(WorktreeDialogStatusRefresh::Clear);
                let state = state_for_action.clone();
                let status = status_for_action.clone();
                let dialog = dialog_for_action.clone();
                let dialog_state = dialog_state_for_action.clone();
                let refresh = refresh_for_action.clone();
                let base_cwd = op_cwd.clone();
                glib::spawn_future_local(async move {
                    let result = open_worktree_from_gtk_async_at_cwd(
                        &state,
                        &base_cwd,
                        &name,
                        action.expect("set above"),
                    )
                    .await;
                    dialog_state.borrow_mut().finish_operation();
                    refresh(WorktreeDialogStatusRefresh::Preserve);
                    match result {
                        Ok(()) => dialog.close(),
                        Err(err) => set_status_message(&status, &err, StatusKind::Error),
                    }
                });
            }
            WorktreeDialogMode::Merge => {
                if dialog_state_for_action.borrow_mut().begin_operation().is_none() {
                    return;
                }
                refresh_for_action(WorktreeDialogStatusRefresh::Clear);
                let state = state_for_action.clone();
                let status = status_for_action.clone();
                let dialog_state = dialog_state_for_action.clone();
                let refresh = refresh_for_action.clone();
                let base_cwd = op_cwd.clone();
                glib::spawn_future_local(async move {
                    let result = merge_worktree_from_gtk_async_at_cwd(&state, &base_cwd, &name).await;
                    dialog_state.borrow_mut().finish_operation();
                    refresh(WorktreeDialogStatusRefresh::Preserve);
                    match result {
                        Ok(message) => set_status_message(&status, &message, StatusKind::Success),
                        Err(err) => set_status_message(&status, &err, StatusKind::Error),
                    }
                });
            }
            WorktreeDialogMode::Remove => {
                let state_confirm = state_for_action.clone();
                let status_confirm = status_for_action.clone();
                let dialog_confirm = dialog_for_action.clone();
                let dialog_state_confirm = dialog_state_for_action.clone();
                let refresh_confirm = refresh_for_action.clone();
                let base_cwd_confirm = op_cwd.clone();
                show_destructive_confirmation(
                    &dialog_for_action,
                    "Remove worktree?",
                    &format!(
                        "Remove worktree '{name}' and close its ForkTTY workspace. The git branch is left intact."
                    ),
                    "Remove worktree",
                    move || {
                        let state = state_confirm.clone();
                        let status = status_confirm.clone();
                        let dialog = dialog_confirm.clone();
                        let dialog_state = dialog_state_confirm.clone();
                        let refresh = refresh_confirm.clone();
                        let name = name.clone();
                        let base_cwd = base_cwd_confirm.clone();
                        if dialog_state.borrow_mut().begin_operation()
                            != Some(WorktreeDialogMode::Remove)
                        {
                            return;
                        }
                        refresh(WorktreeDialogStatusRefresh::Clear);
                        glib::spawn_future_local(async move {
                            let result =
                                remove_worktree_from_gtk_async_at_cwd(&state, &base_cwd, &name)
                                    .await;
                            dialog_state.borrow_mut().finish_operation();
                            refresh(WorktreeDialogStatusRefresh::Preserve);
                            match result {
                                Ok(()) => dialog.close(),
                                Err(err) => set_status_message(&status, &err, StatusKind::Error),
                            }
                        });
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
    dialog: gtk::Window,
    mode_selector: gtk::Box,
    title: gtk::Label,
    subtitle: gtk::Label,
    target_label: gtk::Label,
    entry: gtk::Entry,
    existing: gtk::ComboBoxText,
    hint: gtk::Label,
    status: gtk::Label,
    primary: gtk::Button,
    primary_icon: gtk::Image,
    primary_label: gtk::Label,
    cancel: gtk::Button,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorktreeDialogMode {
    Create,
    Attach,
    Merge,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorktreeDiscoveryState {
    Loading,
    Ready { has_choices: bool },
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorktreeTargetSource {
    Untouched,
    UserTyped,
    ExistingSelection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WorktreeDialogState {
    mode: WorktreeDialogMode,
    discovery: WorktreeDiscoveryState,
    target_source: WorktreeTargetSource,
    running: Option<WorktreeDialogMode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorktreeDialogSensitivity {
    mode_controls: bool,
    target_controls: bool,
    primary: bool,
    cancel: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorktreeDialogStatusRefresh {
    Clear,
    Validate,
    Preserve,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorktreeDialogStatusUpdate {
    Preserve,
    Clear,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorktreeDiscoveryUpdate {
    target: String,
    status_refresh: WorktreeDialogStatusRefresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorktreeDialogDismissal {
    WindowClose,
    Escape,
    Cancel,
}

impl WorktreeDialogState {
    fn new(mode: WorktreeDialogMode) -> Self {
        Self {
            mode,
            discovery: WorktreeDiscoveryState::Loading,
            target_source: WorktreeTargetSource::Untouched,
            running: None,
        }
    }

    fn user_typed_target(&mut self) {
        if self.running.is_some() {
            return;
        }
        self.target_source = WorktreeTargetSource::UserTyped;
    }

    fn select_existing_target(&mut self) {
        if self.running.is_none() {
            self.target_source = WorktreeTargetSource::ExistingSelection;
        }
    }

    fn change_mode(
        &mut self,
        mode: WorktreeDialogMode,
        current_target: &str,
        first_selector: Option<&str>,
    ) -> String {
        if self.running.is_some() {
            return current_target.to_string();
        }
        self.mode = mode;
        self.adopt_existing_target_if_untouched(current_target, first_selector)
    }

    fn begin_operation(&mut self) -> Option<WorktreeDialogMode> {
        if self.running.is_some() {
            return None;
        }
        self.running = Some(self.mode);
        self.running
    }

    fn finish_operation(&mut self) {
        self.running = None;
    }

    fn sensitivity(self, target_valid: bool) -> WorktreeDialogSensitivity {
        let enabled = self.running.is_none();
        WorktreeDialogSensitivity {
            mode_controls: enabled,
            target_controls: enabled,
            primary: enabled && target_valid,
            cancel: enabled,
        }
    }

    fn dismissal_allowed(self, _route: WorktreeDialogDismissal) -> bool {
        self.running.is_none()
    }

    fn discovery_ready(
        &mut self,
        current_target: &str,
        first_selector: Option<&str>,
    ) -> WorktreeDiscoveryUpdate {
        self.discovery = WorktreeDiscoveryState::Ready {
            has_choices: first_selector.is_some(),
        };
        WorktreeDiscoveryUpdate {
            target: self.adopt_existing_target_if_untouched(current_target, first_selector),
            status_refresh: WorktreeDialogStatusRefresh::Preserve,
        }
    }

    fn discovery_failed(&mut self) -> WorktreeDialogStatusRefresh {
        self.discovery = WorktreeDiscoveryState::Failed;
        WorktreeDialogStatusRefresh::Preserve
    }

    fn uses_existing_chooser(self) -> bool {
        self.mode.uses_existing_chooser()
            && matches!(
                self.discovery,
                WorktreeDiscoveryState::Ready { has_choices: true }
            )
            && self.target_source == WorktreeTargetSource::ExistingSelection
    }

    fn adopt_existing_target_if_untouched(
        &mut self,
        current_target: &str,
        first_selector: Option<&str>,
    ) -> String {
        if self.running.is_none()
            && self.target_source == WorktreeTargetSource::Untouched
            && self.mode.uses_existing_chooser()
        {
            if let Some(first_selector) = first_selector {
                self.target_source = WorktreeTargetSource::ExistingSelection;
                return first_selector.to_string();
            }
        }
        current_target.to_string()
    }
}

impl WorktreeDialogMode {
    fn context_label(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create | WorktreeDialogMode::Attach => "Starting from",
            WorktreeDialogMode::Merge => "Merging into",
            WorktreeDialogMode::Remove => "Repository",
        }
    }

    fn dialog_title(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create worktree",
            WorktreeDialogMode::Attach => "Attach worktree",
            WorktreeDialogMode::Merge => "Merge worktree",
            WorktreeDialogMode::Remove => "Remove worktree",
        }
    }

    fn dialog_subtitle(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create a workspace from a new branch.",
            WorktreeDialogMode::Attach => "Open an existing branch or linked worktree.",
            WorktreeDialogMode::Merge => "Merge a linked worktree back into the base checkout.",
            WorktreeDialogMode::Remove => "Remove a linked worktree after dirty-state checks.",
        }
    }

    fn action_label(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "Create",
            WorktreeDialogMode::Attach => "Attach",
            WorktreeDialogMode::Merge => "Merge",
            WorktreeDialogMode::Remove => "Remove",
        }
    }

    fn target_label(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "New branch",
            WorktreeDialogMode::Attach => "Existing branch or worktree",
            WorktreeDialogMode::Merge => "Worktree to merge",
            WorktreeDialogMode::Remove => "Worktree to remove",
        }
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
            WorktreeDialogMode::Create => "Creates a linked checkout and opens it as a workspace.",
            WorktreeDialogMode::Attach => "Reuses an existing branch without creating a new one.",
            WorktreeDialogMode::Merge => "The base checkout shown above receives the merge.",
            WorktreeDialogMode::Remove => "The git branch remains intact.",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            WorktreeDialogMode::Create => "feature/login",
            WorktreeDialogMode::Attach => "branch or worktree name",
            WorktreeDialogMode::Merge | WorktreeDialogMode::Remove => "worktree or branch name",
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
        self.uses_workspace_repo_base()
    }

    fn uses_workspace_repo_base(self) -> bool {
        matches!(self, WorktreeDialogMode::Merge | WorktreeDialogMode::Remove)
    }
}

fn install_worktree_dialog_dismissal_shortcuts(
    window: &gtk::Window,
    dialog_state: Rc<RefCell<WorktreeDialogState>>,
) {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let window_for_close = window.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let is_close_shortcut =
            key == gtk::gdk::Key::w && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if key == gtk::gdk::Key::Escape || is_close_shortcut {
            let allowed = dialog_state
                .borrow()
                .dismissal_allowed(WorktreeDialogDismissal::Escape);
            if allowed {
                window_for_close.close();
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

pub(super) fn worktree_mode_button(label: &str, active: bool) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::with_label(label);
    button.add_css_class("worktree-mode-button");
    button.set_hexpand(true);
    button.set_active(active);
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

fn worktree_dialog_target_status(
    name: &str,
    status_refresh: WorktreeDialogStatusRefresh,
) -> (bool, WorktreeDialogStatusUpdate) {
    let trimmed = name.trim();
    let validation_error = if trimmed.is_empty() {
        None
    } else {
        validate_worktree_name_for_gtk(trimmed).err()
    };
    let valid = !trimmed.is_empty() && validation_error.is_none();
    let status_update = match status_refresh {
        WorktreeDialogStatusRefresh::Preserve => WorktreeDialogStatusUpdate::Preserve,
        WorktreeDialogStatusRefresh::Clear => WorktreeDialogStatusUpdate::Clear,
        WorktreeDialogStatusRefresh::Validate => validation_error
            .map(WorktreeDialogStatusUpdate::Error)
            .unwrap_or(WorktreeDialogStatusUpdate::Clear),
    };
    (valid, status_update)
}

fn worktree_dialog_hint(mode: WorktreeDialogMode, discovery: WorktreeDiscoveryState) -> String {
    let discovery_message = if mode.uses_existing_chooser() {
        match discovery {
            WorktreeDiscoveryState::Failed => {
                Some("Could not load linked worktrees. Type a worktree or branch name.")
            }
            WorktreeDiscoveryState::Loading => Some("Loading linked worktrees…"),
            WorktreeDiscoveryState::Ready { has_choices: false } => {
                Some("No linked worktrees found. Type a worktree or branch name.")
            }
            WorktreeDiscoveryState::Ready { has_choices: true } => None,
        }
    } else {
        None
    };

    discovery_message
        .map(|message| format!("{message} {}", mode.hint()))
        .unwrap_or_else(|| mode.hint().to_string())
}

fn refresh_worktree_dialog(
    state: WorktreeDialogState,
    controls: &WorktreeDialogControls,
    status_refresh: WorktreeDialogStatusRefresh,
) {
    let mode = state.mode;
    controls.dialog.set_title(Some(mode.dialog_title()));
    controls.title.set_label(mode.dialog_title());
    controls.subtitle.set_label(mode.dialog_subtitle());
    controls.target_label.set_label(mode.target_label());
    controls
        .entry
        .set_placeholder_text(Some(mode.placeholder()));
    controls.entry.set_tooltip_text(Some(mode.tooltip()));
    let use_existing_chooser = state.uses_existing_chooser();
    controls.entry.set_visible(!use_existing_chooser);
    controls.existing.set_visible(use_existing_chooser);
    controls
        .hint
        .set_label(&worktree_dialog_hint(mode, state.discovery));
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
    let (valid, status_update) = worktree_dialog_target_status(&name, status_refresh);
    match status_update {
        WorktreeDialogStatusUpdate::Preserve => {}
        WorktreeDialogStatusUpdate::Clear => clear_status_message(&controls.status),
        WorktreeDialogStatusUpdate::Error(err) => {
            set_status_message(&controls.status, &err, StatusKind::Error);
        }
    }
    let sensitivity = state.sensitivity(valid);
    controls
        .mode_selector
        .set_sensitive(sensitivity.mode_controls);
    controls.entry.set_sensitive(sensitivity.target_controls);
    controls.existing.set_sensitive(sensitivity.target_controls);
    controls.primary.set_sensitive(sensitivity.primary);
    controls.cancel.set_sensitive(sensitivity.cancel);
    controls
        .dialog
        .set_deletable(state.dismissal_allowed(WorktreeDialogDismissal::WindowClose));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_copy_keeps_target_and_consequences_explicit() {
        let cases = [
            (
                WorktreeDialogMode::Create,
                "Create worktree",
                "New branch",
                "feature/login",
                "Creates a linked checkout and opens it as a workspace.",
            ),
            (
                WorktreeDialogMode::Attach,
                "Attach worktree",
                "Existing branch or worktree",
                "branch or worktree name",
                "Reuses an existing branch without creating a new one.",
            ),
            (
                WorktreeDialogMode::Merge,
                "Merge worktree",
                "Worktree to merge",
                "worktree or branch name",
                "The base checkout shown above receives the merge.",
            ),
            (
                WorktreeDialogMode::Remove,
                "Remove worktree",
                "Worktree to remove",
                "worktree or branch name",
                "The git branch remains intact.",
            ),
        ];

        for (mode, title, target, placeholder, hint) in cases {
            assert_eq!(mode.dialog_title(), title);
            assert_eq!(mode.target_label(), target);
            assert_eq!(mode.placeholder(), placeholder);
            assert_eq!(mode.hint(), hint);
        }
    }

    #[test]
    fn context_copy_distinguishes_discovery_from_destructive_operations() {
        assert_eq!(WorktreeDialogMode::Create.context_label(), "Starting from");
        assert!(!WorktreeDialogMode::Create.uses_workspace_repo_base());
        assert_eq!(WorktreeDialogMode::Attach.context_label(), "Starting from");
        assert!(!WorktreeDialogMode::Attach.uses_workspace_repo_base());
        assert_eq!(WorktreeDialogMode::Merge.context_label(), "Merging into");
        assert!(WorktreeDialogMode::Merge.uses_workspace_repo_base());
        assert_eq!(WorktreeDialogMode::Remove.context_label(), "Repository");
        assert!(WorktreeDialogMode::Remove.uses_workspace_repo_base());
    }

    #[test]
    fn repository_context_keeps_base_and_source_identity_visible() {
        let source = (
            "feature/login".to_string(),
            PathBuf::from("/tmp/project/.worktrees/feature-login"),
        );

        assert_eq!(
            repository_context_text(Some(&source), Some(Path::new("/tmp/project")), "fallback"),
            "/tmp/project\nFrom feature/login · .worktrees/feature-login"
        );
        assert_eq!(
            repository_context_text(
                Some(&("main".to_string(), PathBuf::from("/tmp/project"))),
                Some(Path::new("/tmp/project")),
                "fallback",
            ),
            "/tmp/project\nFrom main"
        );
    }

    #[test]
    fn destructive_context_keeps_modeled_source_when_live_cwd_leaves_workspace() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            outside_dir.path().join("forktty.sock"),
        )
        .with_notification_dispatch(false);
        {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("feature", workspace_dir.path());
            assert!(model.set_surface_cwd(
                &workspace.focused_surface_id,
                outside_dir.path().to_path_buf()
            ));
        }

        let context = worktree_dialog_context(&state);

        assert_eq!(context.discovery_cwd.as_deref(), Some(outside_dir.path()));
        assert_eq!(context.repo_cwd.as_deref(), Some(workspace_dir.path()));
        assert_eq!(
            context.discovery_text,
            format!("feature\n{}", outside_dir.path().display())
        );
        assert_eq!(
            context.repo_text,
            format!("{}\nFrom feature", workspace_dir.path().display())
        );
    }

    #[test]
    fn worktree_dialog_context_stays_with_one_workspace_snapshot() {
        let first_dir = tempfile::tempdir().unwrap();
        let first_live_dir = tempfile::tempdir().unwrap();
        let second_dir = tempfile::tempdir().unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            second_dir.path().join("forktty.sock"),
        )
        .with_notification_dispatch(false);
        let first_id = {
            let mut model = model.lock().unwrap();
            let first = model.create_workspace("first", first_dir.path());
            assert!(model.set_surface_cwd(
                &first.focused_surface_id,
                first_live_dir.path().to_path_buf()
            ));
            let first_id = first.id.clone();
            model.create_workspace("second", second_dir.path());
            model
                .select_workspace(WorkspaceSelector::Id(&first_id))
                .unwrap();
            first_id
        };

        let snapshot = active_worktree_snapshot(&state).unwrap();
        model
            .lock()
            .unwrap()
            .select_workspace(WorkspaceSelector::Name("second"))
            .unwrap();
        let context = worktree_dialog_context_from_snapshot(Some(snapshot));

        assert_eq!(
            context.discovery_cwd.as_deref(),
            Some(first_live_dir.path())
        );
        assert_eq!(context.repo_cwd.as_deref(), Some(first_dir.path()));
        assert_eq!(
            context.discovery_text,
            format!("first\n{}", first_live_dir.path().display())
        );
        assert_eq!(
            context.repo_text,
            format!("{}\nFrom first", first_dir.path().display())
        );
        assert_ne!(
            model.lock().unwrap().active_workspace_id().as_deref(),
            Some(first_id.as_str())
        );
    }

    #[test]
    fn discovery_status_preserves_mode_consequences() {
        assert_eq!(
            worktree_dialog_hint(WorktreeDialogMode::Remove, WorktreeDiscoveryState::Failed),
            "Could not load linked worktrees. Type a worktree or branch name. The git branch remains intact."
        );
        assert_eq!(
            worktree_dialog_hint(WorktreeDialogMode::Merge, WorktreeDiscoveryState::Loading),
            "Loading linked worktrees… The base checkout shown above receives the merge."
        );
    }

    #[test]
    fn delayed_discovery_preserves_user_typed_target_byte_for_byte() {
        let typed_target = "  feature/naïve=value  ";
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Merge);
        state.user_typed_target();

        let discovery = state.discovery_ready(typed_target, Some("feature/discovered"));

        assert_eq!(discovery.target, typed_target);
        assert_eq!(state.target_source, WorktreeTargetSource::UserTyped);
    }

    #[test]
    fn delayed_discovery_adopts_first_choice_when_target_is_untouched() {
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Remove);

        let discovery = state.discovery_ready("", Some("feature/discovered"));

        assert_eq!(discovery.target, "feature/discovered");
        assert_eq!(state.target_source, WorktreeTargetSource::ExistingSelection);
    }

    #[test]
    fn delayed_discovery_refresh_preserves_a_newer_operation_error() {
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Merge);
        let discovery = state.discovery_ready("", Some("feature/discovered"));

        let (_, status_update) =
            worktree_dialog_target_status(&discovery.target, discovery.status_refresh);

        assert_eq!(status_update, WorktreeDialogStatusUpdate::Preserve);
        assert_eq!(
            state.discovery_failed(),
            WorktreeDialogStatusRefresh::Preserve
        );
    }

    #[test]
    fn running_operation_disables_every_dialog_control() {
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Create);
        assert_eq!(state.begin_operation(), Some(WorktreeDialogMode::Create));

        assert_eq!(
            state.sensitivity(true),
            WorktreeDialogSensitivity {
                mode_controls: false,
                target_controls: false,
                primary: false,
                cancel: false,
            }
        );
    }

    #[test]
    fn duplicate_activation_is_rejected_while_operation_is_running() {
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Attach);

        assert_eq!(state.begin_operation(), Some(WorktreeDialogMode::Attach));
        assert_eq!(state.begin_operation(), None);
        assert_eq!(state.running, Some(WorktreeDialogMode::Attach));
    }

    #[test]
    fn close_escape_and_cancel_are_blocked_while_operation_is_running() {
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Merge);
        assert!(state.begin_operation().is_some());

        for route in [
            WorktreeDialogDismissal::WindowClose,
            WorktreeDialogDismissal::Escape,
            WorktreeDialogDismissal::Cancel,
        ] {
            assert!(!state.dismissal_allowed(route), "{route:?} must be blocked");
        }
    }

    #[test]
    fn operation_failure_reenables_interaction() {
        let mut state = WorktreeDialogState::new(WorktreeDialogMode::Create);
        assert!(state.begin_operation().is_some());

        state.finish_operation();

        assert_eq!(
            state.sensitivity(true),
            WorktreeDialogSensitivity {
                mode_controls: true,
                target_controls: true,
                primary: true,
                cancel: true,
            }
        );
        assert!(state.dismissal_allowed(WorktreeDialogDismissal::WindowClose));
    }
}

#[derive(Clone, Copy)]
pub(super) enum WorktreeAction {
    Create,
    Attach,
}

#[cfg(test)]
pub(super) fn open_worktree_from_gtk(
    state: &SocketAppState,
    name: &str,
    action: WorktreeAction,
) -> Result<(), String> {
    glib::MainContext::new().block_on(open_worktree_from_gtk_async(state, name, action))
}

#[cfg(test)]
pub(super) async fn open_worktree_from_gtk_async(
    state: &SocketAppState,
    name: &str,
    action: WorktreeAction,
) -> Result<(), String> {
    // Create/Attach intentionally follow the live shell discovery base so a
    // pane that `cd`'d into another repo can open a worktree there.
    let cwd = active_worktree_discovery_base(state)
        .map(|(_, cwd)| cwd)
        .ok_or_else(no_active_workspace_message)?;
    open_worktree_from_gtk_async_at_cwd(state, &cwd, name, action).await
}

pub(super) async fn open_worktree_from_gtk_async_at_cwd(
    state: &SocketAppState,
    cwd: &Path,
    name: &str,
    action: WorktreeAction,
) -> Result<(), String> {
    let name = validate_worktree_name_for_gtk(name)?.to_string();
    let cwd = cwd.to_string_lossy().to_string();
    let (_workspace, info) = {
        let cwd = cwd.clone();
        let name = name.clone();
        forktty_socket::open_worktree_transaction(state, cwd.clone(), move || {
            // Config read stays off the main thread alongside the git work.
            let layout = config::load_config()
                .ok()
                .map(|config| config.general.worktree_layout)
                .filter(|layout| !layout.trim().is_empty())
                .unwrap_or_else(|| "nested".to_string());
            match action {
                WorktreeAction::Create => worktree::create(&cwd, &name, &layout),
                WorktreeAction::Attach => worktree::attach(&cwd, &name, &layout),
            }
        })
        .await
        .map_err(|err| err.to_string())?
    };
    save_session_from_state(state);
    if let Some(warning) = &info.setup_warning {
        create_global_notification(
            state,
            "Worktree setup hook failed",
            warning,
            NotificationKind::Error,
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn remove_worktree_from_gtk(state: &SocketAppState, name: &str) -> Result<(), String> {
    glib::MainContext::new().block_on(remove_worktree_from_gtk_async(state, name))
}

pub(super) async fn remove_worktree_from_gtk_async(
    state: &SocketAppState,
    name: &str,
) -> Result<(), String> {
    // Destructive ops must use the modeled workspace checkout, never the live
    // shell CWD (which can point outside the repo or at an unrelated tree).
    let cwd = active_workspace_repo_cwd(state).ok_or_else(no_active_workspace_message)?;
    remove_worktree_from_gtk_async_at_cwd(state, &cwd, name).await
}

pub(super) async fn remove_worktree_from_gtk_async_at_cwd(
    state: &SocketAppState,
    cwd: &Path,
    name: &str,
) -> Result<(), String> {
    let name = validate_worktree_name_for_gtk(name)?.to_string();
    let cwd = cwd.to_string_lossy().to_string();
    forktty_socket::remove_worktree_transaction(state, cwd, name)
        .await
        .map_err(|err| err.to_string())?;
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep a workspace terminal alive: {err}");
    }
    save_session_from_state(state);
    Ok(())
}

#[cfg(test)]
pub(super) fn merge_worktree_from_gtk(
    state: &SocketAppState,
    name: &str,
) -> Result<String, String> {
    glib::MainContext::new().block_on(merge_worktree_from_gtk_async(state, name))
}

pub(super) async fn merge_worktree_from_gtk_async(
    state: &SocketAppState,
    name: &str,
) -> Result<String, String> {
    // Merge into the modeled workspace checkout, not the ephemeral shell CWD.
    let cwd = active_workspace_repo_cwd(state).ok_or_else(no_active_workspace_message)?;
    merge_worktree_from_gtk_async_at_cwd(state, &cwd, name).await
}

pub(super) async fn merge_worktree_from_gtk_async_at_cwd(
    state: &SocketAppState,
    cwd: &Path,
    name: &str,
) -> Result<String, String> {
    let name = validate_worktree_name_for_gtk(name)?.to_string();
    let cwd = cwd.to_string_lossy().to_string();
    let result =
        forktty_socket::run_guarded_worktree_write(state, move || worktree::merge(&cwd, &name))
            .await
            .map_err(|err| err.to_string())?;
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

#[cfg(test)]
pub(super) fn active_workspace_cwd_string(state: &SocketAppState) -> Result<String, String> {
    // Test helper for the Create/Attach discovery base (live shell CWD).
    active_workspace_cwd(state)
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(no_active_workspace_message)
}

pub(super) fn no_active_workspace_message() -> String {
    "No active workspace is available for worktree operations.".to_string()
}

/// Create/Attach discovery base (live shell CWD preferred). Not for Remove/Merge.
#[cfg(test)]
pub(super) fn active_workspace_cwd(state: &SocketAppState) -> Option<PathBuf> {
    active_worktree_discovery_base(state).map(|(_, cwd)| cwd)
}

#[cfg(test)]
pub(super) fn active_workspace_repo_cwd_string(state: &SocketAppState) -> Result<String, String> {
    active_workspace_repo_cwd(state)
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(no_active_workspace_message)
}
