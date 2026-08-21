//! Embedded Ghostty spawn orchestration for `TerminalController`.

use super::*;

impl TerminalController {
    /// Build a PTY-persistence plan for this spawn, or `None` to spawn the
    /// child directly (the default). Persistence is opt-in via
    /// `general.persist_terminal_processes`, requires a detach/reattach broker
    /// on `PATH`, and applies only to plain interactive terminal surfaces:
    /// agent panes persist through provider resume, SSH is already remote, and
    /// browser surfaces are not terminals. The socket path is derived from the
    /// persisted surface id under the runtime dir, so a UI restart re-attaches
    /// the same surface to its surviving process tree without extra state.
    fn pty_persistence_plan(
        &self,
        request: &SpawnRequest,
        config: &config::AppConfig,
    ) -> Option<forktty_core::pty_persistence::PtyPersistencePlan> {
        if !config.general.persist_terminal_processes {
            return None;
        }
        if !request.eligible_for_pty_persistence {
            return None;
        }
        let is_plain_terminal_surface = self
            .model
            .lock()
            .ok()
            .and_then(|model| {
                model.surface(&request.surface_id).map(|surface| {
                    matches!(surface.kind, forktty_core::SurfaceKind::Terminal)
                        && surface.agent_session.is_none()
                })
            })
            .unwrap_or(false);
        if !is_plain_terminal_surface {
            return None;
        }
        let persistence = forktty_core::pty_persistence::detect()?;
        let runtime_dir = request.socket_path.parent()?;
        let socket = match forktty_core::pty_persistence::session_socket_path(
            runtime_dir,
            &request.surface_id,
        ) {
            Ok(socket) => socket,
            Err(err) => {
                eprintln!(
                    "ForkTTY: disabling PTY persistence for {} (invalid socket path): {err}",
                    request.surface_id
                );
                return None;
            }
        };
        if let Err(err) = forktty_core::pty_persistence::ensure_private_session_dir(&socket) {
            eprintln!(
                "ForkTTY: disabling PTY persistence for {} (cannot prepare socket dir): {err}",
                request.surface_id
            );
            return None;
        }
        match forktty_core::pty_persistence::PtyPersistencePlan::new(&persistence, socket) {
            Ok(plan) => Some(plan),
            Err(err) => {
                eprintln!(
                    "ForkTTY: disabling PTY persistence for {} (invalid plan): {err}",
                    request.surface_id
                );
                None
            }
        }
    }

    pub(super) fn spawn_embedded_ghostty(
        &mut self,
        request: SpawnRequest,
        generation: u64,
    ) -> Result<(), String> {
        validate_spawn_cwd(&request.cwd)?;
        let embedder = self.embedded_ghostty()?;
        let config = config::load_config().unwrap_or_default();
        let persistent_scrollback_lines = config.appearance.persistent_scrollback_lines;
        let terminal_appearance = ghostty_terminal_appearance_for_config(&config);
        let scrollback_limit_bytes =
            embedded_ghostty_scrollback_limit_bytes_for_appearance(&terminal_appearance);
        let widget = if embedder.supports_spawn_command() {
            let persistence = self.pty_persistence_plan(&request, &config);
            let argv = forktty_terminal::spawn::embedded_ghostty_command_argv_with_persistence(
                &request,
                persistence.as_ref(),
            )?;
            unsafe {
                embedder.create_widget_for_cwd_and_command(
                    Some(&request.cwd),
                    &argv,
                    scrollback_limit_bytes,
                )?
            }
        } else {
            eprintln!(
                "Embedded Ghostty GTK library does not export command-spawn support; \
                 starting Ghostty's default shell without ForkTTY environment injection"
            );
            unsafe { embedder.create_widget_for_cwd(Some(&request.cwd))? }
        };
        {
            let embedder = Rc::clone(&embedder);
            let backend = self.backend.clone();
            let surface_id = request.surface_id.clone();
            let terminal_zoom_level = Rc::clone(&self.terminal_zoom_level);
            let weak_widget = widget.downgrade();
            widget.connect_local("init", false, move |_| {
                let widget = weak_widget.upgrade()?;
                let embedder = Rc::clone(&embedder);
                let backend = backend.clone();
                let surface_id = surface_id.clone();
                let terminal_zoom_level = Rc::clone(&terminal_zoom_level);
                // Let every init handler finish before changing font size, but
                // use default-priority timeout dispatch: Ghostty's continuous
                // frame/wakeup sources can indefinitely starve an idle source.
                // Reset first because zoom actions issued while this pane was
                // still initializing may or may not have reached Ghostty.
                glib::timeout_add_local_once(Duration::ZERO, move || {
                    let _ = run_embedded_callback_for_generation(
                        &backend,
                        &surface_id,
                        generation,
                        || {
                            match unsafe {
                                embedder.perform_action(
                                    &widget,
                                    EmbeddedSurfaceAction::ResetFontSize,
                                )
                            } {
                                Ok(true) => {}
                                Ok(false) => {
                                    eprintln!(
                                        "ForkTTY: embedded Ghostty rejected inherited zoom reset for {surface_id}"
                                    );
                                    return;
                                }
                                Err(err) => {
                                    eprintln!(
                                        "ForkTTY: cannot reset inherited zoom for {surface_id}: {err}"
                                    );
                                    return;
                                }
                            }
                            let Some((action, steps)) =
                                embedded_zoom_adjustment(terminal_zoom_level.get())
                            else {
                                return;
                            };
                            for _ in 0..steps {
                                match unsafe { embedder.perform_action(&widget, action) } {
                                    Ok(true) => {}
                                    Ok(false) => {
                                        eprintln!(
                                            "ForkTTY: embedded Ghostty rejected inherited zoom for {surface_id}"
                                        );
                                        break;
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "ForkTTY: cannot apply inherited zoom to {surface_id}: {err}"
                                        );
                                        break;
                                    }
                                }
                            }
                        },
                    );
                });
                None
            });
        }
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        let view = build_embedded_ghostty_scroll_view(&widget, terminal_appearance.scrollbar);
        install_embedded_ghostty_accelerators(&widget, Rc::clone(&embedder));
        if let Some(state) = self.state.as_ref() {
            install_embedded_ghostty_context_menu(
                &widget,
                &request.surface_id,
                state,
                &self.parent_window,
                Rc::clone(&embedder),
            );
        }

        let focus = gtk::EventControllerFocus::new();
        {
            let backend = self.backend.clone();
            let model = self.model.clone();
            let surface_id = request.surface_id.clone();
            focus.connect_enter(move |_| {
                apply_embedded_focus_for_generation(&backend, &model, &surface_id, generation);
            });
        }
        widget.add_controller(focus);

        if embedder.supports_send_text() {
            let backend = self.backend.clone();
            let surface_id = request.surface_id.clone();
            let embedder = Rc::clone(&embedder);
            widget.connect_local("init", false, move |args| {
                if let Err(err) = backend.mark_surface_ready_for_generation(&surface_id, generation)
                {
                    if !matches!(err, TerminalError::NotFound(_) | TerminalError::NotReady(_)) {
                        eprintln!(
                            "Failed to mark embedded Ghostty GTK surface ready {surface_id}: {err}"
                        );
                    }
                    return None;
                }
                if let Some(widget) = args
                    .first()
                    .and_then(|value| value.get::<gtk::Widget>().ok())
                {
                    sync_embedded_ghostty_surface_size(
                        &backend,
                        &embedder,
                        &widget,
                        &surface_id,
                        generation,
                    );
                }
                None
            });
        } else {
            eprintln!(
                "Embedded Ghostty GTK pane {} is not socket-ready: \
                 library does not export send-text support",
                request.surface_id
            );
        }

        // Scrollback restore parity: once the surface initializes, seed any
        // persisted scrollback into its terminal/display state through the
        // embedding ABI. This feeds Ghostty's VT stream, never the child PTY, so
        // old output is not re-sent as shell input. A library built before the
        // restore symbol degrades to a no-op (see supports_restore_scrollback).
        if persistent_scrollback_lines > 0 && embedder.supports_restore_scrollback() {
            let backend = self.backend.clone();
            let model = self.model.clone();
            let embedder = Rc::clone(&embedder);
            let surface_id = request.surface_id.clone();
            let weak_widget = widget.downgrade();
            widget.connect_local("init", false, move |_| {
                let widget = weak_widget.upgrade()?;
                let _ = run_embedded_callback_for_generation(
                    &backend,
                    &surface_id,
                    generation,
                    || {
                        let bytes = model.lock().ok().and_then(|model| {
                            let stored = model
                                .surface(&surface_id)
                                .and_then(|surface| surface.persisted_scrollback.as_deref());
                            embedded_scrollback_restore_bytes(
                                persistent_scrollback_lines,
                                embedder.supports_restore_scrollback(),
                                stored,
                            )
                        });
                        if let Some(bytes) = bytes {
                            if let Err(err) =
                                unsafe { embedder.restore_scrollback(&widget, &bytes) }
                            {
                                eprintln!(
                                    "Failed to restore embedded Ghostty scrollback {surface_id}: {err}"
                                );
                            }
                        }
                    },
                );
                None
            });
        }

        {
            let backend = self.backend.clone();
            let embedder = Rc::clone(&embedder);
            let surface_id = request.surface_id.clone();
            widget.connect_notify_local(Some("width"), move |widget, _| {
                sync_embedded_ghostty_surface_size(
                    &backend,
                    &embedder,
                    widget,
                    &surface_id,
                    generation,
                );
            });
        }
        {
            let backend = self.backend.clone();
            let embedder = Rc::clone(&embedder);
            let surface_id = request.surface_id.clone();
            widget.connect_notify_local(Some("height"), move |widget, _| {
                sync_embedded_ghostty_surface_size(
                    &backend,
                    &embedder,
                    widget,
                    &surface_id,
                    generation,
                );
            });
        }

        // Title parity with classic panes: mirror the Ghostty surface title
        // into the model so the pane header / surface list stay accurate.
        {
            let backend = self.backend.clone();
            let model = self.model.clone();
            let surface_id = request.surface_id.clone();
            widget.connect_notify_local(Some("title"), move |widget, _| {
                let title = widget
                    .property::<Option<String>>("title")
                    .unwrap_or_default();
                if embedded_title_is_launcher_wrapper(&title) {
                    return;
                }
                apply_embedded_title_for_generation(
                    &backend,
                    &model,
                    &surface_id,
                    generation,
                    title,
                );
            });
        }

        // Exit/readiness parity: when the child process exits, drop the surface
        // out of the ready set and reflect the closed state in its status. The
        // exact exit code is read through the embedded ABI when available; if
        // the loaded library predates that getter it stays None and the status
        // is the neutral "Closed" (see embedded_child_exit_status).
        {
            let backend = self.backend.clone();
            let model = self.model.clone();
            let embedder = Rc::clone(&embedder);
            let surface_pids = self.surface_pids.clone();
            let surface_id = request.surface_id.clone();
            let workspace_id = request.workspace_id.clone();
            let child_exit_handled = Cell::new(false);
            widget.connect_notify_local(Some("child-exited"), move |widget, _| {
                if child_exit_handled.get() || !widget.property::<bool>("child-exited") {
                    return;
                }
                // Capture the final scrollback before teardown so session save
                // keeps the exited pane's output. Read first, then store under a
                // brief lock; never hold the model lock across the ABI read.
                snapshot_embedded_scrollback_tail_to_model_for_generation(
                    &backend,
                    &model,
                    &embedder,
                    widget,
                    &surface_id,
                    generation,
                    persistent_scrollback_lines,
                );
                let exit_code = unsafe { embedder.surface_exit_code(widget) };
                let notification = match commit_embedded_child_exit_for_generation(
                    &backend,
                    &model,
                    &workspace_id,
                    &surface_id,
                    generation,
                    exit_code,
                    || {
                        remove_surface_pid_for_generation(
                            &mut surface_pids.borrow_mut(),
                            &surface_id,
                            generation,
                        );
                    },
                ) {
                    Ok(notification) => {
                        child_exit_handled.set(true);
                        notification
                    }
                    Err(TerminalError::NotFound(_) | TerminalError::NotReady(_)) => return,
                    Err(err) => {
                        eprintln!(
                            "Failed to commit embedded Ghostty child exit {surface_id}: {err}"
                        );
                        return;
                    }
                };
                if let Some(creation) = notification {
                    for notification_id in creation.evicted_desktop_notification_ids {
                        close_desktop_notification(&notification_id);
                    }
                    dispatch_notification_with_loaded_config(&creation.notification);
                }
            });
        }

        // Clean close parity: when Ghostty requests closure (e.g. the user
        // closes the surface), use the same confirmation as a classic pane.
        // Defer dialog creation so the signal emission never mutates GTK state.
        if let Some(state) = self.state.clone() {
            let backend = self.backend.clone();
            let parent_window = self.parent_window.downgrade();
            let surface_id = request.surface_id.clone();
            widget.connect_local("close-request", false, move |_| {
                let parent_window = parent_window.upgrade()?;
                defer_embedded_close_request_confirmation(
                    parent_window,
                    state.clone(),
                    backend.clone(),
                    surface_id.clone(),
                    generation,
                );
                None
            });
        }

        // PID parity: record the child PID for listening-port discovery and the
        // socket `surfaces` PID field, matching the classic spawn callback. The
        // PID lands on the surface shortly after init, so poll briefly until the
        // embedded ABI returns it. Skipped when the loaded library predates the
        // child-pid getter so older libs don't spin a pointless timer.
        if embedder.supports_child_pid() {
            let backend = self.backend.clone();
            let embedder = Rc::clone(&embedder);
            let surface_pids = self.surface_pids.clone();
            let surface_id = request.surface_id.clone();
            let weak_widget = widget.downgrade();
            let mut attempts: u32 = 0;
            glib::timeout_add_local(EMBEDDED_GHOSTTY_PID_POLL_INTERVAL, move || {
                let Some(widget) = weak_widget.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                attempts += 1;
                if widget.property::<bool>("child-exited")
                    || run_embedded_callback_for_generation(
                        &backend,
                        &surface_id,
                        generation,
                        || (),
                    )
                    .is_none()
                {
                    return glib::ControlFlow::Break;
                }
                // Only the widget getter can associate a concurrently spawned
                // child with this surface; a process-wide child list cannot.
                let child_pid = unsafe { embedder.surface_child_pid(&widget) };
                if let Some(pid) = child_pid {
                    match commit_embedded_surface_pid_for_generation(
                        &backend,
                        &surface_id,
                        generation,
                        pid,
                        |entry| {
                            surface_pids.borrow_mut().insert(surface_id.clone(), entry);
                        },
                    ) {
                        Ok(_) | Err(TerminalError::NotFound(_) | TerminalError::NotReady(_)) => {}
                        Err(err) => eprintln!(
                            "Failed to record embedded Ghostty surface pid {surface_id}: {err}"
                        ),
                    }
                    return glib::ControlFlow::Break;
                }
                if attempts >= EMBEDDED_GHOSTTY_PID_POLL_MAX_ATTEMPTS {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        }

        // Scrollback snapshot parity: embedded panes have no PTY pump loop, so a
        // throttled poll snapshots the scrollback tail into the model. That way a
        // later session save (or app close) persists recent output even without a
        // clean child exit. Config is reloaded each tick so toggling
        // persistent_scrollback_lines at runtime takes effect, matching the
        // classic pump. The ABI read happens outside the model lock, and an
        // unchanged tail skips the model write to avoid churn.
        if embedder.supports_read_text() {
            let backend = self.backend.clone();
            let embedder = Rc::clone(&embedder);
            let model = self.model.clone();
            let surface_id = request.surface_id.clone();
            let weak_widget = widget.downgrade();
            let mut skip_initial_snapshot = model.lock().ok().is_some_and(|model| {
                let stored = model
                    .surface(&surface_id)
                    .and_then(|surface| surface.persisted_scrollback.as_deref());
                stored.is_some_and(|persisted_scrollback| {
                    should_skip_initial_embedded_scrollback_snapshot(
                        embedder.supports_restore_scrollback(),
                        Some(persisted_scrollback),
                    )
                })
            });
            let mut last_snapshot: Option<String> = None;
            glib::timeout_add_local(EMBEDDED_GHOSTTY_SCROLLBACK_SNAPSHOT_INTERVAL, move || {
                let Some(widget) = weak_widget.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if run_embedded_callback_for_generation(&backend, &surface_id, generation, || ())
                    .is_none()
                {
                    return glib::ControlFlow::Break;
                }
                let lines = config::load_config()
                    .map(|config| config.appearance.persistent_scrollback_lines)
                    .unwrap_or(0);
                if lines == 0 {
                    return glib::ControlFlow::Continue;
                }
                if let Some(text) =
                    read_embedded_scrollback_tail(&embedder, &widget, &surface_id, lines)
                {
                    if skip_initial_snapshot {
                        last_snapshot = Some(text);
                        skip_initial_snapshot = false;
                        return glib::ControlFlow::Continue;
                    }
                    if last_snapshot.as_deref() != Some(text.as_str()) {
                        if !commit_embedded_scrollback_for_generation(
                            &backend,
                            &model,
                            &surface_id,
                            generation,
                            text.clone(),
                        ) {
                            return glib::ControlFlow::Break;
                        }
                        last_snapshot = Some(text);
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        let chrome = build_embedded_ghostty_pane_chrome(
            &request.surface_id,
            &view,
            self.state.as_ref(),
            &self.parent_window,
        );
        let backend = self.backend.clone();
        let allocation_surface_id = request.surface_id.clone();
        let allocation_widget = widget.downgrade();
        // Ghostty creates its core surface on the GL area's first non-zero
        // resize. After rapid stack rebuilds GTK can map the new child at 0x0
        // and never renegotiate it, leaving the pane permanently uninitialized.
        // Retry layout for a bounded number of frames: a single queued resize
        // is not sufficient with the GTK 4.14 stack bundled in the AppImage.
        let layout_attempts = Cell::new(0_u8);
        self.container.add_tick_callback(move |container, _| {
            let Some(widget) = allocation_widget.upgrade() else {
                return glib::ControlFlow::Break;
            };
            run_embedded_initial_layout_tick(
                &backend,
                &allocation_surface_id,
                generation,
                container.upcast_ref(),
                &widget,
                &layout_attempts,
            )
        });
        let commit_backend = self.backend.clone();
        let surface_id = request.surface_id.clone();
        let workspace_id = request.workspace_id.clone();
        if run_embedded_callback_for_generation(&commit_backend, &surface_id, generation, || {
            if let Ok(mut model) = self.model.lock() {
                let _ = model.clear_status(&workspace_id, Some(&surface_status_key(&surface_id)));
            }
            self.chromes.insert(surface_id.clone(), chrome);
            self.embedded_ghostty_panes.insert(
                surface_id.clone(),
                EmbeddedGhosttyPane {
                    surface: widget,
                    generation,
                },
            );
            self.rebuild_layout_without_surface_reconciliation();
        })
        .is_none()
        {
            return Ok(());
        }
        Ok(())
    }

    pub(super) fn embedded_ghostty(&mut self) -> Result<Rc<GhosttyGtkEmbedder>, String> {
        if let Some(embedder) = &self.embedded_ghostty {
            return Ok(Rc::clone(embedder));
        }
        let embedder = Rc::new(unsafe { GhosttyGtkEmbedder::load()? });
        if embedder.supports_wakeup_callback() {
            let embedder_for_wakeup = Rc::clone(&embedder);
            let mut last_context_tick = Instant::now() - EMBEDDED_GHOSTTY_CONTEXT_TICK_MIN_INTERVAL;
            glib::timeout_add_local(EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL, move || {
                if embedder_for_wakeup.has_pending_wakeup()
                    && last_context_tick.elapsed() >= EMBEDDED_GHOSTTY_CONTEXT_TICK_MIN_INTERVAL
                    && embedder_for_wakeup.take_pending_wakeup()
                {
                    unsafe {
                        embedder_for_wakeup.tick();
                    }
                    last_context_tick = Instant::now();
                }
                glib::ControlFlow::Continue
            });
        } else {
            let embedder_for_tick = Rc::clone(&embedder);
            glib::timeout_add_local(EMBEDDED_GHOSTTY_CONTEXT_TICK_FALLBACK_INTERVAL, move || {
                unsafe {
                    embedder_for_tick.tick();
                }
                glib::ControlFlow::Continue
            });
        }
        self.embedded_ghostty = Some(Rc::clone(&embedder));
        Ok(embedder)
    }
}

fn defer_embedded_close_request_confirmation(
    parent_window: adw::ApplicationWindow,
    state: SocketAppState,
    backend: Arc<GtkTerminalBackend>,
    surface_id: String,
    generation: u64,
) {
    defer_embedded_close_request_confirmation_with_completion(
        parent_window,
        state,
        backend,
        surface_id,
        generation,
        Rc::new(|_| {}),
    );
}

fn defer_embedded_close_request_confirmation_with_completion(
    parent_window: adw::ApplicationWindow,
    state: SocketAppState,
    backend: Arc<GtkTerminalBackend>,
    surface_id: String,
    generation: u64,
    close_completed: Rc<dyn Fn(GenerationCloseOutcome)>,
) {
    glib::idle_add_local_once(move || {
        let backend_for_confirmation = backend.clone();
        let surface_id_for_confirmation = surface_id.clone();
        let state_for_close = state.clone();
        let backend_for_close = backend.clone();
        let surface_id_for_close = surface_id.clone();
        let close_completed_for_close = Rc::clone(&close_completed);
        let _ = run_embedded_callback_for_generation(&backend, &surface_id, generation, || {
            crate::gtk_app::pane_chrome::show_close_pane_confirmation_if(
                &parent_window,
                &state,
                &surface_id,
                Rc::new(move || {
                    backend_for_confirmation
                        .surface_generation_is_current(&surface_id_for_confirmation, generation)
                        .unwrap_or(false)
                }),
                Rc::new(move || {
                    let close_completed = Rc::clone(&close_completed_for_close);
                    close_surface_by_id_for_generation(
                        &state_for_close,
                        &backend_for_close,
                        &surface_id_for_close,
                        generation,
                        move |outcome| close_completed(outcome),
                    );
                }),
            );
        });
    });
}

/// Refuse to spawn into a missing working directory. Upstream Ghostty treats
/// an inaccessible cwd as "ignore and inherit" (src/termio/Exec.zig), which
/// would silently hand the shell — and any agent driving it — a session in
/// whatever directory the ForkTTY process itself runs from, e.g. after the
/// surface's worktree was removed out from under it. Failing here routes the
/// spawn into the visible spawn-failure status instead.
fn validate_spawn_cwd(cwd: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;

    if !cwd.is_dir() {
        return Err(format!(
            "working directory {} does not exist or is not a directory",
            cwd.display()
        ));
    }
    // Existence is not enough: Ghostty's own check is access()-based, so a
    // directory this user cannot search (traverse) is "ignored and
    // inherited" exactly like a missing one.
    let c_path = std::ffi::CString::new(cwd.as_os_str().as_bytes()).map_err(|_| {
        format!(
            "working directory contains an interior NUL: {}",
            cwd.display()
        )
    })?;
    // SAFETY: plain access(2) on a NUL-terminated path.
    if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } != 0 {
        return Err(format!(
            "working directory {} is not accessible: {}",
            cwd.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn embedded_zoom_adjustment(zoom_level: i32) -> Option<(EmbeddedSurfaceAction, u32)> {
    match zoom_level.cmp(&0) {
        std::cmp::Ordering::Greater => Some((
            EmbeddedSurfaceAction::IncreaseFontSize,
            zoom_level.unsigned_abs(),
        )),
        std::cmp::Ordering::Less => Some((
            EmbeddedSurfaceAction::DecreaseFontSize,
            zoom_level.unsigned_abs(),
        )),
        std::cmp::Ordering::Equal => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_button(
        widget: &gtk::Widget,
        predicate: &impl Fn(&gtk::Button) -> bool,
    ) -> Option<gtk::Button> {
        if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
            if predicate(&button) {
                return Some(button);
            }
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(button) = find_button(&current, predicate) {
                return Some(button);
            }
            child = current.next_sibling();
        }
        None
    }

    fn embedded_close_confirmation_for(parent: &adw::ApplicationWindow) -> Option<gtk::Window> {
        gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
            .find(|window| {
                window.is_visible()
                    && window.title().as_deref() == Some("Close Pane?")
                    && window.transient_for().as_ref() == Some(parent.upcast_ref())
            })
    }

    fn surface_exists_in_backend(terminal: &dyn TerminalBackend, surface_id: &str) -> bool {
        terminal
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id)
    }

    #[test]
    fn embedded_close_request_requires_confirmation_for_live_surface() {
        let _ = crate::test_env::with_gtk_test(|| {
            crate::test_env::with_isolated_user_dirs(|| {
                let model = Arc::new(Mutex::new(WorkspaceModel::new()));
                let (terminal_tx, terminal_rx) = mpsc::channel();
                let terminal = Arc::new(GtkTerminalBackend::new(terminal_tx));
                let state = SocketAppState::new(
                    model.clone(),
                    terminal.clone(),
                    "/bin/sh",
                    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
                )
                .with_notification_dispatch(false);
                let surface_id = model
                    .lock()
                    .unwrap()
                    .create_workspace("main", Path::new("/tmp"))
                    .focused_surface_id;
                spawn_focused_surface_if_needed(&state).unwrap();
                let generation = match terminal_rx.recv().unwrap() {
                    GtkTerminalCommand::Spawn { generation, .. } => generation,
                    _ => panic!("expected embedded spawn command"),
                };
                let parent = adw::ApplicationWindow::builder().build();

                defer_embedded_close_request_confirmation(
                    parent.clone(),
                    state.clone(),
                    terminal.clone(),
                    surface_id.clone(),
                    generation,
                );

                assert!(model.lock().unwrap().surface(&surface_id).is_some());
                assert!(surface_exists_in_backend(terminal.as_ref(), &surface_id));
                while glib::MainContext::default().iteration(false) {}
                let dialog = embedded_close_confirmation_for(&parent)
                    .expect("embedded close request should show a confirmation dialog");
                assert!(model.lock().unwrap().surface(&surface_id).is_some());
                assert!(surface_exists_in_backend(terminal.as_ref(), &surface_id));

                let cancel = find_button(dialog.upcast_ref(), &|button| {
                    button.label().as_deref() == Some("Cancel")
                })
                .expect("confirmation should have a cancel button");
                cancel.emit_clicked();
                assert!(!dialog.is_visible());
                assert!(model.lock().unwrap().surface(&surface_id).is_some());
                assert!(surface_exists_in_backend(terminal.as_ref(), &surface_id));

                let (close_completed_tx, close_completed_rx) = tokio::sync::oneshot::channel();
                let close_completed_tx = Rc::new(RefCell::new(Some(close_completed_tx)));
                defer_embedded_close_request_confirmation_with_completion(
                    parent.clone(),
                    state.clone(),
                    terminal.clone(),
                    surface_id.clone(),
                    generation,
                    Rc::new(move |outcome| {
                        if let Some(sender) = close_completed_tx.borrow_mut().take() {
                            let _ = sender.send(outcome);
                        }
                    }),
                );
                while glib::MainContext::default().iteration(false) {}
                let dialog = embedded_close_confirmation_for(&parent)
                    .expect("second embedded close request should show a confirmation dialog");
                let confirm = find_button(dialog.upcast_ref(), &|button| {
                    button.has_css_class("destructive-action")
                })
                .expect("confirmation should have a destructive button");
                confirm.emit_clicked();
                let close_outcome = glib::MainContext::default()
                    .block_on(close_completed_rx)
                    .expect("generation-bound close should report completion");

                assert!(!dialog.is_visible());
                assert_eq!(close_outcome, GenerationCloseOutcome::Closed);
                assert!(model.lock().unwrap().surface(&surface_id).is_none());
                assert!(!surface_exists_in_backend(terminal.as_ref(), &surface_id));
                parent.close();
            });
        });
    }

    #[test]
    fn stale_embedded_close_request_does_not_show_confirmation() {
        let _ = crate::test_env::with_gtk_test(|| {
            crate::test_env::with_isolated_user_dirs(|| {
                let model = Arc::new(Mutex::new(WorkspaceModel::new()));
                let (terminal_tx, terminal_rx) = mpsc::channel();
                let terminal = Arc::new(GtkTerminalBackend::new(terminal_tx));
                let state = SocketAppState::new(
                    model.clone(),
                    terminal.clone(),
                    "/bin/sh",
                    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
                )
                .with_notification_dispatch(false);
                let surface_id = model
                    .lock()
                    .unwrap()
                    .create_workspace("main", Path::new("/tmp"))
                    .focused_surface_id;
                spawn_focused_surface_if_needed(&state).unwrap();
                let stale_generation = match terminal_rx.recv().unwrap() {
                    GtkTerminalCommand::Spawn { generation, .. } => generation,
                    _ => panic!("expected first embedded spawn command"),
                };
                terminal.close(&surface_id).unwrap();
                assert!(matches!(
                    terminal_rx.recv().unwrap(),
                    GtkTerminalCommand::Close { .. }
                ));
                let replacement_request = {
                    let model = model.lock().unwrap();
                    SpawnRequest::for_surface(
                        model.surface(&surface_id).unwrap(),
                        state.shell.clone(),
                        state.socket_path.clone(),
                    )
                };
                terminal.spawn(replacement_request).unwrap();
                assert!(matches!(
                    terminal_rx.recv().unwrap(),
                    GtkTerminalCommand::Spawn { .. }
                ));
                let parent = adw::ApplicationWindow::builder().build();

                defer_embedded_close_request_confirmation(
                    parent.clone(),
                    state,
                    terminal,
                    surface_id.clone(),
                    stale_generation,
                );
                while glib::MainContext::default().iteration(false) {}

                assert!(embedded_close_confirmation_for(&parent).is_none());
                assert!(model.lock().unwrap().surface(&surface_id).is_some());
                parent.close();
            });
        });
    }

    #[test]
    fn embedded_close_confirmation_does_not_close_replacement_generation() {
        let _ = crate::test_env::with_gtk_test(|| {
            crate::test_env::with_isolated_user_dirs(|| {
                let model = Arc::new(Mutex::new(WorkspaceModel::new()));
                let (terminal_tx, terminal_rx) = mpsc::channel();
                let terminal = Arc::new(GtkTerminalBackend::new(terminal_tx));
                let state = SocketAppState::new(
                    model.clone(),
                    terminal.clone(),
                    "/bin/sh",
                    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
                )
                .with_notification_dispatch(false);
                let surface_id = model
                    .lock()
                    .unwrap()
                    .create_workspace("main", Path::new("/tmp"))
                    .focused_surface_id;
                spawn_focused_surface_if_needed(&state).unwrap();
                let stale_generation = match terminal_rx.recv().unwrap() {
                    GtkTerminalCommand::Spawn { generation, .. } => generation,
                    _ => panic!("expected first embedded spawn command"),
                };
                let parent = adw::ApplicationWindow::builder().build();
                defer_embedded_close_request_confirmation(
                    parent.clone(),
                    state.clone(),
                    terminal.clone(),
                    surface_id.clone(),
                    stale_generation,
                );
                while glib::MainContext::default().iteration(false) {}
                let dialog = embedded_close_confirmation_for(&parent)
                    .expect("current embedded close request should show confirmation");

                terminal.close(&surface_id).unwrap();
                assert!(matches!(
                    terminal_rx.recv().unwrap(),
                    GtkTerminalCommand::Close { .. }
                ));
                let replacement_request = {
                    let model = model.lock().unwrap();
                    SpawnRequest::for_surface(
                        model.surface(&surface_id).unwrap(),
                        state.shell.clone(),
                        state.socket_path.clone(),
                    )
                };
                terminal.spawn(replacement_request).unwrap();
                assert!(matches!(
                    terminal_rx.recv().unwrap(),
                    GtkTerminalCommand::Spawn { .. }
                ));

                let confirm = find_button(dialog.upcast_ref(), &|button| {
                    button.has_css_class("destructive-action")
                })
                .expect("confirmation should have a destructive button");
                confirm.emit_clicked();

                assert!(model.lock().unwrap().surface(&surface_id).is_some());
                assert!(surface_exists_in_backend(terminal.as_ref(), &surface_id));
                parent.close();
            });
        });
    }

    #[test]
    fn spawn_cwd_must_be_an_existing_directory() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        assert!(validate_spawn_cwd(dir.path()).is_ok());

        let missing = dir.path().join("removed-worktree");
        let err = validate_spawn_cwd(&missing).unwrap_err();
        assert!(err.contains("removed-worktree"));

        let file = dir.path().join("plain-file");
        std::fs::write(&file, b"not a directory").unwrap();
        assert!(validate_spawn_cwd(&file).is_err());

        // An existing but unsearchable directory is inherited-and-ignored by
        // Ghostty just like a missing one. Root bypasses mode bits, so the
        // assertion only holds for ordinary users.
        if unsafe { libc::geteuid() } != 0 {
            let sealed = dir.path().join("sealed");
            std::fs::create_dir(&sealed).unwrap();
            std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
            let result = validate_spawn_cwd(&sealed);
            std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o700)).unwrap();
            let err = result.unwrap_err();
            assert!(err.contains("not accessible"), "unexpected error: {err}");
        }
    }

    #[test]
    fn embedded_zoom_adjustment_replays_the_global_level() {
        assert_eq!(embedded_zoom_adjustment(0), None);
        assert_eq!(
            embedded_zoom_adjustment(3),
            Some((EmbeddedSurfaceAction::IncreaseFontSize, 3))
        );
        assert_eq!(
            embedded_zoom_adjustment(-2),
            Some((EmbeddedSurfaceAction::DecreaseFontSize, 2))
        );
    }
}
