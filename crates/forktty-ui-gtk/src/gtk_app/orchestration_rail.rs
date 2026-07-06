//! Right-side orchestration inspector plus the header Router cluster data
//! (team chips, status summary) for the main GTK workbench.

use super::*;

const RAIL_APPROVAL_SLOTS: usize = 2;
const RAIL_HEALTH_SLOTS: usize = 5;
const RAIL_REPORT_SLOTS: usize = 5;
const RAIL_NOTIFICATION_SLOTS: usize = 4;
const RAIL_EXPANDED_MIN_WIDTH: i32 = 320;
const RAIL_COLLAPSED_WIDTH: i32 = 48;

#[derive(Clone)]
pub(super) struct OrchestrationRailUi {
    pub(super) shell: gtk::Box,
    expanded_header: gtk::Box,
    expanded_body: gtk::ScrolledWindow,
    collapsed_strip: gtk::Button,
    collapsed_approvals_badge: gtk::Label,
    collapsed_notifications_badge: gtk::Label,
    collapse_button: gtk::Button,
    expanded_width: Rc<Cell<i32>>,
    attention_value: gtk::Label,
    status_value: gtk::Label,
    strategy_value: gtk::Label,
    strategy_detail_value: gtk::Label,
    fanout_value: gtk::Label,
    max_loops_value: gtk::Label,
    workflow_value: gtk::Label,
    router_plan_value: gtk::Label,
    router_updated_value: gtk::Label,
    loop_value: gtk::Label,
    loop_caption_value: gtk::Label,
    loop_updated_value: gtk::Label,
    loop_progress: gtk::ProgressBar,
    approvals_value: gtk::Label,
    approval_rows: Vec<RailApprovalRow>,
    approvals_empty: gtk::Label,
    approvals_actions: gtk::Box,
    workers_value: gtk::Label,
    health_rows: Vec<RailListRow>,
    health_overflow: gtk::Label,
    reports_value: gtk::Label,
    report_rows: Vec<RailListRow>,
    reports_overflow: gtk::Label,
    reports_link: gtk::Button,
    notifications_value: gtk::Label,
    notification_rows: Vec<RailListRow>,
    notifications_clear: gtk::Button,
}

/// Team chips shown at the end of the app header bar.
#[derive(Clone)]
pub(super) struct OrchestrationHeaderChipsUi {
    pub(super) shell: gtk::Box,
    last_signature: Rc<RefCell<String>>,
}

#[derive(Clone)]
struct RailApprovalRow {
    shell: gtk::Box,
    title: gtk::Label,
    caption: gtk::Label,
    approval_id: Rc<RefCell<Option<String>>>,
}

#[derive(Clone)]
struct RailListRow {
    shell: gtk::Box,
    dot: gtk::Box,
    primary: gtk::Label,
    secondary: gtk::Label,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct RailSnapshot {
    attention_summary: String,
    attention_attention: bool,
    strategy: String,
    strategy_detail: String,
    status: String,
    fanout: String,
    max_loops: String,
    workflow: String,
    router_plan: String,
    router_updated: String,
    loop_state: String,
    loop_caption: String,
    loop_updated: String,
    loop_fraction: f64,
    loop_planned: bool,
    approvals: String,
    approvals_attention: bool,
    approvals_pending: usize,
    approvals_overflow: usize,
    approval_items: Vec<RailApprovalItem>,
    workers: String,
    health_items: Vec<RailListItem>,
    health_overflow: usize,
    reports: String,
    report_count: usize,
    report_items: Vec<RailListItem>,
    report_overflow: usize,
    notifications: String,
    notifications_attention: bool,
    notifications_unread: usize,
    notification_items: Vec<RailListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RailApprovalItem {
    id: String,
    title: String,
    caption: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RailListItem {
    dot: &'static str,
    primary: String,
    secondary: String,
}

#[derive(Debug, Clone)]
struct LiveSurfaceStatus {
    agent: forktty_core::AgentKind,
    lifecycle: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TeamChip {
    pub(super) label: String,
    pub(super) count: usize,
    pub(super) dot: &'static str,
}

pub(super) fn build_orchestration_rail(state: &SocketAppState) -> OrchestrationRailUi {
    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.add_css_class("orchestration-rail");
    shell.set_width_request(RAIL_EXPANDED_MIN_WIDTH);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("orchestration-panel-header");
    let title = gtk::Label::builder()
        .label("Router")
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    title.add_css_class("orchestration-panel-title");
    title.set_hexpand(true);
    let status_value = gtk::Label::builder()
        .label("")
        .single_line_mode(true)
        .build();
    status_value.add_css_class("orchestration-status-chip");
    let collapse_button = gtk::Button::builder()
        .icon_name("forktty-chevron-right-symbolic")
        .has_frame(false)
        .tooltip_text("Collapse Router rail")
        .build();
    collapse_button.add_css_class("flat");
    collapse_button.add_css_class("orchestration-collapse-button");
    set_accessible_button_text(&collapse_button, "Collapse Router rail", None);
    header.append(&title);
    header.append(&status_value);
    header.append(&collapse_button);
    // Header stays outside the scroller so the collapse control never scrolls away.
    shell.append(&header);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    scroller.add_css_class("orchestration-rail-scroll");
    scroller.set_vexpand(true);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("orchestration-rail-body");
    body.set_vexpand(true);
    scroller.set_child(Some(&body));

    let attention_section = rail_section(&body);
    attention_section.add_css_class("orchestration-attention-section");
    let attention_value = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    attention_value.add_css_class("orchestration-attention-summary");
    attention_section.append(&attention_value);

    let strategy_section = rail_section(&body);
    let strategy_header = rail_section_header(&strategy_section, "STRATEGY");
    let edit = rail_outline_button("Edit", "Open Router planner", "app.task-router");
    strategy_header.append(&edit);
    let strategy_value = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    strategy_value.add_css_class("orchestration-strategy-value");
    strategy_section.append(&strategy_value);
    let strategy_detail_value = multiline_value(2);
    strategy_detail_value.add_css_class("orchestration-strategy-detail");
    strategy_section.append(&strategy_detail_value);
    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    meta.add_css_class("orchestration-meta-grid");
    let fanout_value = rail_meta(&meta, "Fan-out");
    let max_loops_value = rail_meta(&meta, "Max loops");
    let workflow_value = rail_meta(&meta, "Workflow");
    strategy_section.append(&meta);

    let decision_section = rail_section(&body);
    rail_section_header(&decision_section, "ROUTER DECISION");
    let router_plan_value = rail_inline_value(&decision_section, "Plan");
    let router_updated_value = rail_inline_value(&decision_section, "Updated");

    let loop_section = rail_section(&body);
    rail_section_header(&loop_section, "LOOP STATE");
    let loop_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let loop_value = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    loop_value.add_css_class("orchestration-loop-value");
    let loop_progress = gtk::ProgressBar::new();
    loop_progress.add_css_class("orchestration-loop-progress");
    loop_progress.set_hexpand(true);
    loop_progress.set_valign(gtk::Align::Center);
    loop_row.append(&loop_value);
    loop_row.append(&loop_progress);
    loop_section.append(&loop_row);
    let loop_caption_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let loop_caption_value = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .hexpand(true)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    loop_caption_value.add_css_class("orchestration-muted-caption");
    let loop_updated_value = gtk::Label::builder()
        .label("")
        .xalign(1.0)
        .single_line_mode(true)
        .build();
    loop_updated_value.add_css_class("orchestration-muted-caption");
    loop_caption_row.append(&loop_caption_value);
    loop_caption_row.append(&loop_updated_value);
    loop_section.append(&loop_caption_row);

    let approvals_section = rail_section(&body);
    let approvals_header = rail_section_header(&approvals_section, "APPROVALS");
    let approvals_value = rail_count_label();
    approvals_header.append(&approvals_value);
    let approval_rows = (0..RAIL_APPROVAL_SLOTS)
        .map(|_| build_approval_row(&approvals_section, state))
        .collect::<Vec<_>>();
    let approvals_empty = gtk::Label::builder()
        .label("No pending approvals")
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    approvals_empty.add_css_class("orchestration-muted-caption");
    approvals_section.append(&approvals_empty);
    let auto_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    auto_row.add_css_class("orchestration-section-actions");
    let auto_label = gtk::Label::builder()
        .label("Auto-approve: Off")
        .xalign(0.0)
        .hexpand(true)
        .single_line_mode(true)
        .tooltip_text("ForkTTY never auto-approves; decisions stay explicit")
        .build();
    auto_label.add_css_class("orchestration-muted-caption");
    auto_row.append(&auto_label);
    auto_row.append(&rail_outline_button(
        "Settings",
        "Open Settings",
        "app.settings",
    ));
    approvals_section.append(&auto_row);

    let workers_section = rail_section(&body);
    let workers_header = rail_section_header(&workers_section, "WORKER HEALTH");
    let workers_value = rail_count_label();
    workers_header.append(&workers_value);
    let health_rows = (0..RAIL_HEALTH_SLOTS)
        .map(|_| build_list_row(&workers_section))
        .collect::<Vec<_>>();
    let health_overflow = rail_overflow_label(&workers_section);
    workers_section.append(&rail_link_row("View all workers", "app.agents"));

    let reports_section = rail_section(&body);
    let reports_header = rail_section_header(&reports_section, "WORKER REPORTS");
    let reports_value = rail_count_label();
    reports_header.append(&reports_value);
    let report_rows = (0..RAIL_REPORT_SLOTS)
        .map(|_| build_list_row(&reports_section))
        .collect::<Vec<_>>();
    let reports_overflow = rail_overflow_label(&reports_section);
    let reports_link = rail_link_row("Open worker reports", "app.agents");
    reports_section.append(&reports_link);

    let notifications_section = rail_section(&body);
    let notifications_header = rail_section_header(&notifications_section, "NOTIFICATIONS");
    let notifications_value = rail_count_label();
    notifications_header.append(&notifications_value);
    let notification_rows = (0..RAIL_NOTIFICATION_SLOTS)
        .map(|_| build_list_row(&notifications_section))
        .collect::<Vec<_>>();
    let clear_all = gtk::Button::builder()
        .label("Clear all")
        .has_frame(false)
        .tooltip_text("Clear all notifications")
        .build();
    clear_all.add_css_class("flat");
    clear_all.add_css_class("orchestration-link-button");
    clear_all.set_halign(gtk::Align::Start);
    set_accessible_button_text(&clear_all, "Clear all notifications", None);
    let state_for_clear = state.clone();
    clear_all.connect_clicked(move |_| {
        let notifications = state_for_clear
            .model
            .lock()
            .map(|mut model| {
                let workspace_id = model.active_workspace_id();
                clear_rail_notifications_from_model(&mut model, workspace_id.as_deref())
            })
            .unwrap_or_default();
        state_for_clear.mark_notification_feed_entries_cleared(&notifications);
        for notification in &notifications {
            close_desktop_notification(&notification.id);
        }
        super::notifications_panel::send_terminal_notification_close_reports_via_backend(
            state_for_clear.terminal.clone(),
            notifications,
        );
    });
    notifications_section.append(&clear_all);

    shell.append(&scroller);

    let strip_content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    strip_content.add_css_class("orchestration-rail-strip-content");
    strip_content.set_valign(gtk::Align::Start);
    strip_content.set_halign(gtk::Align::Center);
    let strip_icon = gtk::Image::from_icon_name("forktty-chevron-left-symbolic");
    strip_content.append(&strip_icon);
    let collapsed_approvals_badge = rail_strip_badge("warn");
    strip_content.append(&collapsed_approvals_badge);
    let collapsed_notifications_badge = rail_strip_badge("info");
    strip_content.append(&collapsed_notifications_badge);
    let collapsed_strip = gtk::Button::builder()
        .child(&strip_content)
        .has_frame(false)
        .tooltip_text("Expand Router rail")
        .build();
    collapsed_strip.add_css_class("flat");
    collapsed_strip.add_css_class("orchestration-rail-collapsed-strip");
    collapsed_strip.set_vexpand(true);
    collapsed_strip.set_valign(gtk::Align::Fill);
    set_accessible_button_text(&collapsed_strip, "Expand Router rail", None);
    shell.append(&collapsed_strip);

    let ui = OrchestrationRailUi {
        shell,
        expanded_header: header,
        expanded_body: scroller,
        collapsed_strip,
        collapsed_approvals_badge,
        collapsed_notifications_badge,
        collapse_button,
        expanded_width: Rc::new(Cell::new(RAIL_EXPANDED_MIN_WIDTH)),
        attention_value,
        status_value,
        strategy_value,
        strategy_detail_value,
        fanout_value,
        max_loops_value,
        workflow_value,
        router_plan_value,
        router_updated_value,
        loop_value,
        loop_caption_value,
        loop_updated_value,
        loop_progress,
        approvals_value,
        approval_rows,
        approvals_empty,
        approvals_actions: auto_row,
        workers_value,
        health_rows,
        health_overflow,
        reports_value,
        report_rows,
        reports_overflow,
        reports_link,
        notifications_value,
        notification_rows,
        notifications_clear: clear_all,
    };
    set_orchestration_rail_collapsed(&ui, false);
    let ui_for_collapse = ui.clone();
    ui.collapse_button.connect_clicked(move |_| {
        set_orchestration_rail_collapsed(&ui_for_collapse, true);
    });
    let ui_for_expand = ui.clone();
    ui.collapsed_strip.connect_clicked(move |_| {
        set_orchestration_rail_collapsed(&ui_for_expand, false);
    });
    refresh_orchestration_rail(&ui, state);
    ui
}

fn set_orchestration_rail_collapsed(ui: &OrchestrationRailUi, collapsed: bool) {
    ui.expanded_header.set_visible(!collapsed);
    ui.expanded_body.set_visible(!collapsed);
    ui.collapsed_strip.set_visible(collapsed);
    ui.shell.set_width_request(if collapsed {
        RAIL_COLLAPSED_WIDTH
    } else {
        RAIL_EXPANDED_MIN_WIDTH
    });
    if collapsed {
        ui.shell.add_css_class("collapsed");
    } else {
        ui.shell.remove_css_class("collapsed");
    }
    // width_request is only a minimum: once the user drags the surrounding
    // paned's divider its position sticks, so move it explicitly or the rail
    // keeps its old width and collapses into an empty column.
    let paned = ui
        .shell
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Paned>().ok());
    if let Some(paned) = paned.filter(|paned| paned.width() > 0) {
        if collapsed {
            ui.expanded_width
                .set(ui.shell.width().max(RAIL_EXPANDED_MIN_WIDTH));
            // Clamped by the rail's minimum width, so this pins it collapsed.
            paned.set_position(paned.width());
        } else {
            paned.set_position(paned.width() - ui.expanded_width.get());
        }
    }
}

fn rail_strip_badge(kind: &'static str) -> gtk::Label {
    let badge = gtk::Label::builder()
        .label("")
        .single_line_mode(true)
        .visible(false)
        .build();
    badge.add_css_class("orchestration-rail-strip-badge");
    badge.add_css_class(kind);
    badge
}

fn set_rail_strip_badge(badge: &gtk::Label, count: usize, tooltip: &str) {
    badge.set_visible(count > 0);
    if count > 0 {
        badge.set_label(&count.to_string());
        badge.set_tooltip_text(Some(tooltip));
    }
}

fn build_approval_row(parent: &gtk::Box, state: &SocketAppState) -> RailApprovalRow {
    let shell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    shell.add_css_class("orchestration-approval-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    let title = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("orchestration-approval-title");
    let caption = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    caption.add_css_class("orchestration-muted-caption");
    text.append(&title);
    text.append(&caption);
    shell.append(&text);

    let approval_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let review = rail_outline_button("Review", "Open approval notifications", "app.notifications");
    shell.append(&review);
    let deny = gtk::Button::builder()
        .label("Deny")
        .has_frame(false)
        .tooltip_text("Deny this approval request")
        .build();
    deny.add_css_class("flat");
    deny.add_css_class("orchestration-outline-button");
    deny.add_css_class("orchestration-deny-button");
    set_accessible_button_text(&deny, "Deny this approval request", None);
    let state_for_deny = state.clone();
    let approval_id_for_deny = approval_id.clone();
    let shell_for_deny = shell.clone();
    deny.connect_clicked(move |_| {
        let Some(id) = approval_id_for_deny.borrow().clone() else {
            return;
        };
        if !rail_approval_id_is_currently_visible(&state_for_deny, &id) {
            shell_for_deny.set_visible(false);
            return;
        }
        // The 1s orchestration refresh re-syncs the slots; hide eagerly so the
        // row cannot be denied twice while the next tick is pending.
        if state_for_deny
            .decide_feed_approval(&id, forktty_core::FeedApprovalState::Denied)
            .is_ok()
        {
            shell_for_deny.set_visible(false);
        }
    });
    shell.append(&deny);
    shell.set_visible(false);
    parent.append(&shell);
    RailApprovalRow {
        shell,
        title,
        caption,
        approval_id,
    }
}

fn build_list_row(parent: &gtk::Box) -> RailListRow {
    let shell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    shell.add_css_class("orchestration-list-row");
    let dot = rail_dot();
    let primary = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .hexpand(true)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    primary.add_css_class("orchestration-list-primary");
    let secondary = gtk::Label::builder()
        .label("")
        .xalign(1.0)
        .single_line_mode(true)
        .build();
    secondary.add_css_class("orchestration-list-secondary");
    shell.append(&dot);
    shell.append(&primary);
    shell.append(&secondary);
    shell.set_visible(false);
    parent.append(&shell);
    RailListRow {
        shell,
        dot,
        primary,
        secondary,
    }
}

fn rail_dot() -> gtk::Box {
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("rail-dot");
    dot.set_valign(gtk::Align::Center);
    dot
}

fn rail_link_row(label: &str, action: &str) -> gtk::Button {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let text = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .single_line_mode(true)
        .build();
    let chevron = gtk::Label::builder().label("\u{203a}").build();
    row.append(&text);
    row.append(&chevron);
    let button = gtk::Button::builder().child(&row).has_frame(false).build();
    button.add_css_class("flat");
    button.add_css_class("orchestration-link-button");
    button.set_action_name(Some(action));
    button.set_tooltip_text(Some(label));
    set_accessible_button_text(&button, label, None);
    button
}

fn rail_overflow_label(parent: &gtk::Box) -> gtk::Label {
    let label = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .visible(false)
        .build();
    label.add_css_class("orchestration-muted-caption");
    parent.append(&label);
    label
}

fn set_list_rows(rows: &[RailListRow], items: &[RailListItem], empty: &str) {
    for (index, row) in rows.iter().enumerate() {
        if let Some(item) = items.get(index) {
            row.shell.set_visible(true);
            row.dot.set_visible(true);
            set_dot_class(&row.dot, item.dot);
            set_rail_value(&row.primary, &item.primary);
            set_rail_value(&row.secondary, &item.secondary);
            for class_name in ["ok", "warn", "err", "info", "idle"] {
                row.secondary.remove_css_class(class_name);
            }
            row.secondary.add_css_class(item.dot);
        } else if index == 0 && items.is_empty() {
            row.shell.set_visible(true);
            row.dot.set_visible(false);
            set_rail_value(&row.primary, empty);
            set_rail_value(&row.secondary, "");
        } else {
            row.shell.set_visible(false);
        }
    }
}

fn set_overflow_label(label: &gtk::Label, count: usize, singular: &str, plural: &str) {
    if count == 0 {
        label.set_visible(false);
        return;
    }
    set_rail_value(label, &format!("+{}", count_label(count, singular, plural)));
    label.set_visible(true);
}

fn set_dot_class(dot: &gtk::Box, class_name: &'static str) {
    for candidate in ["ok", "warn", "err", "info", "idle"] {
        dot.remove_css_class(candidate);
    }
    dot.add_css_class(class_name);
}

pub(super) fn refresh_orchestration_rail(ui: &OrchestrationRailUi, state: &SocketAppState) {
    let snapshot = orchestration_rail_snapshot(state);
    ui.collapsed_strip
        .set_tooltip_text(Some(&format!("Expand Router rail: {}", snapshot.status)));
    set_rail_strip_badge(
        &ui.collapsed_approvals_badge,
        snapshot.approvals_pending,
        "Pending approvals",
    );
    set_rail_strip_badge(
        &ui.collapsed_notifications_badge,
        snapshot.notifications_unread,
        "Unread notifications",
    );
    set_rail_value(&ui.attention_value, &snapshot.attention_summary);
    set_attention_summary_class(&ui.attention_value, snapshot.attention_attention);
    set_rail_value(&ui.status_value, &snapshot.status);
    set_status_chip_class(&ui.status_value, &snapshot.status);
    set_rail_value(&ui.strategy_value, &snapshot.strategy);
    set_rail_value(&ui.strategy_detail_value, &snapshot.strategy_detail);
    set_rail_value(&ui.fanout_value, &snapshot.fanout);
    set_rail_value(&ui.max_loops_value, &snapshot.max_loops);
    set_rail_value(&ui.workflow_value, &snapshot.workflow);
    set_rail_value(&ui.router_plan_value, &snapshot.router_plan);
    set_rail_value(&ui.router_updated_value, &snapshot.router_updated);
    set_rail_value(&ui.loop_value, &snapshot.loop_state);
    set_rail_value(&ui.loop_caption_value, &snapshot.loop_caption);
    set_rail_value(&ui.loop_updated_value, &snapshot.loop_updated);
    ui.loop_progress
        .set_fraction(snapshot.loop_fraction.clamp(0.0, 1.0));
    ui.loop_progress.set_visible(snapshot.loop_planned);
    set_rail_value(&ui.approvals_value, &snapshot.approvals);
    set_count_attention(&ui.approvals_value, snapshot.approvals_attention);
    for (index, row) in ui.approval_rows.iter().enumerate() {
        if let Some(item) = snapshot.approval_items.get(index) {
            row.shell.set_visible(true);
            set_rail_value(&row.title, &item.title);
            set_rail_value(&row.caption, &item.caption);
            *row.approval_id.borrow_mut() = Some(item.id.clone());
        } else {
            row.shell.set_visible(false);
            *row.approval_id.borrow_mut() = None;
        }
    }
    if snapshot.approval_items.is_empty() {
        set_rail_value(&ui.approvals_empty, "No pending approvals");
        ui.approvals_empty.set_visible(true);
    } else if snapshot.approvals_overflow > 0 {
        set_rail_value(
            &ui.approvals_empty,
            &format!("+{} more pending", snapshot.approvals_overflow),
        );
        ui.approvals_empty.set_visible(true);
    } else {
        ui.approvals_empty.set_visible(false);
    }
    ui.approvals_actions
        .set_visible(approvals_settings_visible(&snapshot));
    set_rail_value(&ui.workers_value, &snapshot.workers);
    set_list_rows(
        &ui.health_rows,
        &snapshot.health_items,
        "No visible workers",
    );
    set_overflow_label(
        &ui.health_overflow,
        snapshot.health_overflow,
        "more worker group",
        "more worker groups",
    );
    set_rail_value(&ui.reports_value, &snapshot.reports);
    set_list_rows(&ui.report_rows, &snapshot.report_items, "No worker reports");
    set_overflow_label(
        &ui.reports_overflow,
        snapshot.report_overflow,
        "more report",
        "more reports",
    );
    ui.reports_link.set_visible(reports_link_visible(&snapshot));
    set_rail_value(&ui.notifications_value, &snapshot.notifications);
    set_count_attention(&ui.notifications_value, snapshot.notifications_attention);
    set_list_rows(
        &ui.notification_rows,
        &snapshot.notification_items,
        "No notifications",
    );
    ui.notifications_clear
        .set_visible(!snapshot.notification_items.is_empty());
}

fn set_count_attention(label: &gtk::Label, attention: bool) {
    if attention {
        label.add_css_class("attention");
    } else {
        label.remove_css_class("attention");
    }
}

fn set_attention_summary_class(label: &gtk::Label, attention: bool) {
    if attention {
        label.add_css_class("attention");
    } else {
        label.remove_css_class("attention");
    }
}

fn approvals_settings_visible(snapshot: &RailSnapshot) -> bool {
    !snapshot.approval_items.is_empty()
}

fn reports_link_visible(snapshot: &RailSnapshot) -> bool {
    snapshot.report_count > 0
}

fn set_status_chip_class(label: &gtk::Label, status: &str) {
    for class_name in ["ok", "run", "warn", "err", "info", "idle"] {
        label.remove_css_class(class_name);
    }
    label.add_css_class(feed_status_class(status));
}

pub(super) fn build_orchestration_header_chips(
    state: &SocketAppState,
) -> OrchestrationHeaderChipsUi {
    let shell = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    shell.add_css_class("header-team-chips");
    let ui = OrchestrationHeaderChipsUi {
        shell,
        last_signature: Rc::new(RefCell::new(String::new())),
    };
    refresh_orchestration_header_chips(&ui, state);
    ui
}

pub(super) fn refresh_orchestration_header_chips(
    ui: &OrchestrationHeaderChipsUi,
    state: &SocketAppState,
) {
    let chips = latest_team_chips_for_state(state);
    let signature = chips
        .iter()
        .map(|chip| format!("{}:{}:{}", chip.label, chip.count, chip.dot))
        .collect::<Vec<_>>()
        .join("|");
    if *ui.last_signature.borrow() == signature {
        return;
    }
    *ui.last_signature.borrow_mut() = signature;
    while let Some(child) = ui.shell.first_child() {
        ui.shell.remove(&child);
    }
    ui.shell.set_visible(!chips.is_empty());
    for chip in &chips {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let dot = rail_dot();
        dot.add_css_class(chip.dot);
        let name = gtk::Label::builder()
            .label(&chip.label)
            .single_line_mode(true)
            .build();
        name.add_css_class("header-team-chip-name");
        let count = gtk::Label::builder()
            .label(chip.count.to_string())
            .single_line_mode(true)
            .build();
        count.add_css_class("header-team-chip-count");
        content.append(&dot);
        content.append(&name);
        content.append(&count);
        let button = gtk::Button::builder()
            .child(&content)
            .has_frame(false)
            .build();
        button.add_css_class("flat");
        button.add_css_class("header-team-chip");
        button.set_action_name(Some("app.agents"));
        let tooltip = format!("{} workers: {}", chip.label, chip.count);
        button.set_tooltip_text(Some(&tooltip));
        set_accessible_button_text(&button, &tooltip, None);
        ui.shell.append(&button);
    }
}

/// Compact Router summary for the app status bar, e.g.
/// "Router: parallel_research  Loop 2/5".
pub(super) fn orchestration_status_summary(state: &SocketAppState) -> String {
    let active_workspace_id = active_workspace_id_for_state(state);
    let workflow = latest_workflow_snapshot_for_workspace(
        state.workflow_store_path.as_deref(),
        active_workspace_id.as_deref(),
    );
    match (workflow.loop_iteration_display, workflow.strategy.as_str()) {
        (Some(loop_display), strategy) => format!("Router: {strategy}  {loop_display}"),
        (None, strategy) => format!("Router: {strategy}"),
    }
}

fn rail_section(parent: &gtk::Box) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("orchestration-section");
    parent.append(&section);
    section
}

fn rail_section_header(parent: &gtk::Box, title: &str) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("orchestration-section-header");
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    label.add_css_class("orchestration-section-title");
    label.set_hexpand(true);
    header.append(&label);
    parent.append(&header);
    header
}

fn rail_outline_button(label: &str, tooltip: &str, action: &str) -> gtk::Button {
    let button = gtk::Button::builder().label(label).has_frame(false).build();
    button.add_css_class("flat");
    button.add_css_class("orchestration-outline-button");
    button.set_tooltip_text(Some(tooltip));
    button.set_action_name(Some(action));
    set_accessible_button_text(&button, tooltip, None);
    button
}

fn rail_count_label() -> gtk::Label {
    let label = gtk::Label::builder()
        .label("")
        .single_line_mode(true)
        .build();
    label.add_css_class("orchestration-section-count");
    label
}

fn rail_meta(parent: &gtk::Box, label: &str) -> gtk::Label {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
    box_.add_css_class("orchestration-meta");
    box_.set_hexpand(true);
    let key = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    key.add_css_class("orchestration-meta-key");
    let value = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    value.add_css_class("orchestration-meta-value");
    box_.append(&key);
    box_.append(&value);
    parent.append(&box_);
    value
}

fn rail_inline_value(parent: &gtk::Box, label: &str) -> gtk::Label {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("orchestration-inline-row");
    let key = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    key.add_css_class("orchestration-inline-key");
    let value = gtk::Label::builder()
        .label("")
        .xalign(1.0)
        .hexpand(true)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    value.add_css_class("orchestration-inline-value");
    row.append(&key);
    row.append(&value);
    parent.append(&row);
    value
}

fn multiline_value(lines: i32) -> gtk::Label {
    let value = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .wrap(true)
        .lines(lines)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    value.add_css_class("orchestration-list-value");
    value
}

fn set_rail_value(label: &gtk::Label, value: &str) {
    if label.label() != value {
        label.set_label(value);
        label.set_tooltip_text((!value.is_empty()).then_some(value));
    }
}

fn orchestration_rail_snapshot(state: &SocketAppState) -> RailSnapshot {
    let now_ms = now_unix_ms();
    let active_workspace_id = active_workspace_id_for_state(state);
    let live_statuses = live_surface_statuses(state);
    let (notifications, notifications_unread, notification_items) = state
        .model
        .lock()
        .ok()
        .map(|model| {
            let notifications = current_rail_notifications(&model, active_workspace_id.as_deref());
            let unread = notifications
                .iter()
                .filter(|notification| !notification.read)
                .count();
            (
                format_notification_counts(&notifications),
                unread,
                notification_list_items(&notifications, now_ms),
            )
        })
        .unwrap_or_else(|| ("unavailable".to_string(), 0, Vec::new()));
    let notifications_attention = notifications_unread > 0;
    let visible_approvals = current_pending_feed_approvals(state, active_workspace_id.as_deref());
    let approval_count = visible_approvals.as_ref().map(Vec::len);
    let approval_items = visible_approvals
        .as_ref()
        .map(|approvals| {
            approvals
                .iter()
                .take(RAIL_APPROVAL_SLOTS)
                .map(|approval| RailApprovalItem {
                    caption: format!(
                        "Requested {}",
                        relative_time_label(now_ms, approval.created_at_ms)
                    ),
                    id: approval.id.clone(),
                    title: approval.title.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let approvals = approval_count
        .map(|count| format!("{count} pending"))
        .unwrap_or_else(|| "unavailable".to_string());
    let approvals_attention = approval_count.is_some_and(|count| count > 0);
    let approvals_overflow =
        approval_count.map_or(0, |count| count.saturating_sub(approval_items.len()));
    let workflow = latest_workflow_snapshot_for_workspace(
        state.workflow_store_path.as_deref(),
        active_workspace_id.as_deref(),
    );
    let team = latest_team_snapshot_for_workspace(
        state.team_store_path.as_deref(),
        active_workspace_id.as_deref(),
        &live_statuses,
    );
    let approvals_pending = approval_count.unwrap_or(0);
    let attention_summary =
        format_attention_summary(approvals_pending, notifications_unread, &team);
    let attention_attention = attention_summary != "All clear";
    RailSnapshot {
        attention_summary,
        attention_attention,
        strategy: workflow.strategy,
        strategy_detail: workflow.strategy_detail,
        status: workflow.status,
        fanout: team.fanout,
        max_loops: workflow.max_loops,
        workflow: workflow.workflow,
        router_plan: workflow.router_plan,
        router_updated: workflow.router_updated,
        loop_state: workflow.loop_state,
        loop_caption: workflow.loop_caption,
        loop_updated: workflow.loop_updated,
        loop_fraction: workflow.loop_fraction,
        loop_planned: workflow.loop_planned,
        approvals,
        approvals_attention,
        approvals_pending,
        approvals_overflow,
        approval_items,
        workers: team.workers,
        health_items: team.health_items,
        health_overflow: team.health_overflow,
        reports: team.reports,
        report_count: team.report_count,
        report_items: team.report_items,
        report_overflow: team.report_overflow,
        notifications,
        notifications_attention,
        notifications_unread,
        notification_items,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct WorkflowRailSnapshot {
    strategy: String,
    strategy_detail: String,
    status: String,
    max_loops: String,
    workflow: String,
    router_plan: String,
    router_updated: String,
    loop_state: String,
    loop_caption: String,
    loop_updated: String,
    loop_fraction: f64,
    loop_planned: bool,
    loop_iteration_display: Option<String>,
}

fn latest_workflow_snapshot_for_workspace(
    path: Option<&Path>,
    workspace_id: Option<&str>,
) -> WorkflowRailSnapshot {
    let Some(path) = path else {
        return workflow_fallback("no store", "Workflow store not configured");
    };
    let Ok(store) = forktty_core::load_workflows_from_path(path) else {
        return workflow_fallback("unavailable", "Workflow store unavailable");
    };
    let Some(workflow) = store
        .workflows
        .iter()
        .filter(|workflow| record_matches_workspace(workflow.workspace_id.as_deref(), workspace_id))
        .max_by_key(|workflow| {
            (
                !workflow_is_closed(&workflow.status),
                workflow.updated_at_ms,
            )
        })
    else {
        return workflow_fallback("idle", "No workflow staged");
    };
    let now_ms = now_unix_ms();
    let strategy = workflow
        .loop_recipe
        .as_deref()
        .unwrap_or(workflow.mode.as_str())
        .to_string();
    let status = workflow
        .loop_stage
        .as_deref()
        .map(|stage| format!("loop {stage}"))
        .unwrap_or_else(|| workflow.status.clone());
    let max_loops = workflow
        .loop_max_iterations
        .map(|max| max.to_string())
        .unwrap_or_else(|| "-".to_string());
    let loop_iteration_display = loop_iteration_display(workflow);
    WorkflowRailSnapshot {
        strategy: strategy.clone(),
        strategy_detail: workflow
            .goal
            .as_deref()
            .filter(|goal| !goal.trim().is_empty())
            .unwrap_or("No routing goal recorded")
            .to_string(),
        status,
        max_loops,
        workflow: workflow.status.clone(),
        router_plan: strategy,
        router_updated: relative_time_label(now_ms, u128::from(workflow.updated_at_ms)),
        loop_state: format_loop_state(workflow),
        loop_caption: format_loop_caption(workflow),
        loop_updated: workflow
            .loop_updated_at_ms
            .map(|at_ms| relative_time_label(now_ms, u128::from(at_ms)))
            .unwrap_or_default(),
        loop_fraction: loop_fraction(workflow),
        loop_planned: workflow_has_loop_state(workflow),
        loop_iteration_display,
    }
}

fn workflow_fallback(strategy: &str, detail: &str) -> WorkflowRailSnapshot {
    WorkflowRailSnapshot {
        strategy: strategy.to_string(),
        strategy_detail: detail.to_string(),
        status: "idle".to_string(),
        max_loops: "-".to_string(),
        workflow: "none".to_string(),
        router_plan: "No plan selected".to_string(),
        router_updated: "-".to_string(),
        loop_state: "not planned".to_string(),
        loop_caption: "No loop metadata".to_string(),
        loop_updated: String::new(),
        loop_fraction: 0.0,
        loop_planned: false,
        loop_iteration_display: None,
    }
}

fn workflow_is_closed(status: &str) -> bool {
    matches!(status, "done" | "closed" | "cancelled" | "finished")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TeamRailSnapshot {
    fanout: String,
    workers: String,
    active_workers: usize,
    attention_groups: usize,
    error_groups: usize,
    health_items: Vec<RailListItem>,
    health_overflow: usize,
    reports: String,
    report_count: usize,
    report_items: Vec<RailListItem>,
    report_overflow: usize,
}

#[cfg(test)]
impl TeamRailSnapshot {
    fn from_workers_for_test(workers: &[forktty_core::TeamWorker]) -> Self {
        let live_statuses = std::collections::HashMap::new();
        let report_items = report_list_items(workers);
        let report_count = workers
            .iter()
            .filter(|worker| worker_has_report(worker))
            .count();
        let report_overflow = report_count.saturating_sub(report_items.len());
        let status_counts = team_status_counts(workers, &live_statuses);
        Self {
            fanout: workers.len().to_string(),
            workers: format!("{}/{} active", status_counts.active_workers, workers.len()),
            active_workers: status_counts.active_workers,
            attention_groups: status_counts.attention_groups,
            error_groups: status_counts.error_groups,
            health_items: health_list_items_with_live_statuses(workers, &live_statuses),
            health_overflow: health_overflow_with_live_statuses(workers, &live_statuses),
            reports: count_label(report_count, "report", "reports"),
            report_count,
            report_items,
            report_overflow,
        }
    }
}

fn latest_team_snapshot_for_workspace(
    path: Option<&Path>,
    workspace_id: Option<&str>,
    live_statuses: &std::collections::HashMap<String, LiveSurfaceStatus>,
) -> TeamRailSnapshot {
    let Some(path) = path else {
        return team_fallback("no store");
    };
    let Ok(store) = forktty_core::load_teams_from_path(path) else {
        return team_fallback("unavailable");
    };
    let Some(team) = store
        .teams
        .iter()
        .filter(|team| record_matches_workspace(team.workspace_id.as_deref(), workspace_id))
        .max_by_key(|team| (!team_is_closed(&team.status), team.updated_at_ms))
    else {
        return team_fallback("no team");
    };
    let active = team
        .workers
        .iter()
        .filter(|worker| worker_status_is_active(effective_worker_status(worker, live_statuses)))
        .count();
    let status_counts = if team_is_closed(&team.status) {
        TeamStatusCounts::default()
    } else {
        team_status_counts(&team.workers, live_statuses)
    };
    let workers = if team_is_closed(&team.status) {
        "finished".to_string()
    } else {
        format!("{active}/{} active", team.workers.len())
    };
    let report_items = report_list_items(&team.workers);
    let report_count = team
        .workers
        .iter()
        .filter(|worker| worker_has_report(worker))
        .count();
    TeamRailSnapshot {
        fanout: team.workers.len().to_string(),
        workers,
        active_workers: status_counts.active_workers,
        attention_groups: status_counts.attention_groups,
        error_groups: status_counts.error_groups,
        health_items: health_list_items_with_live_statuses(&team.workers, live_statuses),
        health_overflow: health_overflow_with_live_statuses(&team.workers, live_statuses),
        reports: count_label(report_count, "report", "reports"),
        report_count,
        report_overflow: report_count.saturating_sub(report_items.len()),
        report_items,
    }
}

fn team_fallback(value: &str) -> TeamRailSnapshot {
    TeamRailSnapshot {
        fanout: "0".to_string(),
        workers: value.to_string(),
        active_workers: 0,
        attention_groups: 0,
        error_groups: 0,
        health_items: Vec::new(),
        health_overflow: 0,
        reports: "0 reports".to_string(),
        report_count: 0,
        report_items: Vec::new(),
        report_overflow: 0,
    }
}

#[derive(Debug, Clone, Default)]
struct TeamStatusCounts {
    active_workers: usize,
    attention_groups: usize,
    error_groups: usize,
}

fn team_status_counts(
    workers: &[forktty_core::TeamWorker],
    live_statuses: &std::collections::HashMap<String, LiveSurfaceStatus>,
) -> TeamStatusCounts {
    let chips = team_chips_from_workers_with_live_statuses(workers, live_statuses);
    TeamStatusCounts {
        active_workers: workers
            .iter()
            .filter(|worker| {
                worker_status_is_active(effective_worker_status(worker, live_statuses))
            })
            .count(),
        attention_groups: chips
            .iter()
            .filter(|chip| matches!(chip.dot, "warn" | "err"))
            .count(),
        error_groups: chips.iter().filter(|chip| chip.dot == "err").count(),
    }
}

fn format_attention_summary(
    approvals_pending: usize,
    notifications_unread: usize,
    team: &TeamRailSnapshot,
) -> String {
    let attention_count = approvals_pending + notifications_unread + team.attention_groups;
    if attention_count == 0 {
        return "All clear".to_string();
    }
    format!(
        "Attention {attention_count} · Running {} · Reports {} · Errors {}",
        team.active_workers, team.report_count, team.error_groups
    )
}

fn worker_status_is_active(status: &str) -> bool {
    status == "running" || status == "active"
}

/// Mirrors `forktty_core::team`'s closed statuses so finished teams stop
/// looking live in chips and health rows.
fn team_is_closed(status: &str) -> bool {
    matches!(status, "done" | "closed" | "cancelled" | "finished")
}

fn worker_status_dot(status: &str) -> &'static str {
    let status = status.to_ascii_lowercase();
    if status.contains("fail") || status.contains("error") || status.contains("no_heartbeat") {
        "err"
    } else if status == "running" || status == "active" || status == "done" || status == "ready" {
        "ok"
    } else if status.contains("shutdown")
        || status.contains("stopped")
        || status.contains("ended")
        || status.contains("hibernat")
    {
        "idle"
    } else {
        "warn"
    }
}

/// Groups a team's workers by agent for the header chips and the sidebar TEAM
/// section, preserving first-seen order.
#[cfg(test)]
pub(super) fn team_chips_from_workers(workers: &[forktty_core::TeamWorker]) -> Vec<TeamChip> {
    let live_statuses = std::collections::HashMap::new();
    team_chips_from_workers_with_live_statuses(workers, &live_statuses)
}

fn team_chips_from_workers_with_live_statuses(
    workers: &[forktty_core::TeamWorker],
    live_statuses: &std::collections::HashMap<String, LiveSurfaceStatus>,
) -> Vec<TeamChip> {
    let mut chips: Vec<TeamChip> = Vec::new();
    for worker in workers {
        let label = team_agent_label(worker.agent.as_deref().unwrap_or("worker"));
        let dot = worker_status_dot(effective_worker_status(worker, live_statuses));
        if let Some(chip) = chips.iter_mut().find(|chip| chip.label == label) {
            chip.count += 1;
            chip.dot = merge_dots(chip.dot, dot);
        } else {
            chips.push(TeamChip {
                label,
                count: 1,
                dot,
            });
        }
    }
    chips
}

pub(super) fn latest_team_chips_for_state(state: &SocketAppState) -> Vec<TeamChip> {
    let active_workspace_id = active_workspace_id_for_state(state);
    let live_statuses = live_surface_statuses(state);
    latest_team_chips_for_workspace(
        state.team_store_path.as_deref(),
        active_workspace_id.as_deref(),
        &live_statuses,
    )
}

fn latest_team_chips_for_workspace(
    path: Option<&Path>,
    workspace_id: Option<&str>,
    live_statuses: &std::collections::HashMap<String, LiveSurfaceStatus>,
) -> Vec<TeamChip> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(store) = forktty_core::load_teams_from_path(path) else {
        return Vec::new();
    };
    store
        .teams
        .iter()
        .filter(|team| record_matches_workspace(team.workspace_id.as_deref(), workspace_id))
        .max_by_key(|team| (!team_is_closed(&team.status), team.updated_at_ms))
        .filter(|team| !team_is_closed(&team.status))
        .map(|team| team_chips_from_workers_with_live_statuses(&team.workers, live_statuses))
        .unwrap_or_default()
}

fn active_workspace_id_for_state(state: &SocketAppState) -> Option<String> {
    state
        .model
        .lock()
        .ok()
        .and_then(|model| model.active_workspace_id())
}

fn record_matches_workspace(record_workspace_id: Option<&str>, workspace_id: Option<&str>) -> bool {
    match workspace_id {
        Some(workspace_id) => record_workspace_id
            .is_none_or(|record_workspace_id| record_workspace_id == workspace_id),
        None => record_workspace_id.is_none(),
    }
}

fn live_surface_statuses(
    state: &SocketAppState,
) -> std::collections::HashMap<String, LiveSurfaceStatus> {
    state
        .model
        .lock()
        .ok()
        .map(|model| {
            model
                .list_surfaces(None)
                .into_iter()
                .filter_map(|surface| {
                    surface.agent_session.map(|session| {
                        (
                            surface.id,
                            LiveSurfaceStatus {
                                agent: session.agent,
                                lifecycle: agent_session_lifecycle_status(session.lifecycle)
                                    .to_string(),
                            },
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn agent_session_lifecycle_status(lifecycle: forktty_core::AgentSessionLifecycle) -> &'static str {
    match lifecycle {
        forktty_core::AgentSessionLifecycle::Running => "running",
        forktty_core::AgentSessionLifecycle::Idle => "idle",
        forktty_core::AgentSessionLifecycle::NeedsInput => "needs_input",
        forktty_core::AgentSessionLifecycle::Suspended => "suspended",
        forktty_core::AgentSessionLifecycle::Ended => "ended",
        forktty_core::AgentSessionLifecycle::Unknown => "unknown",
    }
}

fn effective_worker_status<'a>(
    worker: &'a forktty_core::TeamWorker,
    live_statuses: &'a std::collections::HashMap<String, LiveSurfaceStatus>,
) -> &'a str {
    worker
        .surface_id
        .as_deref()
        .and_then(|surface_id| {
            live_statuses.get(surface_id).and_then(|status| {
                matching_live_worker_status(worker.agent.as_deref(), status).map(String::as_str)
            })
        })
        .unwrap_or(worker.status.as_str())
}

fn matching_live_worker_status<'a>(
    worker_agent: Option<&str>,
    status: &'a LiveSurfaceStatus,
) -> Option<&'a String> {
    let worker_agent = worker_agent?.trim();
    if worker_agent.is_empty() {
        return None;
    }
    if forktty_core::agents::agent_metadata_aliases(status.agent)
        .iter()
        .any(|alias| worker_agent.eq_ignore_ascii_case(alias))
    {
        Some(&status.lifecycle)
    } else {
        None
    }
}

fn merge_dots(current: &'static str, next: &'static str) -> &'static str {
    let severity = |dot: &str| match dot {
        "err" => 3,
        "warn" => 2,
        "ok" => 1,
        _ => 0,
    };
    if severity(next) > severity(current) {
        next
    } else {
        current
    }
}

pub(super) fn team_agent_label(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" | "claude_code" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        "grok" => "Grok".to_string(),
        "antigravity" | "agy" => "Agy".to_string(),
        "opencode" => "OpenCode".to_string(),
        "pi" => "Pi".to_string(),
        "" | "worker" => "Worker".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "Worker".to_string(),
            }
        }
    }
}

#[cfg(test)]
fn health_list_items(workers: &[forktty_core::TeamWorker]) -> Vec<RailListItem> {
    let live_statuses = std::collections::HashMap::new();
    health_list_items_with_live_statuses(workers, &live_statuses)
}

fn health_list_items_with_live_statuses(
    workers: &[forktty_core::TeamWorker],
    live_statuses: &std::collections::HashMap<String, LiveSurfaceStatus>,
) -> Vec<RailListItem> {
    let chips = team_chips_from_workers_with_live_statuses(workers, live_statuses);
    let mut items = Vec::new();
    for chip in chips.into_iter().take(RAIL_HEALTH_SLOTS) {
        let ready = workers
            .iter()
            .filter(|worker| {
                team_agent_label(worker.agent.as_deref().unwrap_or("worker")) == chip.label
                    && worker_status_dot(effective_worker_status(worker, live_statuses)) == "ok"
            })
            .count();
        let status_word = match chip.dot {
            "err" => "attention",
            "warn" => "pending",
            "idle" => "done",
            _ => "ready",
        };
        items.push(RailListItem {
            dot: chip.dot,
            primary: chip.label,
            secondary: format!("{ready}/{}  {status_word}", chip.count),
        });
    }
    items
}

fn health_overflow_with_live_statuses(
    workers: &[forktty_core::TeamWorker],
    live_statuses: &std::collections::HashMap<String, LiveSurfaceStatus>,
) -> usize {
    team_chips_from_workers_with_live_statuses(workers, live_statuses)
        .len()
        .saturating_sub(RAIL_HEALTH_SLOTS)
}

fn report_list_items(workers: &[forktty_core::TeamWorker]) -> Vec<RailListItem> {
    workers
        .iter()
        .filter(|worker| worker_has_report(worker))
        .take(RAIL_REPORT_SLOTS)
        .map(|worker| {
            let agent = team_agent_label(worker.agent.as_deref().unwrap_or("worker"));
            let role = worker.role.as_deref().unwrap_or("report");
            RailListItem {
                dot: "ok",
                primary: format!("{agent} \u{00b7} {role}"),
                secondary: "ready".to_string(),
            }
        })
        .collect()
}

fn worker_has_report(worker: &forktty_core::TeamWorker) -> bool {
    worker
        .report
        .as_deref()
        .is_some_and(|report| !report.trim().is_empty())
}

fn current_pending_feed_approvals(
    state: &SocketAppState,
    workspace_id: Option<&str>,
) -> Option<Vec<forktty_socket::PendingFeedApproval>> {
    let approvals = state.pending_feed_approvals(usize::MAX)?;
    let model = state.model.lock().ok()?;
    Some(
        approvals
            .into_iter()
            .filter(|approval| rail_approval_matches_workspace(&model, approval, workspace_id))
            .collect(),
    )
}

fn rail_approval_id_is_currently_visible(state: &SocketAppState, approval_id: &str) -> bool {
    let active_workspace_id = active_workspace_id_for_state(state);
    let Some(approvals) = state.pending_feed_approvals(usize::MAX) else {
        return false;
    };
    let Ok(model) = state.model.lock() else {
        return false;
    };
    rail_approval_id_matches_workspace(
        &model,
        &approvals,
        approval_id,
        active_workspace_id.as_deref(),
    )
}

fn rail_approval_id_matches_workspace(
    model: &forktty_core::WorkspaceModel,
    approvals: &[forktty_socket::PendingFeedApproval],
    approval_id: &str,
    workspace_id: Option<&str>,
) -> bool {
    approvals.iter().any(|approval| {
        approval.id == approval_id && rail_approval_matches_workspace(model, approval, workspace_id)
    })
}

fn rail_approval_matches_workspace(
    model: &forktty_core::WorkspaceModel,
    approval: &forktty_socket::PendingFeedApproval,
    workspace_id: Option<&str>,
) -> bool {
    let approval_workspace = approval.workspace_id.as_deref();
    match workspace_id {
        Some(workspace_id) if approval_workspace.is_some_and(|id| id != workspace_id) => {
            return false;
        }
        None if approval_workspace.is_some() => return false,
        _ => {}
    }
    if let Some(surface_id) = approval.surface_id.as_deref() {
        let Some(surface) = model.surface(surface_id) else {
            return false;
        };
        return workspace_id.is_none_or(|workspace_id| surface.workspace_id == workspace_id);
    }
    approval_workspace.is_none_or(|workspace_id| {
        model
            .workspace_id_for(forktty_core::WorkspaceSelector::Id(workspace_id))
            .is_some()
    })
}

fn current_rail_notifications(
    model: &forktty_core::WorkspaceModel,
    workspace_id: Option<&str>,
) -> Vec<NotificationItem> {
    model
        .list_notifications()
        .into_iter()
        .filter(|notification| rail_notification_matches_workspace(notification, workspace_id))
        .filter(|notification| {
            notification
                .surface_id
                .as_deref()
                .is_none_or(|surface_id| model.surface(surface_id).is_some())
        })
        .collect()
}

fn clear_rail_notifications_from_model(
    model: &mut forktty_core::WorkspaceModel,
    workspace_id: Option<&str>,
) -> Vec<NotificationItem> {
    let notifications = current_rail_notifications(model, workspace_id);
    for notification in &notifications {
        model.dismiss_notification(&notification.id);
    }
    notifications
}

fn rail_notification_matches_workspace(
    notification: &NotificationItem,
    workspace_id: Option<&str>,
) -> bool {
    match workspace_id {
        Some(workspace_id) => notification
            .workspace_id
            .as_deref()
            .is_none_or(|record_workspace_id| record_workspace_id == workspace_id),
        None => notification.workspace_id.is_none(),
    }
}

fn notification_list_items(notifications: &[NotificationItem], now_ms: u128) -> Vec<RailListItem> {
    notifications
        .iter()
        .rev()
        .take(RAIL_NOTIFICATION_SLOTS)
        .map(|notification| RailListItem {
            dot: notification_kind_dot(notification.kind),
            primary: notification.title.trim().to_string(),
            secondary: relative_time_label(now_ms, notification.created_at_ms),
        })
        .collect()
}

fn notification_kind_dot(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "warn",
        NotificationKind::Error => "err",
        NotificationKind::Info => "info",
        NotificationKind::Custom => "idle",
    }
}

fn format_notification_counts(notifications: &[NotificationItem]) -> String {
    let unread = notifications
        .iter()
        .filter(|notification| !notification.read)
        .count();
    let errors = notifications
        .iter()
        .filter(|notification| notification.kind == NotificationKind::Error && !notification.read)
        .count();
    if errors > 0 {
        format!("{unread} new, {}", count_label(errors, "error", "errors"))
    } else if unread > 0 {
        format!("{unread} new")
    } else {
        "clear".to_string()
    }
}

fn format_loop_state(workflow: &forktty_core::WorkflowState) -> String {
    let stage = workflow.loop_stage.as_deref();
    match (loop_iteration_display(workflow), stage) {
        (Some(iteration), Some(stage)) => format!("{iteration} - {stage}"),
        (Some(iteration), None) => iteration,
        (None, Some(stage)) => stage.to_string(),
        (None, None) => "not planned".to_string(),
    }
}

fn format_loop_caption(workflow: &forktty_core::WorkflowState) -> String {
    if workflow.loop_gates.is_empty() {
        return if workflow_has_loop_state(workflow) {
            "0 gates".to_string()
        } else {
            "No loop metadata".to_string()
        };
    }
    let (passed, failed, running) = loop_gate_counts(&workflow.loop_gates);
    let open = workflow
        .loop_gates
        .len()
        .saturating_sub(passed + failed + running);
    let mut parts = Vec::new();
    if passed > 0 {
        parts.push(count_label(passed, "passed", "passed"));
    }
    if failed > 0 {
        parts.push(count_label(failed, "failed", "failed"));
    }
    if running > 0 {
        parts.push(count_label(running, "running", "running"));
    }
    if open > 0 {
        parts.push(count_label(open, "open", "open"));
    }
    if parts.is_empty() {
        count_label(workflow.loop_gates.len(), "gate", "gates")
    } else {
        format!(
            "{}: {}",
            count_label(workflow.loop_gates.len(), "gate", "gates"),
            parts.join(", ")
        )
    }
}

fn loop_fraction(workflow: &forktty_core::WorkflowState) -> f64 {
    match (workflow.loop_iteration, workflow.loop_max_iterations) {
        (Some(iteration), Some(max)) if max > 0 => f64::from(iteration) / f64::from(max),
        _ => 0.0,
    }
}

fn loop_iteration_display(workflow: &forktty_core::WorkflowState) -> Option<String> {
    match (workflow.loop_iteration, workflow.loop_max_iterations) {
        (Some(iteration), Some(max)) => Some(format!("Loop {iteration}/{max}")),
        (Some(iteration), None) => Some(format!("Loop {iteration}")),
        _ => None,
    }
}

fn workflow_has_loop_state(workflow: &forktty_core::WorkflowState) -> bool {
    workflow.loop_recipe.is_some()
        || workflow.loop_stage.is_some()
        || workflow.loop_iteration.is_some()
        || workflow.loop_max_iterations.is_some()
        || workflow.loop_stop_reason.is_some()
        || !workflow.loop_gates.is_empty()
}

fn loop_gate_counts(gates: &[forktty_core::WorkflowLoopGate]) -> (usize, usize, usize) {
    let passed = gates
        .iter()
        .filter(|gate| loop_gate_status_is_passed(&gate.status))
        .count();
    let failed = gates
        .iter()
        .filter(|gate| loop_gate_status_is_failed(&gate.status))
        .count();
    let running = gates
        .iter()
        .filter(|gate| loop_gate_status_is_running(&gate.status))
        .count();
    (passed, failed, running)
}

fn loop_gate_status_is_passed(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "passed" | "pass" | "ok" | "success" | "succeeded" | "done"
    )
}

fn loop_gate_status_is_failed(status: &str) -> bool {
    let status = status.trim().to_ascii_lowercase();
    status == "failed"
        || status == "fail"
        || status == "error"
        || status == "errored"
        || status == "blocked"
}

fn loop_gate_status_is_running(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "running" | "active" | "working" | "in_progress" | "in-progress"
    )
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(agent: &str, status: &str, report: Option<&str>) -> forktty_core::TeamWorker {
        forktty_core::TeamWorker {
            id: format!("{agent}-{status}"),
            role: Some("main".to_string()),
            agent: Some(agent.to_string()),
            surface_id: None,
            launched_surface_id: None,
            worktree_name: None,
            cwd: None,
            report: report.map(str::to_string),
            status: status.to_string(),
            assigned_task_id: None,
            last_heartbeat_ms: 0,
            launched_at_ms: 0,
            last_nudge_ms: 0,
            shutdown_requested_at_ms: 0,
        }
    }

    #[test]
    fn team_chips_group_by_agent_and_keep_worst_dot() {
        let chips = team_chips_from_workers(&[
            worker("codex", "running", None),
            worker("codex", "running", None),
            worker("claude", "running", None),
            worker("grok", "failed", None),
        ]);
        assert_eq!(
            chips,
            vec![
                TeamChip {
                    label: "Codex".to_string(),
                    count: 2,
                    dot: "ok"
                },
                TeamChip {
                    label: "Claude".to_string(),
                    count: 1,
                    dot: "ok"
                },
                TeamChip {
                    label: "Grok".to_string(),
                    count: 1,
                    dot: "err"
                },
            ]
        );
    }

    #[test]
    fn team_agent_label_maps_known_harnesses_and_capitalizes_custom() {
        assert_eq!(team_agent_label("claude-code"), "Claude");
        assert_eq!(team_agent_label("antigravity"), "Agy");
        assert_eq!(team_agent_label("mistral"), "Mistral");
        assert_eq!(team_agent_label(""), "Worker");
    }

    #[test]
    fn health_items_report_ready_counts_per_agent() {
        let items = health_list_items(&[
            worker("codex", "running", None),
            worker("codex", "idle", None),
            worker("grok", "no_heartbeat", None),
        ]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].primary, "Codex");
        assert_eq!(items[0].secondary, "1/2  pending");
        assert_eq!(items[0].dot, "warn");
        assert_eq!(items[1].primary, "Grok");
        assert_eq!(items[1].secondary, "0/1  attention");
        assert_eq!(items[1].dot, "err");
    }

    #[test]
    fn health_items_expose_overflow_when_agent_groups_are_capped() {
        let workers = [
            worker("codex", "running", None),
            worker("claude", "running", None),
            worker("grok", "running", None),
            worker("antigravity", "running", None),
            worker("opencode", "running", None),
            worker("custom-agent", "running", None),
        ];
        let team = TeamRailSnapshot::from_workers_for_test(&workers);

        assert_eq!(team.health_items.len(), RAIL_HEALTH_SLOTS);
        assert_eq!(team.health_overflow, 1);
    }

    #[test]
    fn attention_summary_prioritizes_human_action() {
        let workers = [
            worker("codex", "running", Some("report")),
            worker("claude", "needs_input", None),
            worker("grok", "failed", None),
        ];
        let team = TeamRailSnapshot::from_workers_for_test(&workers);

        assert_eq!(
            format_attention_summary(1, 1, &team),
            "Attention 4 · Running 1 · Reports 1 · Errors 1"
        );
    }

    #[test]
    fn attention_summary_reads_all_clear_when_nothing_needs_action() {
        let team = TeamRailSnapshot::default();

        assert_eq!(format_attention_summary(0, 0, &team), "All clear");
    }

    #[test]
    fn empty_sidebar_sections_hide_secondary_actions() {
        let empty = RailSnapshot::default();
        let with_approval = RailSnapshot {
            approval_items: vec![RailApprovalItem {
                id: "approval-1".to_string(),
                title: "Claude needs input".to_string(),
                caption: "Requested now".to_string(),
            }],
            ..RailSnapshot::default()
        };
        let with_report = RailSnapshot {
            report_count: 1,
            ..RailSnapshot::default()
        };

        assert!(!approvals_settings_visible(&empty));
        assert!(!reports_link_visible(&empty));
        assert!(approvals_settings_visible(&with_approval));
        assert!(reports_link_visible(&with_report));
    }

    #[test]
    fn live_surface_lifecycle_overrides_persisted_worker_status() {
        let mut grok = worker("grok", "running", None);
        grok.surface_id = Some("surface-grok".to_string());
        let live_statuses = std::collections::HashMap::from([(
            "surface-grok".to_string(),
            LiveSurfaceStatus {
                agent: forktty_core::AgentKind::Grok,
                lifecycle: "needs_input".to_string(),
            },
        )]);

        let items = health_list_items_with_live_statuses(&[grok.clone()], &live_statuses);
        assert_eq!(items[0].primary, "Grok");
        assert_eq!(items[0].secondary, "0/1  pending");
        assert_eq!(items[0].dot, "warn");

        let chips = team_chips_from_workers_with_live_statuses(&[grok], &live_statuses);
        assert_eq!(chips[0].dot, "warn");
    }

    #[test]
    fn live_surface_lifecycle_ignores_mismatched_worker_agent() {
        let mut grok = worker("grok", "running", None);
        grok.surface_id = Some("surface-grok".to_string());
        let live_statuses = std::collections::HashMap::from([(
            "surface-grok".to_string(),
            LiveSurfaceStatus {
                agent: forktty_core::AgentKind::ClaudeCode,
                lifecycle: "needs_input".to_string(),
            },
        )]);

        let items = health_list_items_with_live_statuses(&[grok.clone()], &live_statuses);
        assert_eq!(items[0].primary, "Grok");
        assert_eq!(items[0].secondary, "1/1  ready");
        assert_eq!(items[0].dot, "ok");

        let chips = team_chips_from_workers_with_live_statuses(&[grok], &live_statuses);
        assert_eq!(chips[0].dot, "ok");
    }

    #[test]
    fn shutdown_workers_read_as_done_not_pending() {
        let items = health_list_items(&[
            worker("grok", "shutdown", None),
            worker("agy", "shutdown_requested", None),
        ]);
        assert_eq!(items[0].dot, "idle");
        assert_eq!(items[0].secondary, "0/1  done");
        assert_eq!(items[1].dot, "idle");
        // A mixed group keeps the most severe state visible.
        assert_eq!(merge_dots("idle", "ok"), "ok");
        assert_eq!(merge_dots("ok", "idle"), "ok");
        assert_eq!(merge_dots("ok", "err"), "err");
    }

    #[test]
    fn closed_team_statuses_match_core_lifecycle() {
        for status in ["done", "closed", "cancelled", "finished"] {
            assert!(team_is_closed(status), "{status} should read as closed");
        }
        for status in ["active", "running", "working", "forming"] {
            assert!(!team_is_closed(status), "{status} should read as open");
        }
    }

    #[test]
    fn rail_snapshots_ignore_records_from_inactive_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("workflow-v1.json");
        let team_path = dir.path().join("team-v1.json");

        let mut workflow_store = forktty_core::WorkflowStoreData::default();
        workflow_store
            .upsert(
                forktty_core::WorkflowUpsert {
                    workflow_id: Some("closed-router".to_string()),
                    workspace_id: Some("workspace-closed".to_string()),
                    mode: Some("task_strategy".to_string()),
                    status: Some("done".to_string()),
                    goal: Some("Closed workspace router task".to_string()),
                    ..forktty_core::WorkflowUpsert::default()
                },
                10,
            )
            .unwrap();
        forktty_core::save_workflows_to_path(&workflow_path, &workflow_store).unwrap();

        let mut team_store = forktty_core::TeamStoreData::default();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "closed-team".to_string(),
                    workspace_id: Some("workspace-closed".to_string()),
                    leader_surface_id: None,
                    name: Some("Closed Team".to_string()),
                    status: Some("done".to_string()),
                    goal: None,
                },
                20,
            )
            .unwrap();
        forktty_core::save_teams_to_path(&team_path, &team_store).unwrap();

        let workflow =
            latest_workflow_snapshot_for_workspace(Some(&workflow_path), Some("workspace-main"));
        assert_eq!(workflow.strategy, "idle");
        assert_eq!(workflow.workflow, "none");

        let live_statuses = std::collections::HashMap::new();
        let team = latest_team_snapshot_for_workspace(
            Some(&team_path),
            Some("workspace-main"),
            &live_statuses,
        );
        assert_eq!(team.fanout, "0");
        assert_eq!(team.workers, "no team");

        let chips = latest_team_chips_for_workspace(
            Some(&team_path),
            Some("workspace-main"),
            &live_statuses,
        );
        assert!(chips.is_empty());
    }

    #[test]
    fn rail_snapshots_include_global_records_for_active_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("workflow-v1.json");
        let team_path = dir.path().join("team-v1.json");

        let mut workflow_store = forktty_core::WorkflowStoreData::default();
        workflow_store
            .upsert(
                forktty_core::WorkflowUpsert {
                    workflow_id: Some("global-router".to_string()),
                    workspace_id: None,
                    mode: Some("task_strategy".to_string()),
                    status: Some("running".to_string()),
                    goal: Some("Global Router task".to_string()),
                    ..forktty_core::WorkflowUpsert::default()
                },
                10,
            )
            .unwrap();
        forktty_core::save_workflows_to_path(&workflow_path, &workflow_store).unwrap();

        let mut team_store = forktty_core::TeamStoreData::default();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "global-team".to_string(),
                    workspace_id: None,
                    leader_surface_id: None,
                    name: Some("Global Team".to_string()),
                    status: Some("active".to_string()),
                    goal: Some("Global Router task".to_string()),
                },
                20,
            )
            .unwrap();
        team_store
            .upsert_worker(
                forktty_core::TeamWorkerUpsert {
                    team_id: "global-team".to_string(),
                    worker_id: "global-worker".to_string(),
                    role: Some("reviewer".to_string()),
                    agent: Some("codex".to_string()),
                    surface_id: None,
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                21,
            )
            .unwrap();
        forktty_core::save_teams_to_path(&team_path, &team_store).unwrap();

        let workflow =
            latest_workflow_snapshot_for_workspace(Some(&workflow_path), Some("workspace-main"));
        assert_eq!(workflow.workflow, "running");
        assert_eq!(workflow.strategy_detail, "Global Router task");

        let live_statuses = std::collections::HashMap::new();
        let team = latest_team_snapshot_for_workspace(
            Some(&team_path),
            Some("workspace-main"),
            &live_statuses,
        );
        assert_eq!(team.fanout, "1");
        assert_eq!(team.workers, "1/1 active");

        let chips = latest_team_chips_for_workspace(
            Some(&team_path),
            Some("workspace-main"),
            &live_statuses,
        );
        assert_eq!(chips.len(), 1);
    }

    #[test]
    fn rail_workflow_prefers_open_record_over_newer_terminal_record() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("workflow-v1.json");
        let mut workflow_store = forktty_core::WorkflowStoreData::default();
        workflow_store
            .upsert(
                forktty_core::WorkflowUpsert {
                    workflow_id: Some("active-router".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    mode: Some("review".to_string()),
                    status: Some("running".to_string()),
                    goal: Some("Active router audit".to_string()),
                    ..forktty_core::WorkflowUpsert::default()
                },
                10,
            )
            .unwrap();
        workflow_store
            .upsert(
                forktty_core::WorkflowUpsert {
                    workflow_id: Some("newer-done-router".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    mode: Some("review".to_string()),
                    status: Some("done".to_string()),
                    goal: Some("Older completed router audit".to_string()),
                    ..forktty_core::WorkflowUpsert::default()
                },
                20,
            )
            .unwrap();
        forktty_core::save_workflows_to_path(&workflow_path, &workflow_store).unwrap();

        let workflow =
            latest_workflow_snapshot_for_workspace(Some(&workflow_path), Some("workspace-1"));

        assert_eq!(workflow.workflow, "running");
        assert_eq!(workflow.strategy_detail, "Active router audit");
    }

    #[test]
    fn rail_team_chips_prefer_open_record_over_newer_terminal_record() {
        let dir = tempfile::tempdir().unwrap();
        let team_path = dir.path().join("team-v1.json");
        let mut team_store = forktty_core::TeamStoreData::default();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "active-team".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: None,
                    name: Some("Active Team".to_string()),
                    status: Some("active".to_string()),
                    goal: Some("Active router audit".to_string()),
                },
                10,
            )
            .unwrap();
        team_store
            .upsert_worker(
                forktty_core::TeamWorkerUpsert {
                    team_id: "active-team".to_string(),
                    worker_id: "active-worker".to_string(),
                    role: Some("reviewer".to_string()),
                    agent: Some("codex".to_string()),
                    surface_id: Some("surface-active".to_string()),
                    worktree_name: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                    report: None,
                },
                11,
            )
            .unwrap();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "newer-done-team".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: None,
                    name: Some("Done Team".to_string()),
                    status: Some("done".to_string()),
                    goal: Some("Completed router audit".to_string()),
                },
                20,
            )
            .unwrap();
        team_store
            .upsert_worker(
                forktty_core::TeamWorkerUpsert {
                    team_id: "newer-done-team".to_string(),
                    worker_id: "done-worker".to_string(),
                    role: Some("reviewer".to_string()),
                    agent: Some("grok".to_string()),
                    surface_id: Some("surface-done".to_string()),
                    worktree_name: None,
                    status: Some("done".to_string()),
                    assigned_task_id: None,
                    report: None,
                },
                21,
            )
            .unwrap();
        forktty_core::save_teams_to_path(&team_path, &team_store).unwrap();

        let live_statuses = std::collections::HashMap::new();
        let chips =
            latest_team_chips_for_workspace(Some(&team_path), Some("workspace-1"), &live_statuses);

        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].label, "Codex");
        assert_eq!(chips[0].dot, "ok");
    }

    #[test]
    fn rail_loop_snapshot_shows_stage_iteration_and_gate_state() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("workflow-v1.json");
        let mut workflow_store = forktty_core::WorkflowStoreData::default();
        workflow_store
            .upsert(
                forktty_core::WorkflowUpsert {
                    workflow_id: Some("loop-router".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    mode: Some("task_strategy".to_string()),
                    status: Some("running".to_string()),
                    goal: Some("Keep checking until clean".to_string()),
                    ..forktty_core::WorkflowUpsert::default()
                },
                10,
            )
            .unwrap();
        workflow_store
            .set_loop_state(
                "loop-router",
                forktty_core::WorkflowLoopStateInput {
                    recipe: Some("solo_with_verify_loop".to_string()),
                    stage: Some("verify".to_string()),
                    iteration: Some(2),
                    max_iterations: Some(4),
                    gates: Some(vec![
                        forktty_core::WorkflowLoopGateInput {
                            id: "fmt".to_string(),
                            kind: "command".to_string(),
                            label: "cargo fmt".to_string(),
                            status: "passed".to_string(),
                            summary: None,
                        },
                        forktty_core::WorkflowLoopGateInput {
                            id: "test".to_string(),
                            kind: "command".to_string(),
                            label: "cargo test".to_string(),
                            status: "failed".to_string(),
                            summary: None,
                        },
                        forktty_core::WorkflowLoopGateInput {
                            id: "clippy".to_string(),
                            kind: "command".to_string(),
                            label: "cargo clippy".to_string(),
                            status: "running".to_string(),
                            summary: None,
                        },
                    ]),
                    ..forktty_core::WorkflowLoopStateInput::default()
                },
                20,
            )
            .unwrap();
        forktty_core::save_workflows_to_path(&workflow_path, &workflow_store).unwrap();

        let workflow =
            latest_workflow_snapshot_for_workspace(Some(&workflow_path), Some("workspace-1"));

        assert_eq!(workflow.loop_state, "Loop 2/4 - verify");
        assert_eq!(
            workflow.loop_caption,
            "3 gates: 1 passed, 1 failed, 1 running"
        );
        assert_eq!(workflow.loop_iteration_display.as_deref(), Some("Loop 2/4"));
    }

    #[test]
    fn rail_loop_caption_treats_pending_gates_as_open_not_running() {
        let mut workflow_store = forktty_core::WorkflowStoreData::default();
        let workflow = workflow_store
            .upsert(
                forktty_core::WorkflowUpsert {
                    workflow_id: Some("loop-router".to_string()),
                    workspace_id: Some("workspace-1".to_string()),
                    mode: Some("task_strategy".to_string()),
                    status: Some("running".to_string()),
                    goal: Some("Keep checking until clean".to_string()),
                    ..forktty_core::WorkflowUpsert::default()
                },
                10,
            )
            .unwrap();
        let workflow = workflow_store
            .set_loop_state(
                &workflow.id,
                forktty_core::WorkflowLoopStateInput {
                    recipe: Some("solo_with_verify_loop".to_string()),
                    stage: Some("planned".to_string()),
                    iteration: Some(0),
                    max_iterations: Some(3),
                    gates: Some(vec![
                        forktty_core::WorkflowLoopGateInput {
                            id: "implement".to_string(),
                            kind: "stage".to_string(),
                            label: "Implementation".to_string(),
                            status: "pending".to_string(),
                            summary: None,
                        },
                        forktty_core::WorkflowLoopGateInput {
                            id: "verify".to_string(),
                            kind: "verification".to_string(),
                            label: "Verification".to_string(),
                            status: "pending".to_string(),
                            summary: None,
                        },
                    ]),
                    ..forktty_core::WorkflowLoopStateInput::default()
                },
                20,
            )
            .unwrap();

        assert_eq!(format_loop_caption(&workflow), "2 gates: 2 open");
    }

    #[test]
    fn rail_team_prefers_open_record_over_newer_terminal_record() {
        let dir = tempfile::tempdir().unwrap();
        let team_path = dir.path().join("team-v1.json");
        let mut team_store = forktty_core::TeamStoreData::default();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "active-team".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: None,
                    name: Some("Active Team".to_string()),
                    status: Some("active".to_string()),
                    goal: None,
                },
                10,
            )
            .unwrap();
        team_store
            .upsert_worker(
                forktty_core::TeamWorkerUpsert {
                    team_id: "active-team".to_string(),
                    worker_id: "active-worker".to_string(),
                    role: Some("reviewer".to_string()),
                    agent: Some("codex".to_string()),
                    surface_id: None,
                    worktree_name: None,
                    report: None,
                    status: Some("running".to_string()),
                    assigned_task_id: None,
                },
                11,
            )
            .unwrap();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "newer-done-team".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: None,
                    name: Some("Done Team".to_string()),
                    status: Some("done".to_string()),
                    goal: None,
                },
                20,
            )
            .unwrap();
        forktty_core::save_teams_to_path(&team_path, &team_store).unwrap();

        let live_statuses = std::collections::HashMap::new();
        let team = latest_team_snapshot_for_workspace(
            Some(&team_path),
            Some("workspace-1"),
            &live_statuses,
        );

        assert_eq!(team.fanout, "1");
        assert_eq!(team.workers, "1/1 active");
        assert_eq!(team.health_items[0].primary, "Codex");
    }

    #[test]
    fn report_items_only_include_workers_with_reports() {
        let items = report_list_items(&[
            worker("codex", "running", Some("done: findings")),
            worker("claude", "running", None),
            worker("grok", "done", Some("  ")),
        ]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].primary, "Codex \u{00b7} main");
        assert_eq!(items[0].secondary, "ready");
    }

    #[test]
    fn report_count_uses_total_reports_even_when_rows_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let team_path = dir.path().join("team-v1.json");
        let mut team_store = forktty_core::TeamStoreData::default();
        team_store
            .upsert_team(
                forktty_core::TeamUpsert {
                    team_id: "team-1".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    leader_surface_id: None,
                    name: Some("Team".to_string()),
                    status: Some("done".to_string()),
                    goal: None,
                },
                10,
            )
            .unwrap();
        for index in 0..6 {
            team_store
                .upsert_worker(
                    forktty_core::TeamWorkerUpsert {
                        team_id: "team-1".to_string(),
                        worker_id: format!("worker-{index}"),
                        role: Some("reviewer".to_string()),
                        agent: Some(format!("agent-{index}")),
                        surface_id: None,
                        worktree_name: None,
                        report: Some(format!("report {index}")),
                        status: Some("done".to_string()),
                        assigned_task_id: None,
                    },
                    11 + index,
                )
                .unwrap();
        }
        forktty_core::save_teams_to_path(&team_path, &team_store).unwrap();

        let live_statuses = std::collections::HashMap::new();
        let team = latest_team_snapshot_for_workspace(
            Some(&team_path),
            Some("workspace-1"),
            &live_statuses,
        );

        assert_eq!(team.reports, "6 reports");
        assert_eq!(team.report_items.len(), RAIL_REPORT_SLOTS);
        assert_eq!(team.report_overflow, 1);
    }

    #[test]
    fn rail_approvals_ignore_closed_surface_prompts() {
        let mut model = forktty_core::WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let prompt_surface = model
            .split_surface(
                &workspace.focused_surface_id,
                forktty_core::SplitAxis::Horizontal,
            )
            .unwrap();
        let live_approval = forktty_socket::PendingFeedApproval {
            id: "live".to_string(),
            title: "Live approval".to_string(),
            workspace_id: Some(workspace.id.clone()),
            surface_id: Some(workspace.focused_surface_id.clone()),
            created_at_ms: 1,
        };
        let stale_approval = forktty_socket::PendingFeedApproval {
            id: "stale".to_string(),
            title: "Claude needs input".to_string(),
            workspace_id: Some(workspace.id.clone()),
            surface_id: Some(prompt_surface.id.clone()),
            created_at_ms: 2,
        };
        model.close_surface(&prompt_surface.id).unwrap();

        assert!(rail_approval_matches_workspace(
            &model,
            &live_approval,
            Some(&workspace.id)
        ));
        assert!(!rail_approval_matches_workspace(
            &model,
            &stale_approval,
            Some(&workspace.id)
        ));
    }

    #[test]
    fn rail_approval_id_revalidation_uses_current_workspace() {
        let mut model = forktty_core::WorkspaceModel::new();
        let first = model.create_workspace("first", "/tmp/first");
        let second = model.create_workspace("second", "/tmp/second");
        let approval = forktty_socket::PendingFeedApproval {
            id: "approval-1".to_string(),
            title: "Claude needs input".to_string(),
            workspace_id: Some(first.id.clone()),
            surface_id: Some(first.focused_surface_id.clone()),
            created_at_ms: 1,
        };
        let approvals = vec![approval];

        assert!(rail_approval_id_matches_workspace(
            &model,
            &approvals,
            "approval-1",
            Some(&first.id)
        ));
        assert!(!rail_approval_id_matches_workspace(
            &model,
            &approvals,
            "approval-1",
            Some(&second.id)
        ));
    }

    #[test]
    fn rail_notifications_ignore_closed_surface_prompts() {
        let mut model = forktty_core::WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let prompt_surface = model
            .split_surface(
                &workspace.focused_surface_id,
                forktty_core::SplitAxis::Horizontal,
            )
            .unwrap();
        model.create_notification(
            "Claude needs input",
            "Turn complete",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(prompt_surface.id.clone()),
        );
        model.create_notification(
            "Workspace info",
            "Still relevant",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );
        model.close_surface(&prompt_surface.id).unwrap();

        let notifications = current_rail_notifications(&model, Some(&workspace.id));

        assert_eq!(format_notification_counts(&notifications), "1 new");
        assert_eq!(notifications[0].title, "Workspace info");
    }

    #[test]
    fn format_notification_counts_surfaces_new_and_errors() {
        let prompt = NotificationItem {
            id: "one".to_string(),
            title: "Input".to_string(),
            body: String::new(),
            kind: NotificationKind::Prompt,
            created_at_ms: 1,
            read: false,
            workspace_id: None,
            surface_id: None,
            terminal_metadata: None,
        };
        let error = NotificationItem {
            id: "two".to_string(),
            title: "Error".to_string(),
            body: String::new(),
            kind: NotificationKind::Error,
            created_at_ms: 2,
            read: false,
            workspace_id: None,
            surface_id: None,
            terminal_metadata: None,
        };
        assert_eq!(
            format_notification_counts(&[prompt.clone(), error]),
            "2 new, 1 error"
        );
        assert_eq!(format_notification_counts(&[prompt]), "1 new");
        assert_eq!(format_notification_counts(&[]), "clear");
    }

    #[test]
    fn clear_rail_notifications_removes_visible_notifications() {
        let mut model = forktty_core::WorkspaceModel::new();
        model.create_notification("Build failed", "", NotificationKind::Error, None, None);
        model.create_notification(
            "Claude needs input",
            "",
            NotificationKind::Prompt,
            None,
            None,
        );

        let cleared = clear_rail_notifications_from_model(&mut model, None);

        assert_eq!(cleared.len(), 2);
        assert!(model.list_notifications().is_empty());
    }

    #[test]
    fn rail_notification_clear_sends_terminal_close_report() {
        let model = std::sync::Arc::new(std::sync::Mutex::new(forktty_core::WorkspaceModel::new()));
        let terminal = std::sync::Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            std::path::PathBuf::from("/tmp/forktty.sock"),
        );
        let surface_id = {
            let mut model = model.lock().unwrap();
            model.create_workspace("main", "/tmp").focused_surface_id
        };
        spawn_focused_surface_if_needed(&state).unwrap();
        let notification = NotificationItem {
            id: "notification-1".to_string(),
            title: "Build".to_string(),
            body: "Done".to_string(),
            kind: NotificationKind::Prompt,
            created_at_ms: 1,
            read: false,
            workspace_id: None,
            surface_id: Some(surface_id.clone()),
            terminal_metadata: Some(TerminalNotificationMetadata {
                id: "build".to_string(),
                report_activation: false,
                report_close: true,
                buttons: Vec::new(),
                icon_names: Vec::new(),
                icon_data: None,
                icon_cache_id: None,
                urgency: None,
                sound_name: None,
                expires_after_ms: None,
                app_name: None,
                notification_types: Vec::new(),
            }),
        };

        super::notifications_panel::send_terminal_notification_close_reports_via_backend(
            state.terminal.clone(),
            vec![notification],
        );
        let expected = vec!["\x1b]99;i=build:p=close;\x1b\\"];
        for _ in 0..20 {
            if terminal.sent_text(&surface_id).unwrap() == expected {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(terminal.sent_text(&surface_id).unwrap(), expected);
    }

    #[test]
    fn clear_rail_notifications_preserves_inactive_workspace_notifications() {
        let mut model = forktty_core::WorkspaceModel::new();
        let active = model.create_workspace("main", "/tmp/main");
        let active_surface_id = active.focused_surface_id.clone();
        let inactive = model.create_workspace("other", "/tmp/other");
        let inactive_surface_id = inactive.focused_surface_id.clone();
        model.focus_surface_and_select_workspace(&active_surface_id);
        model.create_notification(
            "Active notice",
            "",
            NotificationKind::Prompt,
            Some(active.id.clone()),
            Some(active_surface_id),
        );
        model.create_notification(
            "Inactive notice",
            "",
            NotificationKind::Prompt,
            Some(inactive.id),
            Some(inactive_surface_id),
        );

        let cleared = clear_rail_notifications_from_model(&mut model, Some(&active.id));
        let remaining = model.list_notifications();

        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].title, "Active notice");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "Inactive notice");
    }

    #[test]
    fn count_label_uses_singular_for_one() {
        assert_eq!(count_label(0, "report", "reports"), "0 reports");
        assert_eq!(count_label(1, "report", "reports"), "1 report");
        assert_eq!(count_label(2, "report", "reports"), "2 reports");
    }
}
