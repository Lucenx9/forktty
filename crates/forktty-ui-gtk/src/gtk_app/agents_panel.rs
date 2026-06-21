use super::*;
use forktty_core::agent_resume_command_with_cwd_and_permission_mode;
use forktty_socket::dispatch;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentHudRow {
    pub(super) surface_id: String,
    pub(super) workspace_id: String,
    pub(super) workspace_name: String,
    pub(super) agent_label: &'static str,
    pub(super) section_label: &'static str,
    pub(super) lifecycle_label: &'static str,
    pub(super) lifecycle_class: &'static str,
    pub(super) current: bool,
    pub(super) compact: bool,
    pub(super) needs_input: bool,
    pub(super) surface_title: String,
    pub(super) cwd_label: String,
    pub(super) session_short: String,
    pub(super) permission_mode: Option<String>,
    pub(super) permission_mode_risky: bool,
    pub(super) last_activity_label: String,
    pub(super) can_resume: bool,
    /// The surface has produced output the user has not looked at since last
    /// focusing it. Distinct from lifecycle: an *idle* agent with unread output
    /// has finished and its result is still unseen — the case the HUD exists to
    /// surface. Drives a per-row dot and floats the row up within its group.
    pub(super) unread: bool,
    /// What the agent is waiting on, for needs-input rows: the body of the
    /// surface's most recent prompt notification (the hook message, e.g.
    /// "Claude needs your permission to use Bash"). Shown instead of the raw
    /// terminal tail, which for full-screen TUIs is just their status bar.
    pub(super) attention_hint: Option<String>,
}

pub(super) fn agent_hud_rows(model: &WorkspaceModel, now_ms: u64) -> Vec<AgentHudRow> {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| (workspace.id.clone(), workspace))
        .collect::<BTreeMap<_, _>>();
    let current_surface_id = model
        .active_workspace()
        .map(|workspace| workspace.focused_surface_id.clone());
    let notifications = model.list_notifications();
    let mut rows = model
        .list_surfaces(None)
        .into_iter()
        .filter_map(|surface| {
            let agent_session = surface.agent_session.clone()?;
            let workspace = workspaces.get(&surface.workspace_id)?;
            let compact = agent_session.lifecycle == forktty_core::AgentSessionLifecycle::Ended;
            // Only the persisted cwd, never the disk-walking Codex fallback:
            // this runs for every row on the panel's one-second refresh, and
            // `codex_session_cwd` scans `~/.codex/sessions` from disk — too
            // costly to repeat on the GTK thread each tick. The button is
            // advisory; `agent.resume` resolves the real cwd server-side, and
            // a missing cwd never makes the command unconstructible anyway.
            let can_resume = agent_resume_command_with_cwd_and_permission_mode(
                agent_session.agent,
                &agent_session.session_id,
                agent_session.resume_cwd.as_deref(),
                agent_session.permission_mode.as_deref(),
            )
            .is_ok();
            let (lifecycle_label, lifecycle_class, priority) =
                lifecycle_presentation(agent_session.lifecycle);
            let needs_input =
                agent_session.lifecycle == forktty_core::AgentSessionLifecycle::NeedsInput;
            let permission_mode_risky = agent_session
                .permission_mode
                .as_deref()
                .is_some_and(permission_mode_is_risky);
            let attention_hint = needs_input
                .then(|| {
                    notifications
                        .iter()
                        .rev()
                        .find(|notification| {
                            notification.kind == NotificationKind::Prompt
                                && notification.surface_id.as_deref() == Some(surface.id.as_str())
                        })
                        .map(|notification| notification.body.clone())
                })
                .flatten();
            Some((
                priority,
                AgentHudRow {
                    surface_id: surface.id.clone(),
                    workspace_id: surface.workspace_id.clone(),
                    workspace_name: workspace.name.clone(),
                    agent_label: agent_kind_label(agent_session.agent),
                    section_label: lifecycle_section_label(agent_session.lifecycle),
                    lifecycle_label,
                    lifecycle_class,
                    current: current_surface_id.as_deref() == Some(surface.id.as_str()),
                    compact,
                    needs_input,
                    surface_title: surface_title(&surface),
                    cwd_label: compact_path(&surface.cwd),
                    session_short: short_session_id(&agent_session.session_id),
                    permission_mode: agent_session.permission_mode.clone(),
                    permission_mode_risky,
                    last_activity_label: last_activity_label(
                        agent_session.last_activity_ms,
                        now_ms,
                    ),
                    can_resume,
                    unread: surface.unread,
                    attention_hint,
                },
            ))
        })
        .collect::<Vec<_>>();
    // Attention first (lower priority sorts earlier), then a stable grouping by
    // workspace/agent/surface. The priority comes straight from
    // `lifecycle_presentation` so it can never drift from the labels.
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            // Within a lifecycle group, unseen output floats up (true first).
            .then_with(|| b.1.unread.cmp(&a.1.unread))
            .then_with(|| a.1.workspace_name.cmp(&b.1.workspace_name))
            .then_with(|| a.1.agent_label.cmp(b.1.agent_label))
            .then_with(|| a.1.surface_id.cmp(&b.1.surface_id))
    });
    rows.into_iter().map(|(_, row)| row).collect()
}

pub(super) fn agent_attention_count(state: &SocketAppState) -> usize {
    state
        .model
        .lock()
        .ok()
        .map(|model| {
            model
                .list_surfaces(None)
                .into_iter()
                .filter_map(|surface| surface.agent_session)
                .filter(|session| {
                    session.lifecycle == forktty_core::AgentSessionLifecycle::NeedsInput
                })
                .count()
        })
        .unwrap_or(0)
}

pub(super) fn refresh_agent_indicator(
    button: &gtk::Button,
    badge: &gtk::Label,
    state: &SocketAppState,
) {
    let count = agent_attention_count(state);
    if count == 0 {
        button.remove_css_class("needs-attention");
        badge.set_visible(false);
        badge.set_label("");
        button.set_tooltip_text(Some("Agents"));
        set_accessible_button_text(button, "Agents", None);
        return;
    }
    button.add_css_class("needs-attention");
    badge.set_label(&count.to_string());
    badge.set_visible(true);
    let label = if count == 1 {
        "1 agent needs input".to_string()
    } else {
        format!("{count} agents need input")
    };
    button.set_tooltip_text(Some(&label));
    set_accessible_button_text(button, &label, None);
}

/// Live view state of the agents dialog. A one-second timer re-snapshots the
/// model, rebuilds the list only when the rows actually changed, and patches
/// the output-tail labels in place otherwise — so the buttons under the
/// pointer are not torn down on every tick.
#[derive(Clone)]
struct AgentPanelUi {
    dialog: gtk::Window,
    toast_overlay: adw::ToastOverlay,
    state: SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
    body: gtk::Box,
    subtitle: gtk::Label,
    rendered: Rc<RefCell<Vec<AgentHudRow>>>,
    tail_labels: Rc<RefCell<BTreeMap<String, gtk::Label>>>,
    /// Per-surface tail cache; the generation in each entry lets
    /// `surface_tail_line` skip its full-scrollback read while an agent
    /// produces no output.
    tails: Rc<RefCell<BTreeMap<String, AgentTailEntry>>>,
}

/// `(content_generation, last output line)` snapshot for one agent surface.
pub(super) type AgentTailEntry = (u64, Option<String>);

impl AgentPanelUi {
    fn refresh(&self, first: bool) {
        let rows = self
            .state
            .model
            .lock()
            .ok()
            .map(|model| agent_hud_rows(&model, now_epoch_ms()))
            .unwrap_or_default();
        self.refresh_tails(&rows);
        // Never tear the list down mid-typing: while a reply entry holds
        // focus the structural rebuild waits (rendered stays stale, so the
        // next tick retries); tail labels still update in place below.
        let structure_changed = first || *self.rendered.borrow() != rows;
        if structure_changed && (first || !self.reply_entry_focused()) {
            let needs_input = rows.iter().filter(|row| row.needs_input).count();
            self.subtitle
                .set_label(&agent_panel_subtitle(rows.len(), needs_input));
            while let Some(child) = self.body.first_child() {
                self.body.remove(&child);
            }
            let mut tail_labels = BTreeMap::new();
            if rows.is_empty() {
                self.body.append(&compact_status_page(
                    "forktty-terminal-symbolic",
                    "No Agents",
                    "Agent sessions will appear here after ForkTTY receives hook metadata.",
                ));
            } else {
                self.body.append(&self.build_list(&rows, &mut tail_labels));
            }
            *self.tail_labels.borrow_mut() = tail_labels;
            *self.rendered.borrow_mut() = rows;
        }
        // Sync tail labels: a needs-input row shows what the agent is waiting
        // on (its attention hint), every other row the last output line. Runs
        // every tick so in-place tail updates need no rebuild.
        let rendered = self.rendered.borrow();
        let tails = self.tails.borrow();
        for (surface_id, label) in self.tail_labels.borrow().iter() {
            let hint = rendered
                .iter()
                .find(|row| &row.surface_id == surface_id)
                .and_then(|row| row.attention_hint.as_deref());
            let text = hint
                .or_else(|| tails.get(surface_id).and_then(|(_, line)| line.as_deref()))
                .unwrap_or("");
            if label.label() != text {
                label.set_label(text);
                if text.is_empty() {
                    label.set_tooltip_text(None);
                } else {
                    label.set_tooltip_text(Some(text));
                }
            }
            if hint.is_some() {
                label.add_css_class("attention");
            } else {
                label.remove_css_class("attention");
            }
            label.set_visible(!text.is_empty());
        }
    }

    /// Whether keyboard focus sits in one of this panel's reply entries.
    fn reply_entry_focused(&self) -> bool {
        gtk::prelude::RootExt::focus(&self.dialog)
            .is_some_and(|widget| widget.is::<gtk::Entry>() && widget.is_ancestor(&self.body))
    }

    /// Re-reads each listed agent's output tail, generation-gated so an idle
    /// agent costs one integer compare per tick. Surfaces that dropped out of
    /// the rows fall out of the cache.
    fn refresh_tails(&self, rows: &[AgentHudRow]) {
        let Some(controller) = self.controller.as_ref() else {
            return;
        };
        // try_borrow: a nested main loop iteration must never abort the app
        // if it fires this timer while the controller is mutably borrowed.
        let Ok(controller) = controller.try_borrow() else {
            return;
        };
        let tails = &mut *self.tails.borrow_mut();
        let mut next = BTreeMap::new();
        for row in rows {
            if let Some(entry) =
                controller.surface_tail_line(&row.surface_id, tails.get(&row.surface_id))
            {
                next.insert(row.surface_id.clone(), entry);
            }
        }
        *tails = next;
    }

    fn build_list(
        &self,
        rows: &[AgentHudRow],
        tail_labels: &mut BTreeMap<String, gtk::Label>,
    ) -> gtk::ScrolledWindow {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("agent-list");
        list.update_property(&[gtk::accessible::Property::Label("Agents list")]);
        let mut row_targets = Vec::new();
        let mut previous_section = None;
        for row in rows {
            if previous_section != Some(row.section_label) {
                append_agent_section_header(&list, row.section_label);
                row_targets.push(None);
                previous_section = Some(row.section_label);
            }
            append_agent_row(&list, self, row, tail_labels);
            row_targets.push(Some(row.surface_id.clone()));
        }
        // Enter (or a click outside the buttons) on a row focuses that agent.
        let state = self.state.clone();
        let controller = self.controller.clone();
        let dialog = self.dialog.clone();
        list.connect_row_activated(move |_, row| {
            let Some(surface_id) = agent_surface_for_row_index(row.index(), &row_targets) else {
                return;
            };
            if open_agent_surface(&state, surface_id, controller.as_ref()) {
                dialog.close();
            }
        });
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build()
    }
}

fn append_agent_section_header(list: &gtk::ListBox, label: &'static str) {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let header = gtk::Label::builder().label(label).xalign(0.0).build();
    header.add_css_class("agent-section");
    row.set_child(Some(&header));
    list.append(&row);
}

pub(super) fn agent_surface_for_row_index(
    row_index: i32,
    row_targets: &[Option<String>],
) -> Option<&str> {
    let index = usize::try_from(row_index).ok()?;
    row_targets.get(index)?.as_deref()
}

pub(super) fn show_agent_panel(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
) {
    let has_agents = state
        .model
        .lock()
        .ok()
        .map(|model| {
            model
                .list_surfaces(None)
                .iter()
                .any(|surface| surface.agent_session.is_some())
        })
        .unwrap_or(false);

    let dialog = gtk::Window::builder()
        .title("Agents")
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(620)
        .default_height(if has_agents { 320 } else { 300 })
        .build();
    dialog.set_size_request(480, 300);
    dialog.add_css_class("ft-dialog");
    dialog.add_css_class("agent-panel");
    apply_resizable_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Agents").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder().xalign(0.0).build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("ft-dialog-body");
    body.set_vexpand(true);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&root));
    dialog.set_child(Some(&toast_overlay));

    let ui = AgentPanelUi {
        dialog: dialog.clone(),
        toast_overlay,
        state: state.clone(),
        controller,
        body,
        subtitle,
        rendered: Rc::new(RefCell::new(Vec::new())),
        tail_labels: Rc::new(RefCell::new(BTreeMap::new())),
        tails: Rc::new(RefCell::new(BTreeMap::new())),
    };
    ui.refresh(true);
    dialog.present();
    // Live updates while the dialog is open; closing it (destroy or
    // hide-on-close) flips visibility off and stops the timer.
    glib::timeout_add_local(Duration::from_secs(1), move || {
        if !ui.dialog.is_visible() {
            return glib::ControlFlow::Break;
        }
        ui.refresh(false);
        glib::ControlFlow::Continue
    });
}

pub(super) fn open_agent_surface(
    state: &SocketAppState,
    surface_id: &str,
    controller: Option<&Rc<RefCell<TerminalController>>>,
) -> bool {
    let workspace_id = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return false,
        };
        let Some(surface) = model.surface(surface_id).cloned() else {
            return false;
        };
        if !model.focus_surface(surface_id) {
            return false;
        }
        surface.workspace_id
    };

    match select_workspace_with_terminal(state, &workspace_id) {
        Ok(true) => {
            if let Ok(mut model) = state.model.lock() {
                let _ = model.mark_surface_unread(surface_id, false);
            }
            if let Some(controller) = controller {
                controller.borrow_mut().rebuild_layout();
                controller.borrow_mut().sync_model_focus_to_ui();
            }
            true
        }
        Ok(false) => false,
        Err(err) => {
            eprintln!("Failed to open agent surface {surface_id}: {err}");
            create_global_notification(
                state,
                "Open Agent Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            false
        }
    }
}

pub(super) fn agent_reply_payload(text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| format!("{text}\r"))
}

pub(super) fn forget_agent_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Option<forktty_core::ClearedAgentSession> {
    state
        .model
        .lock()
        .ok()?
        .clear_surface_agent_session_with_metadata(surface_id)
}

pub(super) fn restore_forgotten_agent_surface(
    state: &SocketAppState,
    surface_id: &str,
    agent_session: forktty_core::ClearedAgentSession,
) -> bool {
    state
        .model
        .lock()
        .map(|mut model| {
            model.restore_surface_agent_session_with_metadata(surface_id, agent_session)
        })
        .unwrap_or(false)
}

fn append_agent_row(
    list: &gtk::ListBox,
    ui: &AgentPanelUi,
    row: &AgentHudRow,
    tail_labels: &mut BTreeMap<String, gtk::Label>,
) {
    let list_row = gtk::ListBoxRow::new();
    list_row.set_selectable(false);
    list_row.set_activatable(true);
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    container.add_css_class("agent-row");
    if row.current {
        container.add_css_class("current");
    }
    if row.compact {
        container.add_css_class("compact");
    }
    if row.needs_input {
        container.add_css_class("needs-input");
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 5);
    text.add_css_class("agent-row-text");
    text.set_hexpand(true);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.add_css_class("agent-row-top");
    let agent = gtk::Label::builder()
        .label(row.agent_label)
        .xalign(0.0)
        .build();
    agent.add_css_class("agent-name");
    let lifecycle = gtk::Label::builder()
        .label(row.lifecycle_label)
        .xalign(0.5)
        .build();
    lifecycle.add_css_class("agent-lifecycle");
    lifecycle.add_css_class(row.lifecycle_class);
    top.append(&agent);
    top.append(&lifecycle);
    if row.current {
        let current = gtk::Label::builder().label("Current").build();
        current.add_css_class("agent-current");
        current.set_tooltip_text(Some("This agent is in the focused pane"));
        top.append(&current);
    }
    if let Some(permission_mode) = agent_permission_pill_mode(row) {
        let permission = gtk::Label::builder().label(permission_mode).build();
        permission.add_css_class("agent-permission");
        if row.permission_mode_risky {
            permission.add_css_class("risky");
            permission
                .set_tooltip_text(Some(&format!("Permission mode: {permission_mode} (risky)")));
        } else {
            permission.set_tooltip_text(Some(&format!("Permission mode: {permission_mode}")));
        }
        top.append(&permission);
    }
    if row.unread {
        let unread = gtk::Label::builder().label("●").build();
        unread.add_css_class("agent-unread");
        unread.set_tooltip_text(Some("Unseen output since you last viewed this agent"));
        unread.update_property(&[gtk::accessible::Property::Label("Unseen output")]);
        top.append(&unread);
    }

    let title = gtk::Label::builder()
        .label(format!("{} · {}", row.workspace_name, row.surface_title))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("agent-title");

    let meta = gtk::Label::builder()
        .label(agent_meta_line(row))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    meta.add_css_class("agent-meta");

    // Last line of the agent's terminal output, refreshed live by the panel
    // timer; hidden until there is something to show.
    let tail = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .build();
    tail.add_css_class("agent-tail");

    if row.compact {
        title.add_css_class("compact");
        top.append(&title);
        text.append(&top);
    } else {
        tail_labels.insert(row.surface_id.clone(), tail.clone());
        text.append(&top);
        text.append(&title);
        text.append(&meta);
        text.append(&tail);
    }

    // A waiting agent can be answered without leaving the HUD: the entry
    // types the reply (plus Enter) straight into the agent's terminal. The
    // hook-driven lifecycle flip back to Running then removes the entry.
    if row.needs_input && !row.compact && ui.controller.is_some() {
        let reply = gtk::Entry::builder()
            .placeholder_text("Reply and press Enter...")
            .build();
        reply.add_css_class("agent-reply");
        let controller_for_reply = ui.controller.clone();
        let surface_for_reply = row.surface_id.clone();
        reply.connect_activate(move |entry| {
            let text = entry.text();
            let Some(payload) = agent_reply_payload(text.as_str()) else {
                return;
            };
            let sent = controller_for_reply
                .as_ref()
                .and_then(|controller| controller.try_borrow().ok())
                .is_some_and(|controller| {
                    controller.send_text_to_surface(&surface_for_reply, &payload)
                });
            if sent {
                entry.set_text("");
            } else {
                eprintln!("Failed to reply to agent surface {surface_for_reply}: no live pane");
            }
        });
        text.append(&reply);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.add_css_class("agent-actions");
    actions.set_valign(gtk::Align::Center);
    let focus = gtk::Button::with_label("Focus");
    focus.add_css_class("agent-primary-action");
    focus.set_tooltip_text(Some("Focus this agent surface"));
    let resume = gtk::Button::with_label("Resume");
    resume.add_css_class("agent-secondary-action");
    resume.set_sensitive(row.can_resume);
    resume.set_tooltip_text(Some(if row.can_resume {
        "Resume this agent session in a new tab"
    } else {
        "This agent session cannot be resumed safely"
    }));
    let forget = gtk::Button::with_label("Forget");
    forget.add_css_class("agent-danger-action");
    forget.set_tooltip_text(Some(
        "Stop tracking this agent session; terminal and provider data are unchanged",
    ));

    let state_for_focus = ui.state.clone();
    let controller_for_focus = ui.controller.clone();
    let dialog_for_focus = ui.dialog.clone();
    let surface_for_focus = row.surface_id.clone();
    focus.connect_clicked(move |_| {
        if open_agent_surface(
            &state_for_focus,
            &surface_for_focus,
            controller_for_focus.as_ref(),
        ) {
            dialog_for_focus.close();
        }
    });

    let state_for_resume = ui.state.clone();
    let controller_for_resume = ui.controller.clone();
    let dialog_for_resume = ui.dialog.clone();
    let surface_for_resume = row.surface_id.clone();
    resume.connect_clicked(move |button| {
        button.set_sensitive(false);
        resume_agent_from_hud(
            dialog_for_resume.clone(),
            state_for_resume.clone(),
            controller_for_resume.clone(),
            surface_for_resume.clone(),
            button.clone(),
        );
    });

    let state_for_forget = ui.state.clone();
    let surface_for_forget = row.surface_id.clone();
    let toast_overlay_for_forget = ui.toast_overlay.clone();
    let ui_for_forget = ui.clone();
    forget.connect_clicked(move |button| {
        button.set_sensitive(false);
        let Some(forgotten) = forget_agent_surface(&state_for_forget, &surface_for_forget) else {
            create_global_notification(
                &state_for_forget,
                "Forget Agent Failed",
                "The agent session is no longer tracked.",
                NotificationKind::Error,
            );
            button.set_sensitive(true);
            return;
        };
        save_session_from_state(&state_for_forget);
        ui_for_forget.refresh(false);
        show_agent_forget_toast(
            &toast_overlay_for_forget,
            state_for_forget.clone(),
            ui_for_forget.clone(),
            surface_for_forget.clone(),
            forgotten,
        );
    });

    actions.append(&focus);
    actions.append(&resume);
    actions.append(&forget);
    container.append(&text);
    container.append(&actions);
    list_row.set_child(Some(&container));
    list.append(&list_row);
}

fn show_agent_forget_toast(
    toast_overlay: &adw::ToastOverlay,
    state: SocketAppState,
    ui: AgentPanelUi,
    surface_id: String,
    forgotten: forktty_core::ClearedAgentSession,
) {
    let toast = adw::Toast::new("Agent forgotten");
    toast.set_button_label(Some("Undo"));
    toast.connect_button_clicked(move |toast| {
        if restore_forgotten_agent_surface(&state, &surface_id, forgotten.clone()) {
            save_session_from_state(&state);
            ui.refresh(false);
        } else {
            create_global_notification(
                &state,
                "Restore Agent Failed",
                "The agent surface is no longer available.",
                NotificationKind::Error,
            );
        }
        toast.dismiss();
    });
    toast_overlay.add_toast(toast);
}

fn resume_agent_from_hud(
    dialog: gtk::Window,
    state: SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
    surface_id: String,
    resume_button: gtk::Button,
) {
    glib::MainContext::default().spawn_local(async move {
        match dispatch(&state, "agent.resume", json!({ "surface_id": surface_id })).await {
            Ok(_) => {
                save_session_from_state(&state);
                if let Some(controller) = controller {
                    controller.borrow_mut().rebuild_layout();
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
                dialog.close();
            }
            Err(err) => {
                create_global_notification(
                    &state,
                    "Resume Agent Failed",
                    &err.to_string(),
                    NotificationKind::Error,
                );
                resume_button.set_sensitive(true);
            }
        }
    });
}

fn agent_panel_subtitle(total: usize, needs_input: usize) -> String {
    if total == 0 {
        return "No active agent sessions".to_string();
    }
    if needs_input == 0 {
        return format!(
            "{} {} tracked",
            total,
            if total == 1 { "agent" } else { "agents" }
        );
    }
    format!(
        "{} tracked · {} {} input",
        if total == 1 {
            "1 agent".to_string()
        } else {
            format!("{total} agents")
        },
        needs_input,
        if needs_input == 1 { "needs" } else { "need" }
    )
}

pub(super) fn agent_meta_line(row: &AgentHudRow) -> String {
    let mut parts = vec![
        row.cwd_label.clone(),
        format!("session {}", row.session_short),
        row.last_activity_label.clone(),
    ];
    if agent_permission_pill_mode(row).is_none() {
        if let Some(permission_mode) = row.permission_mode.as_deref() {
            parts.push(format!("mode {permission_mode}"));
        }
    }
    parts.join(" · ")
}

pub(super) fn agent_permission_pill_mode(row: &AgentHudRow) -> Option<&str> {
    row.permission_mode
        .as_deref()
        .filter(|_| row.needs_input || row.permission_mode_risky)
}

fn permission_mode_is_risky(permission_mode: &str) -> bool {
    matches!(
        permission_mode,
        "acceptEdits" | "bypassPermissions" | "dangerously-skip-permissions"
    )
}

fn lifecycle_section_label(lifecycle: forktty_core::AgentSessionLifecycle) -> &'static str {
    match lifecycle {
        forktty_core::AgentSessionLifecycle::NeedsInput => "Needs you",
        forktty_core::AgentSessionLifecycle::Running => "Working",
        forktty_core::AgentSessionLifecycle::Idle
        | forktty_core::AgentSessionLifecycle::Suspended
        | forktty_core::AgentSessionLifecycle::Unknown => "Idle",
        forktty_core::AgentSessionLifecycle::Ended => "Done",
    }
}

fn agent_kind_label(agent: forktty_core::AgentKind) -> &'static str {
    match agent {
        forktty_core::AgentKind::ClaudeCode => "Claude",
        forktty_core::AgentKind::Codex => "Codex",
        forktty_core::AgentKind::Antigravity => "Antigravity",
        forktty_core::AgentKind::Pi => "Pi",
        forktty_core::AgentKind::OpenCode => "OpenCode",
        forktty_core::AgentKind::Custom => "Custom",
    }
}

fn lifecycle_presentation(
    lifecycle: forktty_core::AgentSessionLifecycle,
) -> (&'static str, &'static str, u8) {
    match lifecycle {
        forktty_core::AgentSessionLifecycle::NeedsInput => ("Needs input", "needs-input", 0),
        forktty_core::AgentSessionLifecycle::Running => ("Running", "running", 1),
        forktty_core::AgentSessionLifecycle::Idle => ("Idle", "idle", 2),
        forktty_core::AgentSessionLifecycle::Suspended => ("Suspended", "suspended", 3),
        forktty_core::AgentSessionLifecycle::Unknown => ("Unknown", "unknown", 4),
        forktty_core::AgentSessionLifecycle::Ended => ("Ended", "ended", 5),
    }
}

/// The last non-empty line of a terminal text dump, right-trimmed: what the
/// HUD shows as "what is this agent printing right now".
pub(super) fn last_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim_end)
        .find(|line| !line.trim_start().is_empty())
        .map(str::to_string)
}

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

fn last_activity_label(last_activity_ms: u64, now_ms: u64) -> String {
    if last_activity_ms == 0 {
        return "activity unknown".to_string();
    }
    let elapsed = now_ms.saturating_sub(last_activity_ms);
    let seconds = elapsed / 1000;
    if seconds < 60 {
        return "now".to_string();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
