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

    pub(super) fn spawn_embedded_ghostty(&mut self, request: SpawnRequest) -> Result<(), String> {
        let embedder = self.embedded_ghostty()?;
        let config = config::load_config().unwrap_or_default();
        let persistent_scrollback_lines = config.appearance.persistent_scrollback_lines;
        let terminal_appearance = ghostty_terminal_appearance_for_config(&config);
        let scrollback_limit_bytes =
            embedded_ghostty_scrollback_limit_bytes_for_appearance(&terminal_appearance);
        #[cfg(target_os = "linux")]
        let child_pids_before_spawn = current_process_child_pids();
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
            let model = self.model.clone();
            let surface_id = request.surface_id.clone();
            focus.connect_enter(move |_| {
                if let Ok(mut model) = model.lock() {
                    let _ = model.focus_surface(&surface_id);
                    let _ = model.mark_surface_unread(&surface_id, false);
                }
            });
        }
        widget.add_controller(focus);

        if embedder.supports_send_text() {
            if let Some(state) = self.state.clone() {
                let surface_id = request.surface_id.clone();
                let embedder = Rc::clone(&embedder);
                widget.connect_local("init", false, move |args| {
                    if let Err(err) = state.terminal.mark_surface_ready(&surface_id) {
                        eprintln!(
                            "Failed to mark embedded Ghostty GTK surface ready {surface_id}: {err}"
                        );
                    }
                    if let Some(widget) = args
                        .first()
                        .and_then(|value| value.get::<gtk::Widget>().ok())
                    {
                        sync_embedded_ghostty_surface_size(&state, &embedder, &widget, &surface_id);
                    }
                    None
                });
            }
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
            let model = self.model.clone();
            let embedder = Rc::clone(&embedder);
            let surface_id = request.surface_id.clone();
            let weak_widget = widget.downgrade();
            widget.connect_local("init", false, move |_| {
                let widget = weak_widget.upgrade()?;
                let stored = model.lock().ok().and_then(|model| {
                    model
                        .surface(&surface_id)
                        .and_then(|surface| surface.persisted_scrollback.clone())
                });
                if let Some(bytes) = embedded_scrollback_restore_bytes(
                    persistent_scrollback_lines,
                    embedder.supports_restore_scrollback(),
                    stored.as_deref(),
                ) {
                    if let Err(err) = unsafe { embedder.restore_scrollback(&widget, &bytes) } {
                        eprintln!(
                            "Failed to restore embedded Ghostty scrollback {surface_id}: {err}"
                        );
                    }
                }
                None
            });
        }

        if let Some(state) = self.state.clone() {
            let embedder = Rc::clone(&embedder);
            let surface_id = request.surface_id.clone();
            widget.connect_notify_local(Some("width"), move |widget, _| {
                sync_embedded_ghostty_surface_size(&state, &embedder, widget, &surface_id);
            });
        }
        if let Some(state) = self.state.clone() {
            let embedder = Rc::clone(&embedder);
            let surface_id = request.surface_id.clone();
            widget.connect_notify_local(Some("height"), move |widget, _| {
                sync_embedded_ghostty_surface_size(&state, &embedder, widget, &surface_id);
            });
        }

        self.next_spawn_token = self.next_spawn_token.checked_add(1).unwrap_or(1);
        let spawn_token = self.next_spawn_token;
        self.embedded_spawn_tokens
            .borrow_mut()
            .insert(request.surface_id.clone(), spawn_token);

        // Title parity with classic panes: mirror the Ghostty surface title
        // into the model so the pane header / surface list stay accurate.
        {
            let model = self.model.clone();
            let surface_id = request.surface_id.clone();
            widget.connect_notify_local(Some("title"), move |widget, _| {
                let title = widget
                    .property::<Option<String>>("title")
                    .unwrap_or_default();
                if embedded_title_is_launcher_wrapper(&title) {
                    return;
                }
                if let Ok(mut model) = model.lock() {
                    let _ = model.set_surface_title(&surface_id, title);
                }
            });
        }

        // Exit/readiness parity: when the child process exits, drop the surface
        // out of the ready set and reflect the closed state in its status. The
        // exact exit code is read through the embedded ABI when available; if
        // the loaded library predates that getter it stays None and the status
        // is the neutral "Closed" (see embedded_child_exit_status).
        {
            let model = self.model.clone();
            let state = self.state.clone();
            let embedder = Rc::clone(&embedder);
            let surface_pids = self.surface_pids.clone();
            let embedded_spawn_tokens = self.embedded_spawn_tokens.clone();
            let surface_id = request.surface_id.clone();
            let workspace_id = request.workspace_id.clone();
            widget.connect_notify_local(Some("child-exited"), move |widget, _| {
                if !widget.property::<bool>("child-exited") {
                    return;
                }
                if !matches!(
                    embedded_spawn_tokens.borrow().get(&surface_id),
                    Some(current) if *current == spawn_token
                ) {
                    return;
                }
                // Capture the final scrollback before teardown so session save
                // keeps the exited pane's output. Read first, then store under a
                // brief lock; never hold the model lock across the ABI read.
                snapshot_embedded_scrollback_tail_to_model(
                    &model,
                    &embedder,
                    widget,
                    &surface_id,
                    persistent_scrollback_lines,
                );
                remove_surface_pid_for_spawn(
                    &mut surface_pids.borrow_mut(),
                    &surface_id,
                    spawn_token,
                );
                embedded_spawn_tokens.borrow_mut().remove(&surface_id);
                if let Some(state) = &state {
                    match state.terminal.mark_surface_not_ready(&surface_id) {
                        Ok(()) | Err(TerminalError::NotFound(_)) => {}
                        Err(err) => eprintln!(
                            "Failed to mark embedded Ghostty GTK surface not ready {surface_id}: {err}"
                        ),
                    }
                    match state.terminal.clear_surface_pid(&surface_id) {
                        Ok(()) | Err(TerminalError::NotFound(_)) => {}
                        Err(err) => eprintln!(
                            "Failed to clear embedded Ghostty surface pid {surface_id}: {err}"
                        ),
                    }
                }
                let exit_code = unsafe { embedder.surface_exit_code(widget) };
                let notification = match model.lock() {
                    Ok(mut model) => apply_embedded_child_exit(
                        &mut model,
                        &workspace_id,
                        &surface_id,
                        exit_code,
                    ),
                    Err(_) => None,
                };
                if let Some(notification) = notification {
                    dispatch_notification_with_loaded_config(&notification);
                }
            });
        }

        // Clean close parity: when Ghostty requests closure (e.g. the user
        // closes the surface), drive the same teardown as a classic pane so no
        // stale pane is left behind. Defer to idle so we never destroy the
        // Ghostty widget from inside its own close-request emission.
        if let Some(state) = self.state.clone() {
            let surface_id = request.surface_id.clone();
            widget.connect_local("close-request", false, move |_| {
                let state = state.clone();
                let surface_id = surface_id.clone();
                glib::idle_add_local_once(move || {
                    close_surface_by_id(&state, &surface_id);
                });
                None
            });
        }

        // PID parity: record the child PID for listening-port discovery and the
        // socket `surfaces` PID field, matching the classic spawn callback. The
        // PID lands on the surface shortly after init, so poll briefly until the
        // embedded ABI returns it. Skipped when the loaded library predates the
        // child-pid getter so older libs don't spin a pointless timer.
        if embedder.supports_child_pid() {
            let embedder = Rc::clone(&embedder);
            let surface_pids = self.surface_pids.clone();
            let embedded_spawn_tokens = self.embedded_spawn_tokens.clone();
            let state = self.state.clone();
            let surface_id = request.surface_id.clone();
            let weak_widget = widget.downgrade();
            let mut attempts: u32 = 0;
            glib::timeout_add_local(EMBEDDED_GHOSTTY_PID_POLL_INTERVAL, move || {
                let Some(widget) = weak_widget.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                attempts += 1;
                if widget.property::<bool>("child-exited")
                    || !matches!(
                        embedded_spawn_tokens.borrow().get(&surface_id),
                        Some(current) if *current == spawn_token
                    )
                {
                    return glib::ControlFlow::Break;
                }
                let mut child_pid = unsafe { embedder.surface_child_pid(&widget) };
                #[cfg(target_os = "linux")]
                if child_pid.is_none() {
                    child_pid = new_process_child_pid_since(&child_pids_before_spawn);
                }
                if let Some(pid) = child_pid {
                    surface_pids
                        .borrow_mut()
                        .insert(surface_id.clone(), SurfacePid { pid, spawn_token });
                    if let Some(state) = &state {
                        if let Ok(pid) = u32::try_from(pid) {
                            if let Err(err) = state.terminal.mark_surface_pid(&surface_id, pid) {
                                eprintln!(
                                    "Failed to record embedded Ghostty surface pid {surface_id}: {err}"
                                );
                            }
                        }
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
            let embedder = Rc::clone(&embedder);
            let model = self.model.clone();
            let surface_id = request.surface_id.clone();
            let weak_widget = widget.downgrade();
            let mut skip_initial_snapshot = model
                .lock()
                .ok()
                .and_then(|model| {
                    model
                        .surface(&surface_id)
                        .and_then(|surface| surface.persisted_scrollback.clone())
                })
                .as_deref()
                .is_some_and(|persisted_scrollback| {
                    should_skip_initial_embedded_scrollback_snapshot(
                        embedder.supports_restore_scrollback(),
                        Some(persisted_scrollback),
                    )
                });
            let mut last_snapshot: Option<String> = None;
            glib::timeout_add_local(EMBEDDED_GHOSTTY_SCROLLBACK_SNAPSHOT_INTERVAL, move || {
                let Some(widget) = weak_widget.upgrade() else {
                    return glib::ControlFlow::Break;
                };
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
                        if let Ok(mut model) = model.lock() {
                            model.set_surface_persisted_scrollback(&surface_id, Some(text.clone()));
                        }
                        last_snapshot = Some(text);
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        if let Ok(mut model) = self.model.lock() {
            let _ = model.clear_status(
                &request.workspace_id,
                Some(&surface_status_key(&request.surface_id)),
            );
        }
        let chrome = build_embedded_ghostty_pane_chrome(
            &request.surface_id,
            &view,
            self.state.as_ref(),
            &self.parent_window,
        );
        self.chromes.insert(request.surface_id.clone(), chrome);
        self.embedded_ghostty_panes.insert(
            request.surface_id.clone(),
            EmbeddedGhosttyPane { surface: widget },
        );
        self.rebuild_layout();
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
