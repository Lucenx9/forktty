//! Embedded Ghostty runtime helpers shared by the controller and terminal signals.
//!
//! This module owns the runtime bookkeeping that is not layout orchestration:
//! child PID tracking, scrollback snapshots, backend size synchronization, and
//! model updates for embedded child exits.

use super::*;

#[derive(Clone)]
pub(super) struct EmbeddedGhosttyPane {
    pub(super) surface: gtk::Widget,
    /// Backend incarnation represented by this concrete GTK widget.
    pub(super) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SurfacePid {
    pub(super) pid: i32,
    /// Backend incarnation that produced this PID observation.
    pub(super) generation: u64,
}

/// Run an embedded-widget callback only while its backend incarnation remains
/// current. Callback failures are intentionally quiet: a replaced or explicitly
/// closed surface is normal while GTK still has old signal emissions queued.
pub(super) fn run_embedded_callback_for_generation<T>(
    backend: &GtkTerminalBackend,
    surface_id: &str,
    generation: u64,
    callback: impl FnOnce() -> T,
) -> Option<T> {
    backend
        .with_surface_generation(surface_id, generation, callback)
        .ok()
}

pub(super) fn apply_embedded_title_for_generation(
    backend: &GtkTerminalBackend,
    model: &Arc<Mutex<WorkspaceModel>>,
    surface_id: &str,
    generation: u64,
    title: impl Into<String>,
) -> bool {
    let title = title.into();
    run_embedded_callback_for_generation(backend, surface_id, generation, || {
        model
            .lock()
            .is_ok_and(|mut model| model.set_surface_title(surface_id, title))
    })
    .unwrap_or(false)
}

pub(super) fn apply_embedded_focus_for_generation(
    backend: &GtkTerminalBackend,
    model: &Arc<Mutex<WorkspaceModel>>,
    surface_id: &str,
    generation: u64,
) -> bool {
    run_embedded_callback_for_generation(backend, surface_id, generation, || {
        let Ok(mut model) = model.lock() else {
            return false;
        };
        if model.surface(surface_id).is_none() {
            return false;
        }
        let _ = model.focus_surface(surface_id);
        let _ = model.mark_surface_unread(surface_id, false);
        true
    })
    .unwrap_or(false)
}

pub(super) fn commit_embedded_scrollback_for_generation(
    backend: &GtkTerminalBackend,
    model: &Arc<Mutex<WorkspaceModel>>,
    surface_id: &str,
    generation: u64,
    text: String,
) -> bool {
    run_embedded_callback_for_generation(backend, surface_id, generation, || {
        model
            .lock()
            .is_ok_and(|mut model| model.set_surface_persisted_scrollback(surface_id, Some(text)))
    })
    .unwrap_or(false)
}

/// Read a set of embedded panes while their backend incarnations remain
/// current, then apply every successful tail under one model lock. A stale
/// target is omitted without preventing still-current panes from being saved.
pub(super) fn snapshot_current_generation_scrollback_with<T>(
    backend: &GtkTerminalBackend,
    model: &Arc<Mutex<WorkspaceModel>>,
    mut targets: Vec<(String, u64, T)>,
    mut read_tail: impl FnMut(&str, &T) -> Option<String>,
) {
    while !targets.is_empty() {
        let generations = targets
            .iter()
            .map(|(surface_id, generation, _)| (surface_id.clone(), *generation))
            .collect::<Vec<_>>();
        let result = backend.with_surface_generations(generations, || {
            let snapshots = targets
                .iter()
                .filter_map(|(surface_id, _, target)| {
                    read_tail(surface_id, target).map(|text| (surface_id.clone(), text))
                })
                .collect::<Vec<_>>();
            if snapshots.is_empty() {
                return;
            }
            let Ok(mut model) = model.lock() else {
                eprintln!("Failed to store final embedded Ghostty scrollback: model lock poisoned");
                return;
            };
            for (surface_id, text) in snapshots {
                let _ = model.set_surface_persisted_scrollback(&surface_id, Some(text));
            }
        });
        match result {
            Ok(()) => return,
            Err(TerminalError::NotReady(stale_surface_id))
            | Err(TerminalError::NotFound(stale_surface_id)) => {
                targets.retain(|(surface_id, _, _)| surface_id != &stale_surface_id);
            }
            Err(err) => {
                eprintln!("Failed to snapshot final embedded Ghostty scrollback: {err}");
                return;
            }
        }
    }
}

pub(super) fn run_embedded_initial_layout_tick(
    backend: &GtkTerminalBackend,
    surface_id: &str,
    generation: u64,
    container: &gtk::Widget,
    widget: &gtk::Widget,
    layout_attempts: &Cell<u8>,
) -> glib::ControlFlow {
    run_embedded_callback_for_generation(backend, surface_id, generation, || {
        if widget.width() > 0 && widget.height() > 0 {
            return glib::ControlFlow::Break;
        }
        let attempt = layout_attempts.get().saturating_add(1);
        layout_attempts.set(attempt);
        widget.queue_resize();
        container.queue_resize();
        if attempt >= EMBEDDED_GHOSTTY_INITIAL_LAYOUT_MAX_FRAMES {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    })
    .unwrap_or(glib::ControlFlow::Break)
}

/// Apply a queued GTK command only to the pane incarnation it names.
pub(super) fn run_embedded_pane_command_for_generation<T>(
    pane_generation: u64,
    command_generation: u64,
    command: impl FnOnce() -> T,
) -> Option<T> {
    (pane_generation == command_generation).then(command)
}

/// How often to poll an embedded Ghostty surface for its child PID, and the cap
/// on attempts. The PID usually lands quickly, but the Ghostty mailbox hand-off
/// can lag behind widget readiness under CI/Xvfb, so keep polling long enough
/// for the socket `surfaces` PID field to converge without leaving an unbounded
/// timer behind.
pub(super) const EMBEDDED_GHOSTTY_PID_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const EMBEDDED_GHOSTTY_PID_POLL_MAX_ATTEMPTS: u32 = 300;
/// How often embedded panes snapshot their scrollback tail into the model so a
/// later session save captures recent output. Embedded panes have no PTY pump
/// loop (unlike classic panes), so this throttled poll plays that role; the
/// classic pump snapshots at most every second when content changes.
pub(super) const EMBEDDED_GHOSTTY_SCROLLBACK_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);
/// How often ForkTTY checks whether the embedded Ghostty library reported
/// pending app-mailbox work through the wakeup callback. This timer reads one
/// atomic flag; it does not tick Ghostty unless work is pending.
pub(super) const EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL: Duration = Duration::from_millis(16);
/// Minimum interval between real `ghostty_gtk_context_tick()` calls when the
/// wakeup callback fires continuously. This only stops ticking faster than the
/// wakeup-check cadence; GTK's frame clock paces the actual GL redraw. It was
/// 100ms as a throttle against the old cairo software-renderer leak; with the
/// GL renderer (`GSK_RENDERER=ngl`) redraws are cheap and leak-free, so a 100ms
/// floor would only cap agent/TUI output to ~10fps for no benefit.
pub(super) const EMBEDDED_GHOSTTY_CONTEXT_TICK_MIN_INTERVAL: Duration = Duration::from_millis(16);
/// Fallback for older embedding libraries that do not expose the wakeup
/// callback ABI. Keep it slow: polling `ghostty_gtk_context_tick()` while idle
/// leaks memory in the GTK host.
pub(super) const EMBEDDED_GHOSTTY_CONTEXT_TICK_FALLBACK_INTERVAL: Duration = Duration::from_secs(1);
/// Embedded Ghostty's default retained history budget, matching upstream's
/// `scrollback-limit` default. Keep this bounded so long-running agent panes
/// cannot accumulate unlimited history in the GTK host process.
pub(super) const EMBEDDED_GHOSTTY_SCROLLBACK_LIMIT_BYTES: usize = 10_000_000;

pub(super) fn embedded_ghostty_scrollback_limit_bytes_for_appearance(
    appearance: &GhosttyTerminalAppearance,
) -> usize {
    appearance
        .scrollback_limit_bytes
        .unwrap_or(EMBEDDED_GHOSTTY_SCROLLBACK_LIMIT_BYTES)
}

pub(super) fn build_embedded_ghostty_scroll_view(
    surface: &gtk::Widget,
    scrollbar: GhosttyScrollbarPolicy,
) -> gtk::Widget {
    surface.add_css_class("forktty-terminal-focus-boundary");
    surface.set_hexpand(true);
    surface.set_vexpand(true);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(match scrollbar {
            GhosttyScrollbarPolicy::System => gtk::PolicyType::Automatic,
            GhosttyScrollbarPolicy::Never => gtk::PolicyType::Never,
        })
        .overlay_scrolling(true)
        .kinetic_scrolling(false)
        .child(surface)
        .build();
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    scroller.upcast()
}

pub(super) fn model_focus_still_targets_surface(
    model: &Arc<Mutex<WorkspaceModel>>,
    surface_id: &str,
) -> bool {
    model
        .lock()
        .ok()
        .and_then(|model| model.active_workspace())
        .is_some_and(|workspace| workspace.focused_surface_id == surface_id)
}

pub(super) fn remove_surface_pid_for_generation(
    pids: &mut BTreeMap<String, SurfacePid>,
    surface_id: &str,
    generation: u64,
) -> bool {
    if !matches!(
        pids.get(surface_id),
        Some(entry) if entry.generation == generation
    ) {
        return false;
    }
    pids.remove(surface_id);
    true
}

pub(super) fn commit_embedded_surface_pid_for_generation(
    backend: &GtkTerminalBackend,
    surface_id: &str,
    generation: u64,
    pid: i32,
    commit_observation: impl FnOnce(SurfacePid),
) -> Result<bool, TerminalError> {
    let Ok(backend_pid) = u32::try_from(pid) else {
        return Ok(false);
    };
    backend
        .mark_surface_pid_for_generation_and(surface_id, generation, backend_pid, || {
            commit_observation(SurfacePid { pid, generation });
        })
        .map(|()| true)
}

pub(super) fn current_embedded_surface_pid(
    backend: &GtkTerminalBackend,
    surface_id: &str,
    entry: SurfacePid,
) -> Option<i32> {
    run_embedded_callback_for_generation(backend, surface_id, entry.generation, || entry.pid)
}

pub(super) fn embedded_agent_tail_generation(known: Option<&AgentTailEntry>) -> u64 {
    known
        .map(|(generation, _)| generation.saturating_add(1))
        .unwrap_or(0)
}

/// Reflect an embedded Ghostty child-process exit into the model: set the pane
/// status and, on an abnormal exit, build a notification to dispatch. Mirrors
/// the classic-pane `ChildExit` handling in `terminal_signals.rs`. `exit_code`
/// is `None` when the embedded ABI does not expose the code, in which case the
/// status is neutral and no notification is raised. Returns the notification
/// creation to dispatch outside the model lock, including IDs whose retained
/// desktop handles must be closed.
pub(super) fn apply_embedded_child_exit(
    model: &mut WorkspaceModel,
    workspace_id: &str,
    surface_id: &str,
    exit_code: Option<i32>,
) -> Option<NotificationCreation> {
    model.surface(surface_id)?;
    let _ = model.set_surface_agent_session_lifecycle(
        surface_id,
        forktty_core::AgentSessionLifecycle::Ended,
    );
    let status = embedded_child_exit_status(exit_code);
    let _ = model.set_status(
        workspace_id,
        surface_status_key(surface_id),
        status.label,
        status.value,
        status.color,
    );
    match exit_code {
        Some(code) if code != 0 => Some(model.create_notification_with_evictions(
            "Terminal exited",
            format!("Process exited with status {code}. Use Restart Pane to spawn it again."),
            NotificationKind::Info,
            Some(workspace_id.to_string()),
            Some(surface_id.to_string()),
        )),
        _ => None,
    }
}

/// Atomically commit a child exit only for the current backend generation.
///
/// The returned notification creation is intentionally delivered after the
/// generation lease and model lock are released so the controller can close
/// evicted desktop handles before dispatching the new notification.
pub(super) fn commit_embedded_child_exit_for_generation(
    backend: &GtkTerminalBackend,
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    generation: u64,
    exit_code: Option<i32>,
    remove_pid_observation: impl FnOnce(),
) -> Result<Option<NotificationCreation>, TerminalError> {
    backend.mark_surface_exited_for_generation_and(surface_id, generation, || {
        remove_pid_observation();
        let mut model = model.lock().map_err(|_| TerminalError::LockPoisoned)?;
        Ok(apply_embedded_child_exit(
            &mut model,
            workspace_id,
            surface_id,
            exit_code,
        ))
    })?
}

/// Read the last `lines` of an embedded surface's scrollback as plain text,
/// bounded to `MAX_PERSISTED_SCROLLBACK_BYTES`. Returns `None` if the read
/// fails. Does not touch the model lock so callers can read first and store
/// under a brief lock, never holding the lock across the Ghostty ABI call.
pub(super) fn read_embedded_scrollback_tail(
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    surface_id: &str,
    lines: u32,
) -> Option<String> {
    match unsafe {
        embedder.read_text_snapshot(
            widget,
            surface_id,
            TerminalTextCapture::Tail {
                lines: lines as usize,
            },
            forktty_core::MAX_PERSISTED_SCROLLBACK_BYTES,
        )
    } {
        Ok(snapshot) => Some(snapshot.text),
        Err(err) => {
            eprintln!("Failed to read embedded Ghostty scrollback {surface_id}: {err}");
            None
        }
    }
}

pub(super) fn snapshot_embedded_scrollback_tail_to_model(
    model: &Arc<Mutex<WorkspaceModel>>,
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    surface_id: &str,
    lines: u32,
) {
    if lines == 0 || !embedder.supports_read_text() {
        return;
    }
    let Some(text) = read_embedded_scrollback_tail(embedder, widget, surface_id, lines) else {
        return;
    };
    if let Ok(mut model) = model.lock() {
        let _ = model.set_surface_persisted_scrollback(surface_id, Some(text));
    }
}

pub(super) fn snapshot_embedded_scrollback_tail_to_model_for_generation(
    backend: &GtkTerminalBackend,
    model: &Arc<Mutex<WorkspaceModel>>,
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    surface_id: &str,
    generation: u64,
    lines: u32,
) {
    if lines == 0 || !embedder.supports_read_text() {
        return;
    }
    if run_embedded_callback_for_generation(backend, surface_id, generation, || ()).is_none() {
        return;
    }
    let Some(text) = read_embedded_scrollback_tail(embedder, widget, surface_id, lines) else {
        return;
    };
    let _ = commit_embedded_scrollback_for_generation(backend, model, surface_id, generation, text);
}

#[cfg(test)]
pub(super) fn sync_terminal_surface_size_from_snapshot(
    terminal: &dyn TerminalBackend,
    surface_id: &str,
    snapshot: &TerminalTextSnapshot,
) -> Result<bool, TerminalError> {
    if snapshot.cols == 0 || snapshot.rows == 0 {
        return Ok(false);
    }
    let current = terminal
        .surfaces()?
        .into_iter()
        .find(|surface| surface.surface_id == surface_id)
        .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
    if current.cols == snapshot.cols && current.rows == snapshot.rows {
        return Ok(false);
    }
    terminal.resize(surface_id, snapshot.cols, snapshot.rows)?;
    Ok(true)
}

pub(super) fn sync_terminal_surface_size_from_snapshot_for_generation(
    backend: &GtkTerminalBackend,
    surface_id: &str,
    generation: u64,
    snapshot: &TerminalTextSnapshot,
) -> Result<bool, TerminalError> {
    if snapshot.cols == 0 || snapshot.rows == 0 {
        return Ok(false);
    }
    let current = backend.surface_size_for_generation(surface_id, generation)?;
    if current == (snapshot.cols, snapshot.rows) {
        return Ok(false);
    }
    backend.resize_for_generation(surface_id, generation, snapshot.cols, snapshot.rows)?;
    Ok(true)
}

pub(super) fn sync_embedded_ghostty_surface_size(
    backend: &GtkTerminalBackend,
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    surface_id: &str,
    generation: u64,
) {
    if run_embedded_callback_for_generation(backend, surface_id, generation, || ()).is_none() {
        return;
    }
    if !embedder.supports_bounded_read_text() {
        return;
    }
    let snapshot =
        unsafe { embedder.read_text_snapshot(widget, surface_id, TerminalTextCapture::Visible, 0) };
    match snapshot {
        Ok(snapshot) => {
            if let Err(err) = sync_terminal_surface_size_from_snapshot_for_generation(
                backend, surface_id, generation, &snapshot,
            ) {
                eprintln!("Failed to sync embedded Ghostty size {surface_id}: {err}");
            }
        }
        Err(TerminalError::NotReady(_)) => {}
        Err(err) => eprintln!("Failed to read embedded Ghostty size {surface_id}: {err}"),
    }
}
