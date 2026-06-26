//! Embedded Ghostty runtime helpers shared by the controller and terminal signals.
//!
//! This module owns the runtime bookkeeping that is not layout orchestration:
//! child PID tracking, scrollback snapshots, backend size synchronization, and
//! model updates for embedded child exits.

use super::*;

#[derive(Clone)]
pub(super) struct EmbeddedGhosttyPane {
    pub(super) surface: gtk::Widget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SurfacePid {
    pub(super) pid: i32,
    pub(super) spawn_token: u64,
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

pub(super) fn remove_surface_pid_for_spawn(
    pids: &mut BTreeMap<String, SurfacePid>,
    surface_id: &str,
    spawn_token: u64,
) -> bool {
    if !matches!(
        pids.get(surface_id),
        Some(entry) if entry.spawn_token == spawn_token
    ) {
        return false;
    }
    pids.remove(surface_id);
    true
}

pub(super) fn embedded_agent_tail_generation(known: Option<&AgentTailEntry>) -> u64 {
    known
        .map(|(generation, _)| generation.saturating_add(1))
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
pub(super) fn proc_stat_parent_pid(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(") ")?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(target_os = "linux")]
pub(super) fn current_process_child_pids() -> BTreeSet<i32> {
    let parent_pid = std::process::id();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<i32>().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            (proc_stat_parent_pid(&stat) == Some(parent_pid)).then_some(pid)
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(super) fn new_process_child_pid_since(before: &BTreeSet<i32>) -> Option<i32> {
    let mut candidates = current_process_child_pids()
        .difference(before)
        .copied()
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    match candidates.as_slice() {
        [pid] => Some(*pid),
        _ => None,
    }
}

/// Reflect an embedded Ghostty child-process exit into the model: set the pane
/// status and, on an abnormal exit, build a notification to dispatch. Mirrors
/// the classic-pane `ChildExit` handling in `terminal_signals.rs`. `exit_code`
/// is `None` when the embedded ABI does not expose the code, in which case the
/// status is neutral and no notification is raised. Returns the notification to
/// dispatch (outside the model lock), if any.
pub(super) fn apply_embedded_child_exit(
    model: &mut WorkspaceModel,
    workspace_id: &str,
    surface_id: &str,
    exit_code: Option<i32>,
) -> Option<NotificationItem> {
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
        Some(code) if code != 0 => Some(model.create_notification(
            "Terminal exited",
            format!("Process exited with status {code}. Use Restart Pane to spawn it again."),
            NotificationKind::Info,
            Some(workspace_id.to_string()),
            Some(surface_id.to_string()),
        )),
        _ => None,
    }
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

pub(super) fn sync_embedded_ghostty_surface_size(
    state: &SocketAppState,
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    surface_id: &str,
) {
    if !embedder.supports_bounded_read_text() {
        return;
    }
    let snapshot =
        unsafe { embedder.read_text_snapshot(widget, surface_id, TerminalTextCapture::Visible, 0) };
    match snapshot {
        Ok(snapshot) => {
            if let Err(err) = sync_terminal_surface_size_from_snapshot(
                state.terminal.as_ref(),
                surface_id,
                &snapshot,
            ) {
                eprintln!("Failed to sync embedded Ghostty size {surface_id}: {err}");
            }
        }
        Err(TerminalError::NotReady(_)) => {}
        Err(err) => eprintln!("Failed to read embedded Ghostty size {surface_id}: {err}"),
    }
}
