use super::*;

pub(super) struct PaneChrome {
    pub(super) pane: gtk::Box,
    pub(super) header_revealer: gtk::Revealer,
    pub(super) single_pane_actions: gtk::Box,
    pub(super) focus_marker: gtk::Box,
    pub(super) title: gtk::Label,
    pub(super) cwd: gtk::Label,
    pub(super) attention_dot: gtk::Box,
    pub(super) search_bar: PaneSearchBar,
    pub(super) search_supported: bool,
}

pub(super) type SplitResizeCallback = Rc<dyn Fn(&[String], &[String], f64)>;
pub(super) type SettingsApplyCallback = Rc<dyn Fn(&config::AppConfig)>;

pub(super) struct TerminalController {
    container: gtk::Box,
    parent_window: adw::ApplicationWindow,
    pub(super) model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
    toast_handle: Option<ToastHandle>,
    pub(super) widgets: BTreeMap<String, GhosttyTerminalWidget>,
    embedded_ghostty_panes: BTreeMap<String, EmbeddedGhosttyPane>,
    embedded_ghostty: Option<Rc<GhosttyGtkEmbedder>>,
    chromes: BTreeMap<String, PaneChrome>,
    pending_spawns: BTreeMap<String, PendingSpawn>,
    last_layout_signature: Option<String>,
    last_chrome_signature: Option<String>,
    maximized_pane: bool,
    /// Spawned child PID per surface, used to discover listening ports.
    pub(super) surface_pids: Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    /// Current embedded Ghostty spawn token per surface. Timers and exit
    /// handlers use this to avoid applying stale state after a newer spawn or
    /// close for the same surface id.
    embedded_spawn_tokens: Rc<RefCell<BTreeMap<String, u64>>>,
    next_spawn_token: u64,
    pane_tab_strips: Rc<RefCell<Vec<PaneTabStrip>>>,
    terminal_zoom_level: Cell<i32>,
    #[cfg(feature = "browser")]
    browser_panes: Rc<RefCell<BTreeMap<String, Rc<crate::browser_pane::BrowserPaneWidget>>>>,
}

#[derive(Clone)]
struct EmbeddedGhosttyPane {
    surface: gtk::Widget,
}

#[derive(Clone)]
pub(super) struct PaneTabStrip {
    tabs: Vec<String>,
    stack: gtk::Stack,
    tab_widgets: Vec<gtk::Box>,
    labels: Vec<gtk::Label>,
    select_areas: Vec<gtk::Box>,
}

struct TabStripRefresh {
    active_id: Option<String>,
    tabs: Vec<TabRefresh>,
}

struct TabRefresh {
    title: String,
    active: bool,
}

fn surface_has_agent_session(surface: &forktty_core::Surface) -> bool {
    surface.agent_session.is_some()
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
const EMBEDDED_GHOSTTY_PID_POLL_INTERVAL: Duration = Duration::from_millis(100);
const EMBEDDED_GHOSTTY_PID_POLL_MAX_ATTEMPTS: u32 = 300;
/// How often embedded panes snapshot their scrollback tail into the model so a
/// later session save captures recent output. Embedded panes have no PTY pump
/// loop (unlike classic panes), so this throttled poll plays that role; the
/// classic pump snapshots at most every second when content changes.
const EMBEDDED_GHOSTTY_SCROLLBACK_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);
/// How often ForkTTY checks whether the embedded Ghostty library reported
/// pending app-mailbox work through the wakeup callback. This timer reads one
/// atomic flag; it does not tick Ghostty unless work is pending.
pub(super) const EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL: Duration = Duration::from_millis(16);
/// Minimum interval between real `ghostty_gtk_context_tick()` calls when the
/// wakeup callback fires continuously. This only stops ticking faster than the
/// wakeup-check cadence; GTK's frame clock paces the actual GL redraw. It was
/// 100ms as a throttle against the old cairo software-renderer leak — with the
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
fn current_process_child_pids() -> BTreeSet<i32> {
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
fn new_process_child_pid_since(before: &BTreeSet<i32>) -> Option<i32> {
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
fn read_embedded_scrollback_tail(
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

fn snapshot_embedded_scrollback_tail_to_model(
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

impl TerminalController {
    pub(super) fn new(
        container: gtk::Box,
        parent_window: adw::ApplicationWindow,
        model: Arc<Mutex<WorkspaceModel>>,
    ) -> Self {
        Self {
            container,
            parent_window,
            model,
            state: None,
            toast_handle: None,
            widgets: BTreeMap::new(),
            embedded_ghostty_panes: BTreeMap::new(),
            embedded_ghostty: None,
            chromes: BTreeMap::new(),
            pending_spawns: BTreeMap::new(),
            last_layout_signature: None,
            last_chrome_signature: None,
            maximized_pane: false,
            surface_pids: Rc::new(RefCell::new(BTreeMap::new())),
            embedded_spawn_tokens: Rc::new(RefCell::new(BTreeMap::new())),
            next_spawn_token: 0,
            pane_tab_strips: Rc::new(RefCell::new(Vec::new())),
            terminal_zoom_level: Cell::new(0),
            #[cfg(feature = "browser")]
            browser_panes: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }

    pub(super) fn attach_state(&mut self, state: SocketAppState) {
        self.state = Some(state);
    }

    pub(super) fn attach_toast_handle(&mut self, toast_handle: ToastHandle) {
        self.toast_handle = Some(toast_handle);
    }

    pub(super) fn handle(&mut self, command: GtkTerminalCommand) {
        match command {
            GtkTerminalCommand::Spawn(request) => self.spawn(request),
            GtkTerminalCommand::SendText { surface_id, text } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    widget.send_text(&text);
                } else if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
                    if let Some(embedder) = &self.embedded_ghostty {
                        if let Err(err) = unsafe { embedder.send_text(&pane.surface, &text) } {
                            eprintln!(
                                "Failed to send text to embedded Ghostty GTK surface {surface_id}: {err}"
                            );
                        }
                    } else {
                        eprintln!(
                            "Dropped send-text for embedded Ghostty GTK surface without embedder: {surface_id}"
                        );
                    }
                } else {
                    eprintln!("Dropped send-text for unready terminal surface: {surface_id}");
                }
            }
            GtkTerminalCommand::ReadText {
                surface_id,
                capture,
                max_bytes,
                reply,
            } => {
                let result = if let Some(widget) = self.widgets.get(&surface_id) {
                    widget.read_text(&surface_id, capture, max_bytes)
                } else if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
                    match &self.embedded_ghostty {
                        Some(embedder) if embedder.supports_read_text() => unsafe {
                            embedder.read_text_snapshot(
                                &pane.surface,
                                &surface_id,
                                capture,
                                max_bytes,
                            )
                        },
                        Some(_) => Err(TerminalError::Backend(
                            "embedded Ghostty GTK library does not export read-text support"
                                .to_string(),
                        )),
                        None => Err(TerminalError::Backend(
                            "embedded Ghostty GTK surface has no embedder".to_string(),
                        )),
                    }
                } else {
                    Err(TerminalError::NotReady(surface_id.clone()))
                };
                let _ = reply.send(result);
            }
            GtkTerminalCommand::Resize {
                surface_id,
                cols,
                rows,
            } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    widget.resize_cells(cols, rows);
                }
            }
            GtkTerminalCommand::Close { surface_id } => {
                self.pending_spawns.remove(&surface_id);
                if let (Some(embedder), Some(pane)) = (
                    self.embedded_ghostty.as_ref(),
                    self.embedded_ghostty_panes.get(&surface_id),
                ) {
                    let persistent_scrollback_lines = config::load_config()
                        .map(|config| config.appearance.persistent_scrollback_lines)
                        .unwrap_or(0);
                    snapshot_embedded_scrollback_tail_to_model(
                        &self.model,
                        embedder,
                        &pane.surface,
                        &surface_id,
                        persistent_scrollback_lines,
                    );
                }
                if let Some(chrome) = self.chromes.remove(&surface_id) {
                    detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
                }
                self.embedded_ghostty_panes.remove(&surface_id);
                self.widgets.remove(&surface_id);
                self.surface_pids.borrow_mut().remove(&surface_id);
                self.embedded_spawn_tokens.borrow_mut().remove(&surface_id);
                #[cfg(feature = "browser")]
                if let Some(pane) = self.browser_panes.borrow_mut().remove(&surface_id) {
                    pane.prepare_close();
                }
                self.rebuild_layout();
            }
        }
    }

    fn spawn(&mut self, request: SpawnRequest) {
        if self.widgets.contains_key(&request.surface_id)
            || self
                .embedded_ghostty_panes
                .contains_key(&request.surface_id)
        {
            return;
        }
        mark_spawn_command_pending(&mut self.pending_spawns, &request.surface_id);
        match self.spawn_embedded_ghostty(request.clone()) {
            Ok(()) => {}
            Err(err) => {
                self.pending_spawns.remove(&request.surface_id);
                record_terminal_spawn_failure(
                    &self.model,
                    &request.workspace_id,
                    &request.surface_id,
                    &format!("Failed to spawn embedded Ghostty GTK pane: {err}"),
                    self.state
                        .as_ref()
                        .is_none_or(|state| state.notification_dispatch),
                );
                if let Some(state) = &self.state {
                    let _ = state.terminal.close(&request.surface_id);
                }
                self.last_layout_signature = None;
                self.rebuild_layout();
            }
        }
    }

    fn spawn_embedded_ghostty(&mut self, request: SpawnRequest) -> Result<(), String> {
        let embedder = self.embedded_ghostty()?;
        let config = config::load_config().unwrap_or_default();
        let persistent_scrollback_lines = config.appearance.persistent_scrollback_lines;
        let terminal_appearance = ghostty_terminal_appearance_for_config(&config);
        let scrollback_limit_bytes =
            embedded_ghostty_scrollback_limit_bytes_for_appearance(&terminal_appearance);
        #[cfg(target_os = "linux")]
        let child_pids_before_spawn = current_process_child_pids();
        let widget = if embedder.supports_spawn_command() {
            let argv = forktty_terminal::spawn::embedded_ghostty_command_argv(&request)?;
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
                widget.connect_local("init", false, move |_| {
                    if let Err(err) = state.terminal.mark_surface_ready(&surface_id) {
                        eprintln!(
                            "Failed to mark embedded Ghostty GTK surface ready {surface_id}: {err}"
                        );
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
                // brief lock — never hold the model lock across the ABI read.
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

    fn embedded_ghostty(&mut self) -> Result<Rc<GhosttyGtkEmbedder>, String> {
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

    pub(super) fn rebuild_layout(&mut self) {
        self.spawn_active_surfaces_if_needed();
        self.last_chrome_signature = None;
        // Drop browser panes whose surfaces were removed from the model so the
        // webviews don't linger after a model-driven (e.g. socket) close.
        #[cfg(feature = "browser")]
        {
            let live_surface_ids = self
                .model
                .lock()
                .ok()
                .map(|model| {
                    model
                        .list_surfaces(None)
                        .into_iter()
                        .map(|surface| surface.id)
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let stale_browser_ids = self
                .browser_panes
                .borrow()
                .keys()
                .filter(|surface_id| !live_surface_ids.contains(*surface_id))
                .cloned()
                .collect::<Vec<_>>();
            for surface_id in stale_browser_ids {
                if let Some(pane) = self.browser_panes.borrow_mut().remove(&surface_id) {
                    pane.prepare_close();
                }
            }
            // Cached browser widgets are reused across rebuilds; detach them from
            // their previous parent first, or set_start/end_child asserts
            // (gtk_widget_get_parent(child) == NULL) when re-inserting.
            for pane in self.browser_panes.borrow().values() {
                detach_widget(&pane.widget());
            }
        }
        for chrome in self.chromes.values() {
            detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
        }
        self.pane_tab_strips.borrow_mut().clear();
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }

        let Some((signature, pane_tree, focused_surface_id, workspace_id)) =
            active_layout_snapshot(&self.model)
        else {
            self.set_terminal_sibling_flags(&BTreeSet::new(), false);
            self.last_layout_signature = Some(EMPTY_LAYOUT_SIGNATURE.to_string());
            self.container.append(&empty_terminal_stage(
                self.state.as_ref(),
                Some(&self.parent_window),
            ));
            return;
        };
        let visible_tree = if self.maximized_pane {
            PaneNode::single_leaf(focused_surface_id.clone())
        } else {
            pane_tree
        };
        let visible_panes = collect_panes(&visible_tree);
        let visible_surface_ids = collect_leaves(&visible_tree).into_iter().collect();
        let single_pane = visible_panes.len() == 1;
        self.set_terminal_sibling_flags(&visible_surface_ids, !single_pane);
        let widget = self.widget_for_pane(&visible_tree, &workspace_id);
        for chrome in self.chromes.values() {
            chrome.header_revealer.set_reveal_child(!single_pane);
            chrome.single_pane_actions.set_visible(single_pane);
            chrome.single_pane_actions.set_sensitive(single_pane);
        }
        self.container.append(&widget);
        self.queue_focus_for_surface(&focused_surface_id);
        self.last_layout_signature = Some(effective_layout_signature(
            &signature,
            self.maximized_pane,
            &focused_surface_id,
        ));
    }

    pub(super) fn toggle_maximized_pane(&mut self) {
        self.maximized_pane = !self.maximized_pane;
        self.last_layout_signature = None;
        self.rebuild_layout();
    }

    pub(super) fn zoom_terminal_in(&mut self) {
        self.set_terminal_zoom_level(next_terminal_zoom_level(self.terminal_zoom_level.get(), 1));
    }

    pub(super) fn zoom_terminal_out(&mut self) {
        self.set_terminal_zoom_level(next_terminal_zoom_level(self.terminal_zoom_level.get(), -1));
    }

    pub(super) fn reset_terminal_zoom(&mut self) {
        self.set_terminal_zoom_level(0);
    }

    fn set_terminal_zoom_level(&mut self, zoom_level: i32) {
        let zoom_level = next_terminal_zoom_level(zoom_level, 0);
        let previous_zoom_level = self.terminal_zoom_level.replace(zoom_level);
        if previous_zoom_level == zoom_level {
            return;
        }
        for widget in self.widgets.values() {
            widget.set_zoom_level(zoom_level);
        }
        let embedded_action = if zoom_level == 0 {
            EmbeddedSurfaceAction::ResetFontSize
        } else if zoom_level > previous_zoom_level {
            EmbeddedSurfaceAction::IncreaseFontSize
        } else {
            EmbeddedSurfaceAction::DecreaseFontSize
        };
        for pane in self.embedded_ghostty_panes.values() {
            let _ = self.perform_embedded_action(&pane.surface, embedded_action);
        }
    }

    fn model_focused_widget(&self) -> Option<GhosttyTerminalWidget> {
        let surface_id = {
            let model = self.model.lock().ok()?;
            model.active_workspace()?.focused_surface_id
        };
        self.widgets.get(&surface_id).cloned()
    }

    fn gtk_focused_widget(&self) -> Option<GhosttyTerminalWidget> {
        self.widgets
            .values()
            .find(|widget| widget.has_terminal_focus())
            .cloned()
    }

    /// True when GTK focus currently lives inside `widget` (an embedded Ghostty
    /// surface focuses its internal GL area, never the wrapper widget itself).
    fn embedded_widget_has_focus(&self, widget: &gtk::Widget) -> bool {
        let Some(focus) = gtk::prelude::GtkWindowExt::focus(&self.parent_window) else {
            return false;
        };
        &focus == widget || focus.is_ancestor(widget)
    }

    /// The embedded Ghostty surface that owns GTK focus, mirroring
    /// `gtk_focused_widget` for the embedded-pane path.
    fn focused_embedded_pane(&self) -> Option<gtk::Widget> {
        self.embedded_ghostty_panes
            .values()
            .find(|pane| self.embedded_widget_has_focus(&pane.surface))
            .map(|pane| pane.surface.clone())
    }

    /// The embedded Ghostty surface the model considers focused; used by the
    /// command palette, which owns GTK focus itself while open.
    fn model_focused_embedded_pane(&self) -> Option<gtk::Widget> {
        let surface_id = {
            let model = self.model.lock().ok()?;
            model.active_workspace()?.focused_surface_id
        };
        self.embedded_ghostty_panes
            .get(&surface_id)
            .map(|pane| pane.surface.clone())
    }

    /// Routes a clipboard/search action to the embedded Ghostty surface via its
    /// keybinding ABI. Returns `true` once routed (the accelerator is consumed)
    /// even if Ghostty reports the action as a no-op (e.g. copy with no
    /// selection); `false` only when the embedding library lacks the symbol.
    fn perform_embedded_action(&self, widget: &gtk::Widget, action: EmbeddedSurfaceAction) -> bool {
        let Some(embedder) = self.embedded_ghostty.as_ref() else {
            return false;
        };
        match unsafe { embedder.perform_action(widget, action) } {
            Ok(_) => true,
            Err(err) => {
                eprintln!(
                    "forktty: embedded Ghostty {} unavailable: {err}",
                    action.as_ghostty_action()
                );
                false
            }
        }
    }

    // App-wide clipboard accelerators must only affect a terminal that currently
    // owns GTK focus; the model focus can legitimately be stale while dialogs or
    // search entries are active.
    pub(super) fn copy_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Copy);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.copy_text();
        true
    }

    pub(super) fn paste_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Paste);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.paste_from_clipboard();
        true
    }

    pub(super) fn select_all_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::SelectAll);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.select_all_text();
        true
    }

    pub(super) fn reset_focused_terminal(&self) -> bool {
        if let Some(widget) = self.focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::ClearScreen);
        }
        let Some(widget) = self.gtk_focused_widget() else {
            return false;
        };
        widget.reset_and_clear();
        true
    }

    /// The last non-empty line of `surface_id`'s terminal output plus the
    /// content generation it was read at. `known` short-circuits the bounded
    /// tail read when the generation has not moved; the agent HUD polls this
    /// once per second per agent.
    pub(super) fn surface_tail_line(
        &self,
        surface_id: &str,
        known: Option<&AgentTailEntry>,
    ) -> Option<AgentTailEntry> {
        if let Some(widget) = self.widgets.get(surface_id) {
            let generation = widget.content_generation();
            if let Some((known_generation, line)) = known {
                if *known_generation == generation {
                    return Some((generation, line.clone()));
                }
            }
            let snapshot = widget
                .read_text(surface_id, TerminalTextCapture::Tail { lines: 8 }, 4096)
                .ok()?;
            return Some((generation, last_nonempty_line(&snapshot.text)));
        }
        let pane = self.embedded_ghostty_panes.get(surface_id)?;
        let embedder = self.embedded_ghostty.as_ref()?;
        if !embedder.supports_read_text() {
            return None;
        }
        let generation = embedded_agent_tail_generation(known);
        let snapshot = unsafe {
            embedder
                .read_text_snapshot(
                    &pane.surface,
                    surface_id,
                    TerminalTextCapture::Tail { lines: 8 },
                    4096,
                )
                .ok()?
        };
        Some((generation, last_nonempty_line(&snapshot.text)))
    }

    /// Writes `text` to `surface_id`'s terminal as if typed (the caller adds
    /// any trailing `\r`). `false` when the pane has no live widget.
    pub(super) fn send_text_to_surface(&self, surface_id: &str, text: &str) -> bool {
        if let Some(widget) = self.widgets.get(surface_id) {
            widget.send_text(text);
            return true;
        }
        let Some(pane) = self.embedded_ghostty_panes.get(surface_id) else {
            return false;
        };
        let Some(embedder) = self.embedded_ghostty.as_ref() else {
            return false;
        };
        match unsafe { embedder.send_text(&pane.surface, text) } {
            Ok(()) => true,
            Err(err) => {
                eprintln!(
                    "Failed to send text to embedded Ghostty GTK surface {surface_id}: {err}"
                );
                false
            }
        }
    }

    /// Reveals the floating search bar of the focused terminal pane. The pane
    /// holding GTK focus wins; the model focus is the fallback so the command
    /// palette (which owns GTK focus itself) can open search too.
    pub(super) fn open_search_in_focused_pane(&self) -> bool {
        let surface_id = self
            .widgets
            .iter()
            .find(|(_, widget)| widget.has_terminal_focus())
            .map(|(surface_id, _)| surface_id.clone())
            .or_else(|| {
                self.embedded_ghostty_panes
                    .iter()
                    .find(|(_, pane)| self.embedded_widget_has_focus(&pane.surface))
                    .map(|(surface_id, _)| surface_id.clone())
            })
            .or_else(|| {
                let model = self.model.lock().ok()?;
                Some(model.active_workspace()?.focused_surface_id)
            });
        let Some(surface_id) = surface_id else {
            return false;
        };
        // Embedded panes have no ForkTTY search bar; open Ghostty's own overlay.
        if let Some(pane) = self.embedded_ghostty_panes.get(&surface_id) {
            return self.perform_embedded_action(&pane.surface, EmbeddedSurfaceAction::StartSearch);
        }
        let Some(chrome) = self.chromes.get(&surface_id) else {
            return false;
        };
        if !chrome.search_supported {
            return false;
        }
        chrome.search_bar.container.set_visible(true);
        chrome.search_bar.entry.grab_focus();
        true
    }

    // Explicit commands from the command palette intentionally target the active
    // terminal, because the palette itself owns GTK focus while the user chooses.
    pub(super) fn copy_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Copy);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.copy_text();
        true
    }

    pub(super) fn paste_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::Paste);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.paste_from_clipboard();
        true
    }

    pub(super) fn select_all_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::SelectAll);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.select_all_text();
        true
    }

    pub(super) fn reset_active_terminal(&self) -> bool {
        if let Some(widget) = self.model_focused_embedded_pane() {
            return self.perform_embedded_action(&widget, EmbeddedSurfaceAction::ClearScreen);
        }
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        widget.reset_and_clear();
        true
    }

    pub(super) fn send_model_focused_navigation_input(&self, input: TerminalInput) -> bool {
        let Some(widget) = self.model_focused_widget() else {
            return false;
        };
        forward_terminal_navigation_input(&widget, input);
        true
    }

    fn queue_focus_for_surface(&self, surface_id: &str) {
        if let Some(widget) = self.widgets.get(surface_id) {
            queue_widget_focus(widget.widget());
        } else if let Some(pane) = self.embedded_ghostty_panes.get(surface_id) {
            let model = self.model.clone();
            let surface_id = surface_id.to_string();
            queue_focusable_descendant_focus_when(
                pane.surface.clone(),
                Rc::new(move || model_focus_still_targets_surface(&model, &surface_id)),
            );
        } else {
            // Browser panes are not in self.widgets; hand keyboard focus to the
            // pane's focus target so keyboard-only nav reaches the browser.
            #[cfg(feature = "browser")]
            if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
                queue_widget_focus(pane.focus_target());
            }
        }
    }

    fn set_terminal_sibling_flags(
        &self,
        visible_surface_ids: &BTreeSet<String>,
        has_siblings: bool,
    ) {
        for (surface_id, widget) in &self.widgets {
            widget.set_has_siblings(has_siblings && visible_surface_ids.contains(surface_id));
        }
    }

    /// Pushes the model's focused surface into each terminal widget so the
    /// renderer never dims the logically active pane while GTK focus sits in
    /// a non-terminal widget (the pane's search entry, the command palette,
    /// dialogs). Runs from `ensure_layout_current`:
    /// focus-only changes don't alter the layout signature, so the sibling
    /// flag path alone would miss them.
    fn sync_terminal_model_focus_flags(&self) {
        let focused_surface_id = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id);
        for (surface_id, widget) in &self.widgets {
            widget.set_is_model_focused(focused_surface_id.as_deref() == Some(surface_id.as_str()));
        }
    }

    pub(super) fn sync_model_focus_to_ui(&mut self) {
        self.ensure_layout_current();
        if let Some(surface_id) = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id)
        {
            self.queue_focus_for_surface(&surface_id);
        }
    }

    pub(super) fn ensure_layout_current(&mut self) {
        self.spawn_active_surfaces_if_needed();
        let Some((signature, _, focused_surface_id, _)) = active_layout_snapshot(&self.model)
        else {
            if self.last_layout_signature.as_deref() != Some(EMPTY_LAYOUT_SIGNATURE) {
                self.rebuild_layout();
            }
            self.sync_terminal_model_focus_flags();
            return;
        };
        let signature =
            effective_layout_signature(&signature, self.maximized_pane, &focused_surface_id);
        if self.last_layout_signature.as_deref() != Some(signature.as_str()) {
            self.rebuild_layout();
        } else {
            self.refresh_chromes();
        }
        self.sync_terminal_model_focus_flags();
    }

    fn spawn_active_surfaces_if_needed(&mut self) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let backend_surface_ids = state
            .terminal
            .surfaces()
            .map(|surfaces| {
                surfaces
                    .into_iter()
                    .map(|surface| surface.surface_id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let (surfaces, model_surface_ids) = {
            let Ok(model) = self.model.lock() else {
                return;
            };
            let Some(workspace) = model.active_workspace() else {
                return;
            };
            let statuses = model.list_status(&workspace.id);
            let surfaces = model
                .list_surfaces(Some(&workspace.id))
                .into_iter()
                .map(|surface| {
                    let blocked = surface_status_blocks_auto_spawn(&statuses, &surface.id);
                    (surface, blocked)
                })
                .collect::<Vec<_>>();
            // Reconcile against every workspace's surfaces, not just the active
            // one, so a terminal backgrounded in another workspace is never
            // mistaken for an orphan.
            let model_surface_ids = model
                .list_surfaces(None)
                .into_iter()
                .map(|surface| surface.id)
                .collect::<BTreeSet<_>>();
            (surfaces, model_surface_ids)
        };
        clear_modeled_pending_spawns(
            &mut self.pending_spawns,
            &model_surface_ids,
            &backend_surface_ids,
        );
        // A surface restarted or closed on the GTK thread while a socket client
        // concurrently removed it from the model can leave a backend terminal
        // (PTY + widget) with no model counterpart. Tear those orphans down;
        // the queued Close event drops the widget on the next rebuild.
        for surface_id in orphaned_backend_surfaces(
            &backend_surface_ids,
            &model_surface_ids,
            &self.pending_spawns,
        ) {
            match state.terminal.close(&surface_id) {
                Ok(()) | Err(TerminalError::NotFound(_)) => {}
                Err(err) => {
                    eprintln!("Failed to reap orphaned terminal surface {surface_id}: {err}");
                }
            }
        }
        for (surface, auto_spawn_blocked) in surfaces {
            if auto_spawn_blocked {
                continue;
            }
            if self.widgets.contains_key(&surface.id)
                || self.embedded_ghostty_panes.contains_key(&surface.id)
                || self.pending_spawns.contains_key(&surface.id)
                || backend_surface_ids.contains(&surface.id)
            {
                continue;
            }
            // Browser surfaces are rendered by browser_panes and never get a
            // terminal backend; spawning a PTY for them would leak a hidden
            // shell process and emit bogus terminal status/port/close events.
            // Ssh surfaces rewrite the request to launch ssh <host>; this is
            // what respawns restored remote workspaces on session restore.
            // Agent terminal surfaces rewrite to the provider resume argv.
            let base =
                SpawnRequest::for_surface(&surface, state.shell.clone(), state.socket_path.clone());
            let Some(request) = forktty_socket::spawn_request_for_surface(base, &surface) else {
                continue;
            };
            let surface_id = surface.id.clone();
            let workspace_id = surface.workspace_id.clone();
            mark_spawn_command_pending(&mut self.pending_spawns, &surface_id);
            if let Err(err) = state.terminal.spawn(request) {
                self.pending_spawns.remove(&surface_id);
                record_terminal_spawn_failure(
                    &self.model,
                    &workspace_id,
                    &surface_id,
                    &err.to_string(),
                    state.notification_dispatch,
                );
            }
        }
    }

    fn refresh_chromes(&mut self) {
        let Ok(model) = self.model.lock() else {
            return;
        };
        let focused_surface_id = model
            .active_workspace()
            .map(|workspace| workspace.focused_surface_id);
        let chrome_surface_ids = self.chromes.keys().cloned().collect::<Vec<_>>();
        let tab_strip_tabs = self
            .pane_tab_strips
            .borrow()
            .iter()
            .map(|strip| strip.tabs.clone())
            .collect::<Vec<_>>();
        let signature = chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strip_tabs);
        if self.last_chrome_signature.as_deref() == Some(signature.as_str()) {
            return;
        }
        self.last_chrome_signature = Some(signature);
        let chrome_updates = self
            .chromes
            .keys()
            .filter_map(|surface_id| {
                model.surface(surface_id).map(|surface| {
                    (
                        surface_id.clone(),
                        surface.clone(),
                        focused_surface_id.as_deref() == Some(surface_id.as_str()),
                    )
                })
            })
            .collect::<Vec<_>>();
        let tab_updates = tab_strip_refreshes(&model, &tab_strip_tabs);
        // Browser navigation only mutates a surface's url (same layout structure),
        // so the layout signature is unchanged and rebuild_layout does not fire.
        // Snapshot before touching live GTK/WebKit widgets, then release the
        // model lock so focus and load callbacks can safely sync back into it.
        #[cfg(feature = "browser")]
        let browser_targets = model
            .list_surfaces(None)
            .into_iter()
            .filter_map(|surface| match surface.kind {
                forktty_core::SurfaceKind::Browser { url, .. } => {
                    let active = focused_surface_id.as_deref() == Some(surface.id.as_str());
                    Some((surface.id, (url, active)))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        drop(model);
        for (surface_id, surface, active) in chrome_updates {
            if let Some(widget) = self.widgets.get(&surface_id) {
                widget.set_local_selection_on_mouse_drag(surface_has_agent_session(&surface));
            }
            if let Some(chrome) = self.chromes.get(&surface_id) {
                update_pane_chrome(chrome, &surface, active);
            }
        }
        self.refresh_tab_strips(&tab_updates);
        // Push the latest url into the live webview on each refresh tick, and
        // drop panes for surfaces that no longer exist to avoid leaking them.
        #[cfg(feature = "browser")]
        {
            let stale = self
                .browser_panes
                .borrow()
                .keys()
                .filter(|surface_id| !browser_targets.contains_key(*surface_id))
                .cloned()
                .collect::<Vec<_>>();
            for surface_id in stale {
                if let Some(pane) = self.browser_panes.borrow_mut().remove(&surface_id) {
                    pane.prepare_close();
                }
            }
            for (surface_id, pane) in self.browser_panes.borrow().iter() {
                if let Some((url, active)) = browser_targets.get(surface_id) {
                    // Safe to call every tick: BrowserPaneWidget edge-triggers on the
                    // last *requested* url, so an unchanged url is a no-op.
                    pane.load_uri(url);
                    pane.set_active(*active);
                }
            }
        }
    }

    fn refresh_tab_strips(&self, updates: &[TabStripRefresh]) {
        for (strip, update) in self.pane_tab_strips.borrow().iter().zip(updates.iter()) {
            let Some(active_id) = update.active_id.as_ref() else {
                continue;
            };
            let previous_visible = strip
                .stack
                .visible_child_name()
                .map(|name| name.to_string());
            if previous_visible.as_deref() != Some(active_id.as_str()) {
                strip.stack.set_visible_child_name(active_id.as_str());
                self.queue_focus_for_surface(active_id);
            }
            for (idx, tab_update) in update.tabs.iter().enumerate() {
                if let Some(tab) = strip.tab_widgets.get(idx) {
                    if tab_update.active {
                        tab.add_css_class("active");
                    } else {
                        tab.remove_css_class("active");
                    }
                    update_tab_tooltip(tab, Some(tab_update.title.clone()));
                }
                if let Some(label) = strip.labels.get(idx) {
                    label.set_label(&tab_update.title);
                    if let Some(select) = strip.select_areas.get(idx) {
                        update_tab_tooltip(select, Some(tab_update.title.clone()));
                        select.update_property(&[gtk::accessible::Property::Label(&format!(
                            "Select tab {}",
                            tab_update.title
                        ))]);
                    }
                }
            }
        }
    }

    fn widget_for_pane(&self, node: &PaneNode, workspace_id: &str) -> gtk::Widget {
        let model = self.model.clone();
        let state_for_resize = self.state.clone();
        let workspace_id_for_resize = workspace_id.to_string();
        let pending_resize_save = Rc::new(RefCell::new(None::<glib::SourceId>));
        let on_resize: SplitResizeCallback =
            Rc::new(move |left: &[String], right: &[String], ratio: f64| {
                let changed = if let Ok(mut model) = model.lock() {
                    model.update_split_partition_ratio(&workspace_id_for_resize, left, right, ratio)
                } else {
                    false
                };
                if !changed {
                    return;
                }
                let Some(state) = state_for_resize.clone() else {
                    return;
                };
                if let Some(source_id) = pending_resize_save.borrow_mut().take() {
                    source_id.remove();
                }
                let state_for_timeout = state.clone();
                let pending_resize_save_for_timeout = pending_resize_save.clone();
                let source_id =
                    glib::timeout_add_local_once(SESSION_RESIZE_SAVE_DEBOUNCE, move || {
                        *pending_resize_save_for_timeout.borrow_mut() = None;
                        save_session_from_state(&state_for_timeout);
                    });
                *pending_resize_save.borrow_mut() = Some(source_id);
            });
        self.widget_for_pane_with_resize(node, on_resize)
    }

    fn widget_for_pane_with_resize(
        &self,
        node: &PaneNode,
        on_resize: SplitResizeCallback,
    ) -> gtk::Widget {
        match node {
            PaneNode::Leaf { tabs, active } => {
                let Some(active_index) = active_tab_index_for_leaf(tabs, *active) else {
                    return missing_surface_placeholder(
                        "empty-leaf",
                        self.state.as_ref(),
                        Some(&self.model),
                    )
                    .upcast();
                };
                let active_id = &tabs[active_index];
                if tabs.len() == 1 {
                    self.pane_widget_for(active_id)
                } else {
                    self.leaf_widget_with_tabstrip(tabs, active_index)
                }
            }
            PaneNode::Split {
                axis,
                children,
                sizes,
            } => {
                let orientation = match axis {
                    SplitAxis::Horizontal => gtk::Orientation::Horizontal,
                    SplitAxis::Vertical => gtk::Orientation::Vertical,
                };
                let on_resize_inner = on_resize.clone();
                build_split_widget(orientation, children, sizes, on_resize, move |child| {
                    self.widget_for_pane_with_resize(child, on_resize_inner.clone())
                })
            }
        }
    }

    fn pane_widget_for(&self, surface_id: &str) -> gtk::Widget {
        let kind = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.surface(surface_id).map(|s| s.kind.clone()));
        match kind {
            #[cfg(feature = "browser")]
            Some(forktty_core::SurfaceKind::Browser { url, profile }) => {
                self.browser_pane_widget(surface_id, &url, &profile.to_string())
            }
            #[cfg(not(feature = "browser"))]
            Some(forktty_core::SurfaceKind::Browser { .. }) => {
                browser_unavailable_placeholder(surface_id).upcast()
            }
            _ => self.terminal_pane_widget(surface_id),
        }
    }

    /// Build a compact per-pane tab strip for leaves with >1 tab.
    fn leaf_widget_with_tabstrip(&self, tabs: &[String], active: usize) -> gtk::Widget {
        let Some(active) = active_tab_index_for_leaf(tabs, active) else {
            return missing_surface_placeholder(
                "empty-tabs",
                self.state.as_ref(),
                Some(&self.model),
            )
            .upcast();
        };
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.set_hexpand(true);
        outer.set_vexpand(true);

        // Collect surface titles from model.
        let titles: Vec<String> = {
            let model = self.model.lock().ok();
            tabs.iter()
                .map(|id| {
                    model
                        .as_ref()
                        .and_then(|m| m.surface(id))
                        .map(surface_title)
                        .unwrap_or_else(|| "Terminal".to_string())
                })
                .collect()
        };

        let tabstrip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabstrip.add_css_class("pane-tabstrip");
        tabstrip.set_hexpand(true);
        let mut tab_widgets = Vec::with_capacity(tabs.len());
        let mut labels = Vec::with_capacity(tabs.len());
        let mut select_areas = Vec::with_capacity(tabs.len());

        for (idx, (surface_id, title)) in tabs.iter().zip(titles.iter()).enumerate() {
            let tab = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            tab.add_css_class("pane-tab");
            tab.set_valign(gtk::Align::Center);
            update_tab_tooltip(&tab, Some(title.clone()));
            if idx == active {
                tab.add_css_class("active");
            }

            let label = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(18)
                .single_line_mode(true)
                .build();
            label.add_css_class("pane-tab-label");

            let select = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            select.set_hexpand(true);
            select.set_valign(gtk::Align::Center);
            select.set_focusable(true);
            select.add_css_class("pane-tab-select");
            let grip = gtk::Image::from_icon_name("forktty-menu-symbolic");
            grip.add_css_class("pane-tab-grip");
            grip.set_tooltip_text(Some("Drag Tab"));
            select.append(&grip);
            select.append(&label);
            update_tab_tooltip(&select, Some(title.clone()));
            select.update_property(&[gtk::accessible::Property::Label(&format!(
                "Select tab {title}"
            ))]);

            let model_for_select = self.model.clone();
            let select_id = surface_id.clone();
            let primary_click = gtk::GestureClick::new();
            primary_click.set_button(gtk::gdk::BUTTON_PRIMARY);
            primary_click.connect_released(move |_, _n_press, _x, _y| {
                if let Ok(mut m) = model_for_select.lock() {
                    let _ = m.select_tab(&select_id);
                }
            });
            select.add_controller(primary_click);

            let close = gtk::Button::builder()
                .icon_name("forktty-close-symbolic")
                .has_frame(false)
                .build();
            close.add_css_class("flat");
            close.add_css_class("pane-tab-close");
            close.set_tooltip_text(Some("Close Tab"));
            set_accessible_button_text(&close, "Close Tab", None);

            let state_for_close = self.state.clone();
            let parent_for_close = self.parent_window.clone();
            let close_id = surface_id.clone();
            let is_last_tab = tabs.len() == 1;
            close.connect_clicked(move |_| {
                if is_last_tab {
                    if let Some(state) = &state_for_close {
                        show_close_pane_confirmation(&parent_for_close, state, &close_id);
                    }
                    return;
                }
                if let Some(state) = &state_for_close {
                    close_tab_surface(state, &close_id);
                }
            });

            if let Some(state) = &self.state {
                install_tab_context_menu(&tab, surface_id, state, &self.parent_window);
            }

            tab.append(&select);
            tab.append(&close);
            install_tab_drag_source(&tab, surface_id);
            tabstrip.append(&tab);
            tab_widgets.push(tab);
            labels.push(label);
            select_areas.push(select);
        }
        install_tabstrip_drop_target(
            &tabstrip,
            &tab_widgets,
            tabs,
            self.model.clone(),
            self.state.clone(),
        );
        outer.append(&tabstrip);

        // Keep tab children mounted and switch the visible child in-place so a
        // tab change does not rebuild the surrounding split tree.
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::None);
        for surface_id in tabs {
            let pane_widget = self.pane_widget_for(surface_id);
            stack.add_named(&pane_widget, Some(surface_id.as_str()));
        }
        let active_id = &tabs[active.min(tabs.len().saturating_sub(1))];
        stack.set_visible_child_name(active_id.as_str());
        outer.append(&stack);

        self.pane_tab_strips.borrow_mut().push(PaneTabStrip {
            tabs: tabs.to_vec(),
            stack,
            tab_widgets,
            labels,
            select_areas,
        });

        outer.upcast()
    }

    #[cfg(feature = "browser")]
    fn browser_pane_widget(&self, surface_id: &str, url: &str, profile_id: &str) -> gtk::Widget {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id) {
            // Widget self-guards on the last requested url; calling unconditionally
            // is harmless and avoids the committed-uri divergence problem.
            pane.load_uri(url);
            pane.set_active(self.is_surface_focused(surface_id));
            return pane.widget();
        }
        let pane = Rc::new(crate::browser_pane::BrowserPaneWidget::new(profile_id, url));
        pane.set_active(self.is_surface_focused(surface_id));
        if let Some(state) = &self.state {
            install_pane_reorder_dnd(&pane.drag_handle(), surface_id, state);
        }
        // Address-bar Enter navigates via the model so socket + manual share one path.
        let model = self.model.clone();
        let id = surface_id.to_string();
        pane.connect_address_activate(move |text| {
            let Some(normalized) = forktty_core::normalize_browser_url(&text) else {
                return;
            };
            if let Ok(mut m) = model.lock() {
                m.set_surface_url(&id, &normalized);
            }
        });
        // Keep model focus in sync when the browser pane gains focus, mirroring
        // the terminal focus handler, so focus-driven split/close target this pane.
        {
            let focus_model = self.model.clone();
            let focus_id = surface_id.to_string();
            pane.connect_focus_in(move || {
                if let Ok(mut m) = focus_model.lock() {
                    let _ = m.focus_surface(&focus_id);
                    let _ = m.mark_surface_unread(&focus_id, false);
                }
            });
        }
        // Keep the model's browser URL in sync with WebKit commits caused by
        // redirects, history buttons, or user link clicks. Without this, the
        // periodic refresh path sees the old model URL and navigates back.
        {
            let url_model = self.model.clone();
            let url_id = surface_id.to_string();
            pane.connect_uri_committed(move |url| {
                if let Ok(mut m) = url_model.lock() {
                    let _ = m.set_surface_url(&url_id, &url);
                }
            });
        }
        // Wire the × button to the same confirmation flow terminal panes use.
        if let Some(state) = self.state.clone() {
            let parent = self.parent_window.clone();
            let sid_close = surface_id.to_string();
            pane.connect_close(move || {
                show_close_pane_confirmation(&parent, &state, &sid_close);
            });
        }
        let widget = pane.widget();
        self.browser_panes
            .borrow_mut()
            .insert(surface_id.to_string(), pane);
        widget
    }

    #[cfg(feature = "browser")]
    fn is_surface_focused(&self, surface_id: &str) -> bool {
        self.model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .is_some_and(|workspace| workspace.focused_surface_id == surface_id)
    }

    #[cfg(feature = "browser")]
    pub(super) fn browser_pane(
        &self,
        surface_id: &str,
    ) -> Option<Rc<crate::browser_pane::BrowserPaneWidget>> {
        if let Some(pane) = self.browser_panes.borrow().get(surface_id).cloned() {
            return Some(pane);
        }
        let (url, profile_id) = self.model.lock().ok().and_then(|model| {
            model
                .surface(surface_id)
                .and_then(|surface| match &surface.kind {
                    forktty_core::SurfaceKind::Browser { url, profile } => {
                        Some((url.clone(), profile.to_string()))
                    }
                    _ => None,
                })
        })?;
        let _ = self.browser_pane_widget(surface_id, &url, &profile_id);
        self.browser_panes.borrow().get(surface_id).cloned()
    }

    fn terminal_pane_widget(&self, surface_id: &str) -> gtk::Widget {
        let Some(chrome) = self.chromes.get(surface_id) else {
            return missing_surface_placeholder(surface_id, self.state.as_ref(), Some(&self.model))
                .upcast();
        };
        let (surface, active) = self
            .model
            .lock()
            .ok()
            .and_then(|model| {
                let surface = model.surface(surface_id)?.clone();
                let active = model
                    .list_workspaces()
                    .into_iter()
                    .any(|workspace| workspace.focused_surface_id == surface_id);
                Some((surface, active))
            })
            .unwrap_or_else(|| {
                (
                    Surface {
                        id: surface_id.to_string(),
                        workspace_id: String::new(),
                        cwd: PathBuf::from("/"),
                        title: "Terminal".to_string(),
                        unread: false,
                        needs_attention: false,
                        kind: forktty_core::SurfaceKind::Terminal,
                        agent_session: None,
                        persisted_scrollback: None,
                    },
                    false,
                )
            });

        update_pane_chrome(chrome, &surface, active);
        chrome.pane.clone().upcast()
    }
}

/// Backend terminal surfaces that no longer map to any model surface are orphans
/// that must be torn down: this happens when a surface is restarted or closed on
/// the GTK thread while a socket client concurrently removes the same surface
/// from the model, so the GTK path re-spawns (or pre-spawns a replacement) into
/// the backend after the model entry is gone, stranding a hidden PTY and widget.
/// Surfaces with a freshly in-flight spawn are excluded because their backend
/// entry exists before the matching `Spawn` command commits the widget. A
/// pending spawn is only protected for one reconciliation after its backend
/// appears while absent from the model; if the model commit is lost, the next
/// reconciliation drops the pending marker and reaps the orphan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PendingSpawn {
    observed_unmodeled_backend: bool,
}

pub(super) fn orphaned_backend_surfaces(
    backend_ids: &BTreeSet<String>,
    model_ids: &BTreeSet<String>,
    pending: &BTreeMap<String, PendingSpawn>,
) -> Vec<String> {
    backend_ids
        .iter()
        .filter(|id| !model_ids.contains(*id) && !pending.contains_key(*id))
        .cloned()
        .collect()
}

pub(super) fn mark_spawn_command_pending(
    pending: &mut BTreeMap<String, PendingSpawn>,
    surface_id: &str,
) {
    pending.insert(surface_id.to_string(), PendingSpawn::default());
}

pub(super) fn clear_modeled_pending_spawns(
    pending: &mut BTreeMap<String, PendingSpawn>,
    model_ids: &BTreeSet<String>,
    backend_ids: &BTreeSet<String>,
) {
    pending.retain(|id, spawn| {
        if model_ids.contains(id) {
            return false;
        }
        if backend_ids.contains(id) {
            let keep = !spawn.observed_unmodeled_backend;
            spawn.observed_unmodeled_backend = true;
            return keep;
        }
        true
    });
}

fn tab_strip_refreshes(model: &WorkspaceModel, strip_tabs: &[Vec<String>]) -> Vec<TabStripRefresh> {
    let workspace = model.active_workspace();
    strip_tabs
        .iter()
        .map(|tabs| {
            let active_id = workspace
                .as_ref()
                .and_then(|workspace| active_tab_for_tabs(&workspace.pane_tree, tabs));
            let tabs = tabs
                .iter()
                .map(|surface_id| {
                    let title = model
                        .surface(surface_id)
                        .map(surface_title)
                        .unwrap_or_else(|| "Terminal".to_string());
                    let active = active_id.as_deref() == Some(surface_id.as_str());
                    TabRefresh { title, active }
                })
                .collect();
            TabStripRefresh { active_id, tabs }
        })
        .collect()
}

pub(super) fn chrome_refresh_signature(
    model: &WorkspaceModel,
    chrome_surface_ids: &[String],
    strip_tabs: &[Vec<String>],
) -> String {
    let focused_surface_id = model
        .active_workspace()
        .map(|workspace| workspace.focused_surface_id);
    let mut signature = String::new();
    signature.push_str("focus=");
    if let Some(focused_surface_id) = focused_surface_id.as_deref() {
        signature.push_str(focused_surface_id);
    }
    signature.push('\n');

    for surface_id in chrome_surface_ids {
        signature.push_str("chrome=");
        signature.push_str(surface_id);
        if let Some(surface) = model.surface(surface_id) {
            signature.push('|');
            signature.push_str(&surface_title(surface));
            signature.push('|');
            signature.push_str(&surface.cwd.to_string_lossy());
            signature.push('|');
            signature.push_str(if surface.unread { "u1" } else { "u0" });
            signature.push('|');
            signature.push_str(if surface.needs_attention { "a1" } else { "a0" });
            signature.push('|');
            signature.push_str(if surface.agent_session.is_some() {
                "agent1"
            } else {
                "agent0"
            });
            if let forktty_core::SurfaceKind::Browser { url, .. } = &surface.kind {
                signature.push_str("|browser=");
                signature.push_str(url);
            }
        }
        signature.push('\n');
    }

    let workspace = model.active_workspace();
    for tabs in strip_tabs {
        signature.push_str("tabs=");
        if let Some(active_id) = workspace
            .as_ref()
            .and_then(|workspace| active_tab_for_tabs(&workspace.pane_tree, tabs))
        {
            signature.push_str(&active_id);
        }
        for surface_id in tabs {
            signature.push('|');
            signature.push_str(surface_id);
            signature.push(':');
            if let Some(surface) = model.surface(surface_id) {
                signature.push_str(&surface_title(surface));
            }
        }
        signature.push('\n');
    }

    signature
}

fn install_tabstrip_drop_target(
    tabstrip: &gtk::Box,
    tab_widgets: &[gtk::Box],
    surface_ids: &[String],
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
) {
    let strip_for_drop = tabstrip.clone().upcast::<gtk::Widget>();
    let tab_targets = surface_ids
        .iter()
        .cloned()
        .zip(
            tab_widgets
                .iter()
                .map(|tab| tab.clone().upcast::<gtk::Widget>()),
        )
        .collect::<Vec<_>>();

    install_tab_drop_target_on(&strip_for_drop, &strip_for_drop, &tab_targets, model, state);
}

fn install_tab_drop_target_on(
    handle: &gtk::Widget,
    tabstrip: &gtk::Widget,
    tab_targets: &[(String, gtk::Widget)],
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
) {
    let handle_for_drop = handle.clone();
    let strip_for_drop = tabstrip.clone();
    let tab_targets = tab_targets.to_vec();
    let tab_order = tab_targets
        .iter()
        .map(|(surface_id, _)| surface_id.clone())
        .collect::<Vec<_>>();
    let tab_targets_for_drop = tab_targets.clone();
    let tab_order_for_drop = tab_order.clone();
    let tab_targets_for_motion = tab_targets.clone();
    let tab_order_for_motion = tab_order.clone();
    let handle_for_motion = handle.clone();
    let strip_for_motion = tabstrip.clone();
    let target = tab_drop_target(move |source_id, x, y| {
        clear_tab_drop_indicators(&tab_targets_for_drop);
        let Some((strip_x, _)) = handle_for_drop.translate_coordinates(&strip_for_drop, x, y)
        else {
            return false;
        };
        let midpoints = tab_drop_midpoints(&strip_for_drop, &tab_targets_for_drop);
        let Some((target_id, position)) =
            tab_drop_target_at_x(&midpoints, strip_x).filter(|(target_id, position)| {
                !tab_move_would_keep_order(&tab_order_for_drop, &source_id, target_id, *position)
            })
        else {
            return false;
        };
        let moved = model
            .lock()
            .ok()
            .is_some_and(|mut model| model.move_tab(&source_id, target_id, position));
        if moved {
            if let Some(state) = state.as_ref() {
                save_session_from_state(state);
            }
        }
        moved
    });
    target.set_preload(true);
    target.connect_motion(move |target, x, y| {
        let Some(source_id) = target
            .value()
            .and_then(|value| tab_dnd_id_from_value(&value))
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::MOVE;
        };
        let Some((strip_x, _)) = handle_for_motion.translate_coordinates(&strip_for_motion, x, y)
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::empty();
        };
        let midpoints = tab_drop_midpoints(&strip_for_motion, &tab_targets_for_motion);
        let Some((target_id, position)) =
            tab_drop_target_at_x(&midpoints, strip_x).filter(|(target_id, position)| {
                !tab_move_would_keep_order(&tab_order_for_motion, &source_id, target_id, *position)
            })
        else {
            clear_tab_drop_indicators(&tab_targets_for_motion);
            return gdk::DragAction::empty();
        };
        set_tab_drop_indicator(&tab_targets_for_motion, target_id, position);
        gdk::DragAction::MOVE
    });
    let tab_targets_for_leave = tab_targets.clone();
    target.connect_leave(move |_| {
        clear_tab_drop_indicators(&tab_targets_for_leave);
    });
    handle.add_controller(target);
}

fn tab_drop_midpoints(
    tabstrip: &gtk::Widget,
    tab_targets: &[(String, gtk::Widget)],
) -> Vec<(String, f64)> {
    tab_targets
        .iter()
        .filter_map(|(surface_id, tab)| {
            let (tab_x, _) = tab.translate_coordinates(tabstrip, 0.0, 0.0)?;
            Some((
                surface_id.clone(),
                tab_x + f64::from(tab.allocated_width()) / 2.0,
            ))
        })
        .collect()
}

pub(super) fn tab_drop_target_at_x(
    tab_midpoints: &[(String, f64)],
    x: f64,
) -> Option<(&str, forktty_core::MovePosition)> {
    for (surface_id, midpoint) in tab_midpoints {
        if x < *midpoint {
            return Some((surface_id.as_str(), forktty_core::MovePosition::Before));
        }
    }
    tab_midpoints
        .last()
        .map(|(surface_id, _)| (surface_id.as_str(), forktty_core::MovePosition::After))
}

pub(super) fn tab_move_would_keep_order(
    tab_order: &[String],
    source_id: &str,
    target_id: &str,
    position: forktty_core::MovePosition,
) -> bool {
    if source_id == target_id {
        return true;
    }
    let source_index = tab_order
        .iter()
        .position(|surface_id| surface_id == source_id);
    let target_index = tab_order
        .iter()
        .position(|surface_id| surface_id == target_id);
    matches!(
        (source_index, target_index, position),
        (Some(source), Some(target), forktty_core::MovePosition::Before)
            if source + 1 == target
    ) || matches!(
        (source_index, target_index, position),
        (Some(source), Some(target), forktty_core::MovePosition::After)
            if target + 1 == source
    )
}

fn clear_tab_drop_indicators(tab_targets: &[(String, gtk::Widget)]) {
    for (_, tab) in tab_targets {
        tab.remove_css_class("drop-before");
        tab.remove_css_class("drop-after");
    }
}

fn set_tab_drop_indicator(
    tab_targets: &[(String, gtk::Widget)],
    target_id: &str,
    position: forktty_core::MovePosition,
) {
    clear_tab_drop_indicators(tab_targets);
    let Some((_, tab)) = tab_targets
        .iter()
        .find(|(surface_id, _)| surface_id == target_id)
    else {
        return;
    };
    match position {
        forktty_core::MovePosition::Before => tab.add_css_class("drop-before"),
        forktty_core::MovePosition::After => tab.add_css_class("drop-after"),
    }
}

// In maximize mode only the focused pane is rendered, so the layout signature
// must change when focus moves between panes even though the tree is the same.
pub(super) fn effective_layout_signature(
    signature: &str,
    maximized: bool,
    focused_surface_id: &str,
) -> String {
    if maximized {
        format!("{signature}#max:{focused_surface_id}")
    } else {
        signature.to_string()
    }
}

fn install_embedded_ghostty_accelerators(widget: &gtk::Widget, embedder: Rc<GhosttyGtkEmbedder>) {
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    // Hold the widget weakly: the controller is owned by `widget`, so a strong
    // clone here forms a widget -> controller -> closure -> widget cycle that
    // keeps the embedded Ghostty surface alive forever after the pane closes.
    let widget_for_key = widget.downgrade();
    key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
        let Some(widget_for_key) = widget_for_key.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(action) = embedded_surface_action_for_accelerator(key, modifiers) else {
            return glib::Propagation::Proceed;
        };
        match unsafe { embedder.perform_action(&widget_for_key, action) } {
            Ok(_) => glib::Propagation::Stop,
            Err(err) => {
                eprintln!(
                    "forktty: embedded Ghostty {} unavailable: {err}",
                    action.as_ghostty_action()
                );
                glib::Propagation::Proceed
            }
        }
    });
    widget.add_controller(key_controller);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EmbeddedContextMenuAction {
    pub(super) icon_name: &'static str,
    pub(super) label: &'static str,
    pub(super) action: EmbeddedSurfaceAction,
}

pub(super) const EMBEDDED_CONTEXT_MENU_ACTIONS: &[EmbeddedContextMenuAction] = &[
    EmbeddedContextMenuAction {
        icon_name: "forktty-copy-symbolic",
        label: "Copy",
        action: EmbeddedSurfaceAction::Copy,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-paste-symbolic",
        label: "Paste",
        action: EmbeddedSurfaceAction::Paste,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-select-all-symbolic",
        label: "Select All",
        action: EmbeddedSurfaceAction::SelectAll,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-search-symbolic",
        label: "Find",
        action: EmbeddedSurfaceAction::StartSearch,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-clear-symbolic",
        label: "Reset and Clear",
        action: EmbeddedSurfaceAction::ClearScreen,
    },
];

pub(super) fn embedded_surface_action_for_accelerator(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
) -> Option<EmbeddedSurfaceAction> {
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    if !ctrl || !shift || alt {
        return None;
    }
    match key {
        gtk::gdk::Key::C | gtk::gdk::Key::c => Some(EmbeddedSurfaceAction::Copy),
        gtk::gdk::Key::V | gtk::gdk::Key::v => Some(EmbeddedSurfaceAction::Paste),
        gtk::gdk::Key::A | gtk::gdk::Key::a => Some(EmbeddedSurfaceAction::SelectAll),
        gtk::gdk::Key::F | gtk::gdk::Key::f => Some(EmbeddedSurfaceAction::StartSearch),
        _ => None,
    }
}

fn perform_embedded_context_action(
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    action: EmbeddedSurfaceAction,
) -> bool {
    match unsafe { embedder.perform_action(widget, action) } {
        Ok(performed) => performed,
        Err(err) => {
            eprintln!(
                "forktty: embedded Ghostty {} unavailable: {err}",
                action.as_ghostty_action()
            );
            false
        }
    }
}

fn handle_embedded_right_click_action(
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    action: TerminalRightClickAction,
) -> bool {
    match action {
        TerminalRightClickAction::ContextMenu => false,
        TerminalRightClickAction::Ignore => true,
        TerminalRightClickAction::Copy => {
            perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Copy);
            true
        }
        TerminalRightClickAction::Paste => {
            perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Paste);
            true
        }
        TerminalRightClickAction::CopyOrPaste => {
            if !perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Copy) {
                perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Paste);
            }
            true
        }
    }
}

fn build_embedded_ghostty_context_menu(
    state: &SocketAppState,
    surface_id: &str,
    widget: &gtk::Widget,
    parent: &adw::ApplicationWindow,
    embedder: Rc<GhosttyGtkEmbedder>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");

    let snapshot = terminal_context_snapshot(state, surface_id);
    if let Some((workspace, surface)) = snapshot.as_ref() {
        add_terminal_context_menu_header(&menu, workspace, surface);
        add_context_menu_separator(&menu);
    }

    for item in EMBEDDED_CONTEXT_MENU_ACTIONS {
        let embedder_for_action = Rc::clone(&embedder);
        let widget_for_action = widget.clone();
        let action = item.action;
        add_context_menu_item(
            &menu,
            &popover,
            item.icon_name,
            item.label,
            false,
            move || {
                perform_embedded_context_action(&embedder_for_action, &widget_for_action, action);
            },
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
        "Split Right",
        false,
        move || {
            focus_surface_and(&state_, &sid, |state| {
                split_active_surface(state, SplitAxis::Horizontal)
            });
        },
    );

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-vertical-symbolic",
        "Split Down",
        false,
        move || {
            focus_surface_and(&state_, &sid, |state| {
                split_active_surface(state, SplitAxis::Vertical)
            });
        },
    );

    add_context_menu_separator(&menu);

    if let Some((workspace, surface)) = snapshot {
        let workspace_id = workspace.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-copy-symbolic",
            "Copy Workspace ID",
            false,
            move || copy_to_clipboard(&workspace_id),
        );

        let surface_id = surface.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-copy-symbolic",
            "Copy Surface ID",
            false,
            move || copy_to_clipboard(&surface_id),
        );

        let identifiers = format!(
            "workspace_id={}\nsurface_id={}\nsocket_path={}",
            workspace.id,
            surface.id,
            state.socket_path.to_string_lossy()
        );
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-copy-symbolic",
            "Copy IDs",
            false,
            move || copy_to_clipboard(&identifiers),
        );

        let cwd = surface.cwd.to_string_lossy().to_string();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-folder-symbolic",
            "Copy Working Directory",
            false,
            move || copy_to_clipboard(&cwd),
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-refresh-symbolic",
        "Restart Pane",
        false,
        move || {
            restart_surface(&state_, &sid);
        },
    );

    let state_ = state.clone();
    let parent_ = parent.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-close-symbolic",
        "Close Pane",
        true,
        move || {
            show_close_pane_confirmation(&parent_, &state_, &sid);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

fn install_embedded_ghostty_context_menu(
    widget: &gtk::Widget,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
    embedder: Rc<GhosttyGtkEmbedder>,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let widget_for_menu = widget.downgrade();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();

    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        let Some(widget_for_menu) = widget_for_menu.upgrade() else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_menu.grab_focus();
        if let Ok(mut model) = state_for_menu.model.lock() {
            let _ = model.focus_surface(&surface_id_for_menu);
            let _ = model.mark_surface_unread(&surface_id_for_menu, false);
        }

        let config = config::load_config().unwrap_or_default();
        if handle_embedded_right_click_action(
            &embedder,
            &widget_for_menu,
            terminal_right_click_action_for_config(&config),
        ) {
            return;
        }

        let previous_popover = current_popover_for_menu.borrow_mut().take();
        if let Some(popover) = previous_popover {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }

        let popover = build_embedded_ghostty_context_menu(
            &state_for_menu,
            &surface_id_for_menu,
            &widget_for_menu,
            &parent_for_menu,
            Rc::clone(&embedder),
        );
        let current_popover_for_closed = current_popover_for_menu.clone();
        popover.connect_closed(move |popover| {
            let should_clear = current_popover_for_closed
                .borrow()
                .as_ref()
                .is_some_and(|current| current == popover);
            if should_clear {
                current_popover_for_closed.borrow_mut().take();
            }
            if popover.parent().is_some() {
                popover.unparent();
            }
        });

        let (popover_x, popover_y) = widget_for_menu
            .translate_coordinates(&parent_for_menu, x, y)
            .unwrap_or((x, y));
        popover.set_parent(&parent_for_menu);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            popover_x.round() as i32,
            popover_y.round() as i32,
            1,
            1,
        )));
        *current_popover_for_menu.borrow_mut() = Some(popover.clone());
        popover.popup();
    });
    widget.add_controller(gesture);
}

fn install_tab_context_menu(
    tab: &gtk::Box,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tab_for_menu = tab.clone();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();
    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        // Drop the RefMut before popdown(): it emits `closed` synchronously and
        // its handler re-borrows the same cell.
        let previous_popover = current_popover_for_menu.borrow_mut().take();
        if let Some(popover) = previous_popover {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
        let popover = build_tab_context_menu(&state_for_menu, &surface_id_for_menu);
        let current_popover_for_closed = current_popover_for_menu.clone();
        popover.connect_closed(move |popover| {
            let should_clear = current_popover_for_closed
                .borrow()
                .as_ref()
                .is_some_and(|current| current == popover);
            if should_clear {
                current_popover_for_closed.borrow_mut().take();
            }
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        let (popover_x, popover_y) = tab_for_menu
            .translate_coordinates(&parent_for_menu, x, y)
            .unwrap_or((x, y));
        popover.set_parent(&parent_for_menu);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            popover_x.round() as i32,
            popover_y.round() as i32,
            1,
            1,
        )));
        *current_popover_for_menu.borrow_mut() = Some(popover.clone());
        popover.popup();
    });
    tab.add_controller(gesture);
}

fn build_tab_context_menu(state: &SocketAppState, surface_id: &str) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");
    if let Some((workspace, surface)) = terminal_context_snapshot(state, surface_id) {
        add_terminal_context_menu_header(&menu, &workspace, &surface);
        add_context_menu_separator(&menu);
    }

    let state_for_new_tab = state.clone();
    let new_tab_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-add-symbolic",
        "New Tab",
        false,
        move || add_new_tab_surface(&state_for_new_tab, &new_tab_id),
    );

    let state_for_split = state.clone();
    let split_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
        "Split Right",
        false,
        move || {
            focus_surface_and(&state_for_split, &split_id, |state| {
                split_active_surface(state, SplitAxis::Horizontal)
            });
        },
    );

    let state_for_split = state.clone();
    let split_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-vertical-symbolic",
        "Split Down",
        false,
        move || {
            focus_surface_and(&state_for_split, &split_id, |state| {
                split_active_surface(state, SplitAxis::Vertical)
            });
        },
    );

    add_context_menu_separator(&menu);

    let state_for_close = state.clone();
    let close_id = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-close-symbolic",
        "Close Tab",
        true,
        move || {
            close_tab_surface(&state_for_close, &close_id);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_ghostty_view_wraps_surface_in_vertical_scroller() {
        if gtk::init().is_err() {
            return;
        }

        let surface = gtk::TextView::new().upcast::<gtk::Widget>();
        let view = build_embedded_ghostty_scroll_view(&surface, GhosttyScrollbarPolicy::System);
        let scroller = view
            .downcast::<gtk::ScrolledWindow>()
            .expect("embedded view should be a scrolled window");

        assert_eq!(
            scroller.policy(),
            (gtk::PolicyType::Never, gtk::PolicyType::Automatic)
        );
        assert!(scroller.property::<bool>("overlay-scrolling"));
        assert_eq!(scroller.child().as_ref(), Some(&surface));
        assert!(surface.property::<bool>("hexpand"));
        assert!(surface.property::<bool>("vexpand"));

        let hidden_surface = gtk::TextView::new().upcast::<gtk::Widget>();
        let hidden =
            build_embedded_ghostty_scroll_view(&hidden_surface, GhosttyScrollbarPolicy::Never)
                .downcast::<gtk::ScrolledWindow>()
                .expect("hidden policy still uses a scrolled window");
        assert_eq!(
            hidden.policy(),
            (gtk::PolicyType::Never, gtk::PolicyType::Never)
        );
    }
}
