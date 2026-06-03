use super::*;

pub(super) fn terminal_focus_click_should_claim(
    terminal_has_focus: bool,
    model_focused_surface_id: Option<&str>,
    surface_id: &str,
) -> bool {
    !terminal_has_focus || model_focused_surface_id.is_some_and(|focused| focused != surface_id)
}

#[cfg(feature = "gtk-vte")]
pub(super) fn attach_vte_signal_handlers(
    widget: &VteTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    request: &SpawnRequest,
    surface_pids: &Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    state: Option<SocketAppState>,
    spawn_token: u64,
) {
    let surface_id = request.surface_id.clone();
    let focus_model = model.clone();
    widget.connect_has_focus_notify(move |terminal| {
        if terminal.has_focus() {
            terminal.add_css_class("focused-terminal");
            if let Ok(mut model) = focus_model.lock() {
                let _ = model.focus_surface(&surface_id);
                let _ = model.mark_surface_unread(&surface_id, false);
            }
        } else {
            terminal.remove_css_class("focused-terminal");
        }
    });

    let surface_id = request.surface_id.clone();
    let focus_click_model = model.clone();
    let focus_click = gtk::GestureClick::new();
    focus_click.set_button(gtk::gdk::BUTTON_PRIMARY);
    focus_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let widget_for_focus_click = widget.downgrade();
    focus_click.connect_pressed(move |gesture, _n_press, _x, _y| {
        let Some(widget_for_focus_click) = widget_for_focus_click.upgrade() else {
            return;
        };
        let model_focused_surface_id = focus_click_model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id);
        if !terminal_focus_click_should_claim(
            widget_for_focus_click.has_focus(),
            model_focused_surface_id.as_deref(),
            &surface_id,
        ) {
            return;
        }

        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_focus_click.grab_focus();
        if let Ok(mut model) = focus_click_model.lock() {
            let _ = model.focus_surface(&surface_id);
            let _ = model.mark_surface_unread(&surface_id, false);
        }
    });
    widget.add_controller(focus_click);

    let surface_id = request.surface_id.clone();
    let title_model = model.clone();
    widget.connect_window_title_changed(move |terminal| {
        if let Some(title) = terminal.window_title() {
            if let Ok(mut model) = title_model.lock() {
                let _ = model.set_surface_title(&surface_id, title.to_string());
            }
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let prompt_model = model.clone();
    let last_prompt_tail = Rc::new(RefCell::new(String::new()));
    let last_prompt_notification = Rc::new(RefCell::new(None));
    let visible_last_prompt = last_prompt_notification.clone();
    widget.connect_contents_changed(move |terminal| {
        let tail = visible_terminal_tail(terminal);
        if tail.is_empty() {
            return;
        }

        let mut previous = last_prompt_tail.borrow_mut();
        if previous.as_str() == tail {
            return;
        }
        *previous = tail.clone();
        drop(previous);

        if !looks_like_prompt(&tail) {
            return;
        }
        emit_prompt_notification(
            &prompt_model,
            &visible_last_prompt,
            &workspace_id,
            &surface_id,
            "A terminal appears to be waiting for input",
        );
    });

    if vte_terminal_signal_exists("shell-precmd") {
        let surface_id = request.surface_id.clone();
        let workspace_id = request.workspace_id.clone();
        let precmd_model = model.clone();
        let last_prompt = last_prompt_notification.clone();
        widget.connect_shell_precmd(move |_| {
            if let Ok(mut model) = precmd_model.lock() {
                let _ = model.set_status(
                    &workspace_id,
                    surface_status_key(&surface_id),
                    "Terminal",
                    "Ready",
                    Some("green".to_string()),
                );
            }
            emit_prompt_notification(
                &precmd_model,
                &last_prompt,
                &workspace_id,
                &surface_id,
                "Shell integration reported a ready prompt",
            );
        });
    }

    if vte_terminal_signal_exists("shell-preexec") {
        let surface_id = request.surface_id.clone();
        let workspace_id = request.workspace_id.clone();
        let preexec_model = model.clone();
        widget.connect_shell_preexec(move |_| {
            if let Ok(mut model) = preexec_model.lock() {
                let _ = model.set_status(
                    &workspace_id,
                    surface_status_key(&surface_id),
                    "Terminal",
                    "Running",
                    Some("blue".to_string()),
                );
            }
        });
    }

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let bell_model = model.clone();
    widget.connect_bell(move |_| {
        if let Ok(mut model) = bell_model.lock() {
            // The user may have closed this pane before the bell signal
            // drained from VTE. Don't materialize a notification that points
            // at a surface the model no longer knows about — the row would
            // render as a dead-end click target.
            if model.surface(&surface_id).is_none() {
                return;
            }
            let notification = model.create_notification(
                "Terminal bell",
                "A terminal requested attention",
                NotificationKind::Info,
                Some(workspace_id.clone()),
                Some(surface_id.clone()),
            );
            dispatch_notification_with_loaded_config(&notification);
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let exit_model = model.clone();
    let exit_surface_pids = surface_pids.clone();
    let exit_state = state;
    // VTE emits child-exited exactly once in normal teardown but can in rare
    // cases (force-kill, fast respawn) fire twice. A single-shot latch keeps
    // the status + notification idempotent per surface.
    let exit_fired = Rc::new(Cell::new(false));
    widget.connect_child_exited(move |_, status| {
        if exit_fired.replace(true) {
            return;
        }
        let mut pids = exit_surface_pids.borrow_mut();
        if !remove_surface_pid_for_spawn(&mut pids, &surface_id, spawn_token) {
            return;
        }
        drop(pids);
        if let Some(state) = &exit_state {
            match state.terminal.mark_surface_not_ready(&surface_id) {
                Ok(()) | Err(TerminalError::NotFound(_)) => {}
                Err(err) => {
                    eprintln!("Failed to mark terminal surface not ready {surface_id}: {err}")
                }
            }
        }
        if let Ok(mut model) = exit_model.lock() {
            if model.surface(&surface_id).is_none() {
                return;
            }
            if status == 0 {
                let _ = model.set_status(
                    &workspace_id,
                    surface_status_key(&surface_id),
                    "Terminal",
                    "Closed",
                    None,
                );
                return;
            }
            let _ = model.set_status(
                &workspace_id,
                surface_status_key(&surface_id),
                "Terminal",
                format!("Exited ({status})"),
                Some("yellow".to_string()),
            );
            let notification = model.create_notification(
                "Terminal exited",
                format!("Process exited with status {status}. Use Restart Pane to spawn it again."),
                NotificationKind::Info,
                Some(workspace_id.clone()),
                Some(surface_id.clone()),
            );
            dispatch_notification_with_loaded_config(&notification);
        }
    });
}

#[cfg(all(feature = "gtk-ghostty", not(feature = "gtk-vte")))]
pub(super) fn attach_vte_signal_handlers(
    widget: &VteTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    request: &SpawnRequest,
    _surface_pids: &Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    _state: Option<SocketAppState>,
    _spawn_token: u64,
) {
    let surface_id = request.surface_id.clone();
    let focus_model = model.clone();
    let gtk_widget = widget.widget();
    gtk_widget.connect_has_focus_notify(move |terminal| {
        if terminal.has_focus() {
            terminal.add_css_class("focused-terminal");
            if let Ok(mut model) = focus_model.lock() {
                let _ = model.focus_surface(&surface_id);
                let _ = model.mark_surface_unread(&surface_id, false);
            }
        } else {
            terminal.remove_css_class("focused-terminal");
        }
    });

    let surface_id = request.surface_id.clone();
    let focus_click_model = model.clone();
    let focus_click = gtk::GestureClick::new();
    focus_click.set_button(gtk::gdk::BUTTON_PRIMARY);
    focus_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let widget_for_focus_click = widget.downgrade();
    focus_click.connect_pressed(move |gesture, _n_press, _x, _y| {
        let Some(widget_for_focus_click) = widget_for_focus_click.upgrade() else {
            return;
        };
        let model_focused_surface_id = focus_click_model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id);
        if !terminal_focus_click_should_claim(
            widget_for_focus_click.has_focus(),
            model_focused_surface_id.as_deref(),
            &surface_id,
        ) {
            return;
        }

        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_focus_click.grab_focus();
        if let Ok(mut model) = focus_click_model.lock() {
            let _ = model.focus_surface(&surface_id);
            let _ = model.mark_surface_unread(&surface_id, false);
        }
    });
    gtk_widget.add_controller(focus_click);
}

pub(super) fn surface_status_key(surface_id: &str) -> String {
    format!("surface:{surface_id}:status")
}

#[cfg(feature = "gtk-vte")]
pub(super) fn vte_terminal_signal_exists(name: &str) -> bool {
    glib::subclass::SignalId::lookup(name, VteTerminalWidget::static_type()).is_some()
}

#[cfg(all(feature = "gtk-ghostty", not(feature = "gtk-vte")))]
pub(super) fn vte_terminal_signal_exists(_name: &str) -> bool {
    false
}

pub(super) fn emit_prompt_notification(
    model: &Arc<Mutex<WorkspaceModel>>,
    last_prompt_notification: &Rc<RefCell<Option<Instant>>>,
    workspace_id: &str,
    surface_id: &str,
    body: &str,
) {
    let now = Instant::now();
    {
        let mut last_prompt = last_prompt_notification.borrow_mut();
        if last_prompt.is_some_and(|last| now.duration_since(last) < PROMPT_NOTIFICATION_THROTTLE) {
            return;
        }
        *last_prompt = Some(now);
    }

    if let Some(notification) =
        create_prompt_notification_if_surface_exists(model, workspace_id, surface_id, body)
    {
        dispatch_notification_with_loaded_config(&notification);
    }
}

pub(super) fn create_prompt_notification_if_surface_exists(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    body: &str,
) -> Option<NotificationItem> {
    let mut model = model.lock().ok()?;
    let surface = model.surface(surface_id)?;
    if surface.workspace_id != workspace_id {
        return None;
    }
    Some(model.create_notification(
        "Terminal prompt",
        body,
        NotificationKind::Prompt,
        Some(workspace_id.to_string()),
        Some(surface_id.to_string()),
    ))
}

pub(super) fn looks_like_prompt(text: &str) -> bool {
    text.lines().rev().take(4).any(|line| {
        let trimmed = line.trim();
        trimmed == ">"
            || trimmed == "❯"
            || trimmed.contains("(Y/n)")
            || trimmed.contains("(y/N)")
            || trimmed.contains("Do you want to proceed")
            || (trimmed.starts_with("? ") && trimmed.ends_with(':'))
    })
}

pub(super) fn visible_text_tail(text: &str) -> String {
    let mut chars = text.chars().rev().take(4096).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

#[cfg(feature = "gtk-vte")]
pub(super) fn visible_terminal_tail(terminal: &VteTerminalWidget) -> String {
    const PROMPT_SCAN_ROWS: i64 = 8;

    let rows = terminal.row_count().max(1);
    let cols = terminal.column_count().max(1);
    let start_row = rows.saturating_sub(PROMPT_SCAN_ROWS);
    let (text, _) = terminal.text_range_format(Format::Text, start_row, 0, rows - 1, cols);
    let Some(text) = text else {
        return String::new();
    };
    visible_text_tail(text.as_str())
}

pub(super) fn dispatch_notification_with_loaded_config(notification: &NotificationItem) {
    if !should_dispatch_notification(notification) {
        return;
    }
    let config = config::load_config().unwrap_or_default();
    for error in dispatch_notification(&config, notification) {
        if error.channel == "desktop" && is_desktop_notification_rate_limit(&error.message) {
            continue;
        }
        eprintln!(
            "Failed to dispatch {} notification: {}",
            error.channel, error.message
        );
    }
}

pub(super) fn should_dispatch_notification(notification: &NotificationItem) -> bool {
    static RECENT_NOTIFICATIONS: OnceLock<Mutex<BTreeMap<String, Instant>>> = OnceLock::new();

    let now = Instant::now();
    let key = format!(
        "{}\n{}\n{}",
        notification_kind_class(notification.kind),
        notification.title,
        notification.body
    );
    let recent = RECENT_NOTIFICATIONS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut recent) = recent.lock() else {
        return true;
    };

    recent.retain(|_, last_seen| now.duration_since(*last_seen) < NOTIFICATION_DEDUPE_WINDOW);
    if recent
        .get(&key)
        .is_some_and(|last_seen| now.duration_since(*last_seen) < NOTIFICATION_DEDUPE_WINDOW)
    {
        return false;
    }
    recent.insert(key, now);
    true
}

pub(super) fn is_desktop_notification_rate_limit(message: &str) -> bool {
    message.contains("ExcessNotificationGeneration")
        || message.contains("too many similar notifications")
}
