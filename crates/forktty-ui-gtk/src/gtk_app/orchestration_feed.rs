//! Bottom workflow feed dock for the GTK workbench: tabbed live rows over
//! workflow events, team events, notifications, and attention rows.

use super::*;

const FEED_ROW_COUNT: usize = 5;
const FEED_LOG_ROWS_WHEN_EVENTS_EXIST: usize = 2;
const FEED_ATTENTION_SCAN_LIMIT: usize = FEED_ROW_COUNT * 4;
const FEED_FILTER_COUNT: usize = 4;
type FeedClearCutoffs = std::collections::HashMap<Option<String>, [u128; FEED_FILTER_COUNT]>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedFilter {
    All,
    Attention,
    Events,
    Logs,
}

#[derive(Clone)]
pub(super) struct OrchestrationFeedUi {
    pub(super) shell: gtk::Box,
    rows_revealer: gtk::Revealer,
    collapse_button: gtk::Button,
    clear_button: gtk::Button,
    live_chip: gtk::Label,
    rows: Vec<FeedRowUi>,
    tabs: Vec<gtk::Button>,
    filter: Rc<Cell<u8>>,
    cleared_before_ms: Rc<RefCell<FeedClearCutoffs>>,
    collapsed: Rc<Cell<bool>>,
}

#[derive(Clone)]
struct FeedRowUi {
    shell: gtk::Box,
    time: gtk::Label,
    body: gtk::Label,
    status: gtk::Label,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeedLine {
    at_ms: u128,
    source: FeedSource,
    body: String,
    status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedSource {
    Workflow,
    Team,
    Notification,
    Log,
}

pub(super) fn build_orchestration_feed(state: &SocketAppState) -> OrchestrationFeedUi {
    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.add_css_class("orchestration-feed");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    header.add_css_class("orchestration-feed-header");
    let mut tabs = Vec::new();
    for (label, active) in [
        ("WORKFLOW FEED", true),
        ("ATTENTION", false),
        ("EVENTS", false),
        ("LOGS", false),
    ] {
        let tab = gtk::Button::builder().label(label).has_frame(false).build();
        tab.add_css_class("flat");
        tab.add_css_class("orchestration-feed-tab");
        if active {
            tab.add_css_class("active");
        }
        set_accessible_button_text(&tab, label, None);
        header.append(&tab);
        tabs.push(tab);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&spacer);
    let live = gtk::Label::builder()
        .label("live")
        .single_line_mode(true)
        .build();
    live.add_css_class("orchestration-feed-live");
    header.append(&live);
    let clear = gtk::Button::builder()
        .label("Clear")
        .has_frame(false)
        .tooltip_text("Hide current feed rows")
        .build();
    clear.add_css_class("flat");
    clear.add_css_class("orchestration-feed-clear");
    set_accessible_button_text(&clear, "Hide current feed rows", None);
    header.append(&clear);
    let collapse = gtk::Button::builder()
        .icon_name("forktty-chevron-down-symbolic")
        .has_frame(false)
        .tooltip_text("Collapse workflow feed")
        .build();
    collapse.add_css_class("flat");
    collapse.add_css_class("orchestration-feed-collapse");
    set_accessible_button_text(&collapse, "Collapse workflow feed", None);
    header.append(&collapse);
    shell.append(&header);

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows_box.add_css_class("orchestration-feed-rows");
    rows_box.add_css_class("orchestration-feed-rows-shell");
    // Fixed rows-area height keeps the dock from jumping as rows appear/disappear.
    rows_box.set_height_request(122);
    let rows = (0..FEED_ROW_COUNT)
        .map(|_| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.add_css_class("orchestration-feed-row");
            let time = gtk::Label::builder()
                .label("")
                .xalign(0.0)
                .single_line_mode(true)
                .width_chars(8)
                .build();
            time.add_css_class("orchestration-feed-time");
            let body = gtk::Label::builder()
                .label("")
                .xalign(0.0)
                .hexpand(true)
                .single_line_mode(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            body.add_css_class("orchestration-feed-body");
            let status = gtk::Label::builder()
                .label("")
                .single_line_mode(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(14)
                .build();
            status.add_css_class("orchestration-feed-status");
            row.append(&time);
            row.append(&body);
            row.append(&status);
            rows_box.append(&row);
            FeedRowUi {
                shell: row,
                time,
                body,
                status,
            }
        })
        .collect::<Vec<_>>();
    let rows_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(160)
        .reveal_child(true)
        .child(&rows_box)
        .build();
    shell.append(&rows_revealer);

    let ui = OrchestrationFeedUi {
        shell,
        rows_revealer,
        collapse_button: collapse,
        clear_button: clear.clone(),
        live_chip: live,
        rows,
        tabs,
        filter: Rc::new(Cell::new(0)),
        cleared_before_ms: Rc::new(RefCell::new(FeedClearCutoffs::default())),
        collapsed: Rc::new(Cell::new(false)),
    };
    set_workflow_feed_collapsed(&ui, false);
    for (index, tab) in ui.tabs.iter().enumerate() {
        let ui_for_tab = ui.clone();
        let state_for_tab = state.clone();
        tab.connect_clicked(move |_| {
            if ui_for_tab.collapsed.get() {
                set_workflow_feed_collapsed(&ui_for_tab, false);
            }
            ui_for_tab.filter.set(index as u8);
            for (tab_index, tab) in ui_for_tab.tabs.iter().enumerate() {
                if tab_index == index {
                    tab.add_css_class("active");
                } else {
                    tab.remove_css_class("active");
                }
            }
            refresh_orchestration_feed(&ui_for_tab, &state_for_tab);
        });
    }
    let ui_for_clear = ui.clone();
    let state_for_clear = state.clone();
    clear.connect_clicked(move |_| {
        let filter = feed_filter_from_index(ui_for_clear.filter.get());
        let workspace_id = active_workspace_id_for_state(&state_for_clear);
        set_feed_clear_cutoff(
            &mut ui_for_clear.cleared_before_ms.borrow_mut(),
            workspace_id.as_deref(),
            filter,
            now_unix_ms(),
        );
        refresh_orchestration_feed(&ui_for_clear, &state_for_clear);
    });
    let ui_for_collapse = ui.clone();
    ui.collapse_button.connect_clicked(move |_| {
        let collapsed = !ui_for_collapse.collapsed.get();
        set_workflow_feed_collapsed(&ui_for_collapse, collapsed);
    });
    refresh_orchestration_feed(&ui, state);
    ui
}

fn set_workflow_feed_collapsed(ui: &OrchestrationFeedUi, collapsed: bool) {
    ui.collapsed.set(collapsed);
    ui.rows_revealer.set_reveal_child(!collapsed);
    ui.clear_button.set_visible(!collapsed);
    ui.live_chip.set_visible(!collapsed);
    if collapsed {
        ui.shell.add_css_class("collapsed");
    } else {
        ui.shell.remove_css_class("collapsed");
    }
    ui.collapse_button.set_icon_name(if collapsed {
        "forktty-chevron-up-symbolic"
    } else {
        "forktty-chevron-down-symbolic"
    });
    ui.collapse_button.set_tooltip_text(Some(if collapsed {
        "Expand workflow feed"
    } else {
        "Collapse workflow feed"
    }));
    set_accessible_button_text(
        &ui.collapse_button,
        if collapsed {
            "Expand workflow feed"
        } else {
            "Collapse workflow feed"
        },
        None,
    );
}

pub(super) fn refresh_orchestration_feed(ui: &OrchestrationFeedUi, state: &SocketAppState) {
    let filter = feed_filter_from_index(ui.filter.get());
    let workspace_id = active_workspace_id_for_state(state);
    let cleared_before_ms =
        feed_clear_cutoffs_for_workspace(&ui.cleared_before_ms.borrow(), workspace_id.as_deref());
    let lines =
        orchestration_feed_lines_for_filter_in_workspace(state, filter, workspace_id.as_deref())
            .into_iter()
            .filter(|line| feed_line_visible_after_clear(line, filter, &cleared_before_ms))
            .collect::<Vec<_>>();
    for (index, row) in ui.rows.iter().enumerate() {
        if let Some(line) = lines.get(index) {
            row.shell.set_visible(true);
            set_feed_label(&row.time, &clock_time_label(line.at_ms));
            set_feed_label(&row.body, &line.body);
            set_feed_label(&row.status, &line.status);
            set_feed_status_class(&row.status, &line.status);
        } else if index == 0 {
            row.shell.set_visible(true);
            set_feed_label(&row.time, "");
            set_feed_label(&row.body, feed_empty_body(filter));
            set_feed_label(&row.status, "idle");
            set_feed_status_class(&row.status, "idle");
        } else {
            row.shell.set_visible(false);
        }
    }
}

fn feed_filter_from_index(index: u8) -> FeedFilter {
    match index {
        1 => FeedFilter::Attention,
        2 => FeedFilter::Events,
        3 => FeedFilter::Logs,
        _ => FeedFilter::All,
    }
}

fn feed_filter_slot(filter: FeedFilter) -> usize {
    match filter {
        FeedFilter::All => 0,
        FeedFilter::Attention => 1,
        FeedFilter::Events => 2,
        FeedFilter::Logs => 3,
    }
}

fn set_feed_clear_cutoff(
    cutoffs: &mut FeedClearCutoffs,
    workspace_id: Option<&str>,
    filter: FeedFilter,
    at_ms: u128,
) {
    cutoffs
        .entry(workspace_id.map(str::to_string))
        .or_insert([0; FEED_FILTER_COUNT])[feed_filter_slot(filter)] = at_ms;
}

fn feed_clear_cutoff(
    cutoffs: &FeedClearCutoffs,
    workspace_id: Option<&str>,
    filter: FeedFilter,
) -> u128 {
    cutoffs
        .get(&workspace_id.map(str::to_string))
        .map(|values| values[feed_filter_slot(filter)])
        .unwrap_or_default()
}

fn feed_clear_cutoffs_for_workspace(
    cutoffs: &FeedClearCutoffs,
    workspace_id: Option<&str>,
) -> [u128; FEED_FILTER_COUNT] {
    let mut values = [0; FEED_FILTER_COUNT];
    for filter in [
        FeedFilter::All,
        FeedFilter::Attention,
        FeedFilter::Events,
        FeedFilter::Logs,
    ] {
        values[feed_filter_slot(filter)] = feed_clear_cutoff(cutoffs, workspace_id, filter);
    }
    values
}

fn feed_empty_body(filter: FeedFilter) -> &'static str {
    match filter {
        FeedFilter::All => "No feed activity yet",
        FeedFilter::Attention => "No attention items",
        FeedFilter::Events => "No workflow or team events yet",
        FeedFilter::Logs => "No logs yet",
    }
}

fn feed_line_visible_after_clear(
    line: &FeedLine,
    filter: FeedFilter,
    cleared_before_ms: &[u128; FEED_FILTER_COUNT],
) -> bool {
    line.at_ms > cleared_before_ms[feed_filter_slot(filter)] && feed_line_matches(line, filter)
}

fn feed_line_matches(line: &FeedLine, filter: FeedFilter) -> bool {
    match filter {
        FeedFilter::All => true,
        FeedFilter::Attention => feed_line_needs_attention(line),
        FeedFilter::Events => matches!(line.source, FeedSource::Workflow | FeedSource::Team),
        FeedFilter::Logs => matches!(line.source, FeedSource::Log),
    }
}

fn feed_line_needs_attention(line: &FeedLine) -> bool {
    matches!(feed_status_class(&line.status), "err" | "warn")
        || text_mentions_attention(&line.body)
        || text_mentions_attention(&line.status)
}

fn text_mentions_attention(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "needs input",
        "need input",
        "approval",
        "pending",
        "blocked",
        "failed",
        "failure",
        "error",
        "warning",
        "stale",
        "conflict",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn set_feed_label(label: &gtk::Label, value: &str) {
    if label.label() != value {
        label.set_label(value);
        label.set_tooltip_text((!value.is_empty()).then_some(value));
    }
}

/// Maps a feed status word to the chip color class used by style.css.
pub(super) fn feed_status_class(status: &str) -> &'static str {
    let status = status.to_ascii_lowercase();
    if status.contains("fail") || status.contains("error") || status.contains("no_heartbeat") {
        "err"
    } else if status.contains("shutdown") || status.contains("stopped") || status == "idle" {
        "idle"
    } else if status.contains("run") || status.contains("start") || status.contains("active") {
        "run"
    } else if status.contains("done") || status.contains("pass") || status.contains("complete") {
        "ok"
    } else if status == "warn"
        || status.contains("warning")
        || status.contains("approval")
        || status.contains("prompt")
        || status.contains("pend")
    {
        "warn"
    } else {
        "info"
    }
}

fn set_feed_status_class(label: &gtk::Label, status: &str) {
    for class_name in ["ok", "run", "warn", "err", "info", "idle"] {
        label.remove_css_class(class_name);
    }
    label.add_css_class(feed_status_class(status));
}

fn orchestration_feed_lines_for_filter_in_workspace(
    state: &SocketAppState,
    filter: FeedFilter,
    active_workspace_id: Option<&str>,
) -> Vec<FeedLine> {
    let event_scan_limit = if filter == FeedFilter::Attention {
        FEED_ATTENTION_SCAN_LIMIT
    } else {
        FEED_ROW_COUNT
    };
    let mut lines = if matches!(
        filter,
        FeedFilter::All | FeedFilter::Attention | FeedFilter::Events
    ) {
        orchestration_event_lines_for_workspace(
            state.workflow_store_path.as_deref(),
            state.team_store_path.as_deref(),
            active_workspace_id,
            event_scan_limit,
        )
    } else {
        Vec::new()
    };
    if let Ok(model) = state.model.lock() {
        if matches!(filter, FeedFilter::All | FeedFilter::Attention) {
            lines.extend(feed_notification_lines_for_workspace_with_limit(
                &model,
                active_workspace_id,
                event_scan_limit,
            ));
        }
        if let Some(workspace_id) = active_workspace_id {
            lines = append_log_lines_for_filter(lines, &model.list_logs(workspace_id), filter);
        }
    }
    finalize_feed_lines(
        lines
            .into_iter()
            .filter(|line| feed_line_matches(line, filter))
            .collect(),
    )
}

fn combined_feed_log_limit(has_non_log_rows: bool) -> usize {
    if has_non_log_rows {
        FEED_LOG_ROWS_WHEN_EVENTS_EXIST
    } else {
        FEED_ROW_COUNT
    }
}

fn append_log_lines(lines: &mut Vec<FeedLine>, logs: &[forktty_core::LogEntry]) {
    let has_non_log_rows = lines.iter().any(|line| {
        matches!(
            line.source,
            FeedSource::Workflow | FeedSource::Team | FeedSource::Notification
        )
    });
    let log_limit = combined_feed_log_limit(has_non_log_rows);
    lines.extend(logs.iter().take(log_limit).map(|log| FeedLine {
        at_ms: log.timestamp_ms,
        source: FeedSource::Log,
        body: format!("Log: {}", log.message.trim()),
        status: feed_log_level(log.level).to_string(),
    }));
}

fn append_log_lines_for_filter(
    mut lines: Vec<FeedLine>,
    logs: &[forktty_core::LogEntry],
    filter: FeedFilter,
) -> Vec<FeedLine> {
    match filter {
        FeedFilter::All => {
            append_log_lines(&mut lines, logs);
            lines
        }
        FeedFilter::Events => lines,
        FeedFilter::Attention => {
            lines.extend(
                logs.iter()
                    .take(FEED_ATTENTION_SCAN_LIMIT)
                    .map(|log| FeedLine {
                        at_ms: log.timestamp_ms,
                        source: FeedSource::Log,
                        body: format!("Log: {}", log.message.trim()),
                        status: feed_log_level(log.level).to_string(),
                    }),
            );
            lines
        }
        FeedFilter::Logs => logs
            .iter()
            .take(FEED_ROW_COUNT)
            .map(|log| FeedLine {
                at_ms: log.timestamp_ms,
                source: FeedSource::Log,
                body: format!("Log: {}", log.message.trim()),
                status: feed_log_level(log.level).to_string(),
            })
            .collect(),
    }
}

#[cfg(test)]
fn feed_notification_lines_for_workspace(
    model: &forktty_core::WorkspaceModel,
    workspace_id: Option<&str>,
) -> Vec<FeedLine> {
    feed_notification_lines_for_workspace_with_limit(model, workspace_id, FEED_ROW_COUNT)
}

fn feed_notification_lines_for_workspace_with_limit(
    model: &forktty_core::WorkspaceModel,
    workspace_id: Option<&str>,
    limit: usize,
) -> Vec<FeedLine> {
    model
        .list_notifications()
        .into_iter()
        .filter(|notification| feed_notification_matches_workspace(notification, workspace_id))
        .filter(|notification| {
            notification
                .surface_id
                .as_deref()
                .is_none_or(|surface_id| model.surface(surface_id).is_some())
        })
        .rev()
        .take(limit)
        .map(|notification| FeedLine {
            at_ms: notification.created_at_ms,
            source: FeedSource::Notification,
            body: notification.title.trim().to_string(),
            status: feed_notification_kind(notification.kind).to_string(),
        })
        .collect()
}

fn feed_notification_matches_workspace(
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

fn finalize_feed_lines(mut lines: Vec<FeedLine>) -> Vec<FeedLine> {
    lines.sort_by_key(|line| std::cmp::Reverse(line.at_ms));
    lines.truncate(FEED_ROW_COUNT);
    lines
}

#[cfg(test)]
fn orchestration_feed_lines_for_workspace(
    workflow_path: Option<&Path>,
    team_path: Option<&Path>,
    workspace_id: Option<&str>,
) -> Vec<FeedLine> {
    finalize_feed_lines(orchestration_event_lines_for_workspace(
        workflow_path,
        team_path,
        workspace_id,
        FEED_ROW_COUNT,
    ))
}

fn orchestration_event_lines_for_workspace(
    workflow_path: Option<&Path>,
    team_path: Option<&Path>,
    workspace_id: Option<&str>,
    per_store_limit: usize,
) -> Vec<FeedLine> {
    let mut lines = Vec::new();
    if let Some(path) = workflow_path {
        if let Ok(store) = forktty_core::load_workflows_from_path(path) {
            let workflow_workspaces = store
                .workflows
                .iter()
                .map(|workflow| (workflow.id.as_str(), workflow.workspace_id.as_deref()))
                .collect::<std::collections::HashMap<_, _>>();
            lines.extend(
                store
                    .events
                    .iter()
                    .rev()
                    .filter(|event| {
                        workflow_workspaces
                            .get(event.workflow_id.as_str())
                            .is_some_and(|record_workspace| {
                                record_matches_workspace(*record_workspace, workspace_id)
                            })
                    })
                    .take(per_store_limit)
                    .map(|event| FeedLine {
                        at_ms: u128::from(event.at_ms),
                        source: FeedSource::Workflow,
                        body: format!("Router: {}", event.summary),
                        status: feed_status_label(&event.kind),
                    }),
            );
        }
    }
    if let Some(path) = team_path {
        if let Ok(store) = forktty_core::load_teams_from_path(path) {
            let team_workspaces = store
                .teams
                .iter()
                .map(|team| (team.id.as_str(), team.workspace_id.as_deref()))
                .collect::<std::collections::HashMap<_, _>>();
            lines.extend(
                store
                    .events
                    .iter()
                    .rev()
                    .filter(|event| {
                        team_workspaces.get(event.team_id.as_str()).is_some_and(
                            |record_workspace| {
                                record_matches_workspace(*record_workspace, workspace_id)
                            },
                        )
                    })
                    .take(per_store_limit)
                    .map(|event| FeedLine {
                        at_ms: u128::from(event.created_at_ms),
                        source: FeedSource::Team,
                        body: format!("Team: {}", event.summary),
                        status: feed_status_label(&event.kind),
                    }),
            );
        }
    }
    lines
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
        Some(workspace_id) => record_workspace_id == Some(workspace_id),
        None => record_workspace_id.is_none(),
    }
}

fn feed_log_level(level: forktty_core::LogLevel) -> &'static str {
    match level {
        forktty_core::LogLevel::Info => "info",
        forktty_core::LogLevel::Warn => "warn",
        forktty_core::LogLevel::Error => "error",
    }
}

fn feed_notification_kind(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "approval",
        NotificationKind::Error => "error",
        NotificationKind::Info => "info",
        NotificationKind::Custom => "note",
    }
}

fn feed_status_label(kind: &str) -> String {
    match kind {
        "workflow.updated" | "workflow.upserted" => "workflow".to_string(),
        "workflow.loop.set" | "workflow.loop_set" => "loop".to_string(),
        "team.upserted" => "team".to_string(),
        "team.task.upserted" => "task".to_string(),
        "team.worker.launched" | "team.worker.upserted" => "worker".to_string(),
        "team.worker.shutdown_requested" => "shutdown".to_string(),
        "team.message.sent" => "message".to_string(),
        "team.message.acked" => "acked".to_string(),
        other => other.rsplit('.').next().unwrap_or(other).replace('_', " "),
    }
}

/// Wall-clock HH:MM:SS for feed rows, matching terminal log conventions.
fn clock_time_label(at_ms: u128) -> String {
    let seconds = (at_ms / 1_000).min(i64::MAX as u128) as i64;
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|datetime| datetime.format("%H:%M:%S").ok())
        .map(|formatted| formatted.to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

pub(super) fn relative_time_label(now_ms: u128, at_ms: u128) -> String {
    let age_ms = now_ms.saturating_sub(at_ms);
    let seconds = age_ms / 1_000;
    if seconds < 5 {
        "now".to_string()
    } else if seconds < 60 {
        format!("{seconds}s ago")
    } else {
        let minutes = seconds / 60;
        if minutes < 60 {
            format!("{minutes}m ago")
        } else if minutes < 60 * 24 {
            format!("{}h ago", minutes / 60)
        } else {
            format!("{}d ago", minutes / (60 * 24))
        }
    }
}

pub(super) fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_label_uses_compact_units() {
        assert_eq!(relative_time_label(10_000, 9_000), "now");
        assert_eq!(relative_time_label(10_000, 4_000), "6s ago");
        assert_eq!(relative_time_label(125_000, 5_000), "2m ago");
        assert_eq!(relative_time_label(7_205_000, 5_000), "2h ago");
        assert_eq!(relative_time_label(180_000_000, 0), "2d ago");
    }

    #[test]
    fn feed_status_class_maps_event_kinds_to_chip_colors() {
        assert_eq!(feed_status_class("running"), "run");
        assert_eq!(feed_status_class("started"), "run");
        assert_eq!(feed_status_class("done"), "ok");
        assert_eq!(feed_status_class("tests passed"), "ok");
        assert_eq!(feed_status_class("no_heartbeat"), "err");
        assert_eq!(feed_status_class("error"), "err");
        assert_eq!(feed_status_class("warn"), "warn");
        assert_eq!(feed_status_class("approval"), "warn");
        assert_eq!(feed_status_class("info"), "info");
        assert_eq!(feed_status_class("note"), "info");
        assert_eq!(feed_status_class("shutdown"), "idle");
        assert_eq!(feed_status_class("shutdown_requested"), "idle");
        assert_eq!(feed_status_class("idle"), "idle");
    }

    #[test]
    fn feed_status_label_compacts_event_kinds() {
        assert_eq!(
            feed_status_label("team.worker.shutdown_requested"),
            "shutdown"
        );
        assert_eq!(feed_status_label("team.task.upserted"), "task");
        assert_eq!(feed_status_label("custom.long_event"), "long event");
    }

    #[test]
    fn feed_filters_split_events_from_notification_logs() {
        let workflow = FeedLine {
            at_ms: 1,
            source: FeedSource::Workflow,
            body: "Router: loop planned".to_string(),
            status: "info".to_string(),
        };
        let team = FeedLine {
            at_ms: 2,
            source: FeedSource::Team,
            body: "Team: worker started".to_string(),
            status: "running".to_string(),
        };
        let log = FeedLine {
            at_ms: 3,
            source: FeedSource::Log,
            body: "tests passed".to_string(),
            status: "info".to_string(),
        };
        assert!(feed_line_matches(&workflow, FeedFilter::All));
        assert!(feed_line_matches(&workflow, FeedFilter::Events));
        assert!(!feed_line_matches(&workflow, FeedFilter::Logs));
        assert!(feed_line_matches(&team, FeedFilter::Events));
        assert!(!feed_line_matches(&log, FeedFilter::Events));
        assert!(feed_line_matches(&log, FeedFilter::Logs));
    }

    #[test]
    fn attention_filter_surfaces_only_rows_requiring_user_attention() {
        let approval = FeedLine {
            at_ms: 1,
            source: FeedSource::Notification,
            body: "Claude needs input".to_string(),
            status: "approval".to_string(),
        };
        let error = FeedLine {
            at_ms: 2,
            source: FeedSource::Log,
            body: "Log: Antigravity tool error".to_string(),
            status: "error".to_string(),
        };
        let warning = FeedLine {
            at_ms: 3,
            source: FeedSource::Log,
            body: "Log: quota warning".to_string(),
            status: "warn".to_string(),
        };
        let needs_input = FeedLine {
            at_ms: 4,
            source: FeedSource::Team,
            body: "Team: worker needs input".to_string(),
            status: "worker".to_string(),
        };
        let normal_event = FeedLine {
            at_ms: 5,
            source: FeedSource::Workflow,
            body: "Router: review done".to_string(),
            status: "workflow".to_string(),
        };
        let info_log = FeedLine {
            at_ms: 6,
            source: FeedSource::Log,
            body: "Log: provider request started".to_string(),
            status: "info".to_string(),
        };

        assert!(feed_line_matches(&approval, FeedFilter::Attention));
        assert!(feed_line_matches(&error, FeedFilter::Attention));
        assert!(feed_line_matches(&warning, FeedFilter::Attention));
        assert!(feed_line_matches(&needs_input, FeedFilter::Attention));
        assert!(!feed_line_matches(&normal_event, FeedFilter::Attention));
        assert!(!feed_line_matches(&info_log, FeedFilter::Attention));
    }

    #[test]
    fn logs_filter_does_not_treat_notifications_as_logs() {
        let notification = FeedLine {
            at_ms: 1,
            source: FeedSource::Notification,
            body: "Claude needs input".to_string(),
            status: "approval".to_string(),
        };

        assert!(feed_line_matches(&notification, FeedFilter::All));
        assert!(!feed_line_matches(&notification, FeedFilter::Events));
        assert!(!feed_line_matches(&notification, FeedFilter::Logs));
    }

    #[test]
    fn feed_empty_body_matches_active_filter() {
        assert_eq!(feed_empty_body(FeedFilter::All), "No feed activity yet");
        assert_eq!(feed_empty_body(FeedFilter::Attention), "No attention items");
        assert_eq!(
            feed_empty_body(FeedFilter::Events),
            "No workflow or team events yet"
        );
        assert_eq!(feed_empty_body(FeedFilter::Logs), "No logs yet");
    }

    #[test]
    fn feed_clear_threshold_is_filter_specific() {
        let workflow = FeedLine {
            at_ms: 10,
            source: FeedSource::Workflow,
            body: "Router: done".to_string(),
            status: "workflow".to_string(),
        };
        let log = FeedLine {
            at_ms: 10,
            source: FeedSource::Log,
            body: "Log: noisy provider trace".to_string(),
            status: "info".to_string(),
        };
        let cleared_before_ms = [0, 0, 0, 20];

        assert!(feed_line_visible_after_clear(
            &workflow,
            FeedFilter::Events,
            &cleared_before_ms
        ));
        assert!(feed_line_visible_after_clear(
            &log,
            FeedFilter::All,
            &cleared_before_ms
        ));
        assert!(!feed_line_visible_after_clear(
            &log,
            FeedFilter::Logs,
            &cleared_before_ms
        ));
    }

    #[test]
    fn feed_clear_threshold_is_workspace_specific() {
        let mut cutoffs = FeedClearCutoffs::default();
        set_feed_clear_cutoff(&mut cutoffs, Some("workspace-a"), FeedFilter::All, 20);
        set_feed_clear_cutoff(&mut cutoffs, Some("workspace-a"), FeedFilter::Logs, 30);

        assert_eq!(
            feed_clear_cutoff(&cutoffs, Some("workspace-a"), FeedFilter::All),
            20
        );
        assert_eq!(
            feed_clear_cutoff(&cutoffs, Some("workspace-a"), FeedFilter::Logs),
            30
        );
        assert_eq!(
            feed_clear_cutoff(&cutoffs, Some("workspace-b"), FeedFilter::All),
            0
        );
        assert_eq!(feed_clear_cutoff(&cutoffs, None, FeedFilter::All), 0);

        let workflow = FeedLine {
            at_ms: 10,
            source: FeedSource::Workflow,
            body: "Router: done".to_string(),
            status: "workflow".to_string(),
        };
        let workspace_a_cutoff = [
            feed_clear_cutoff(&cutoffs, Some("workspace-a"), FeedFilter::All),
            feed_clear_cutoff(&cutoffs, Some("workspace-a"), FeedFilter::Attention),
            feed_clear_cutoff(&cutoffs, Some("workspace-a"), FeedFilter::Events),
            feed_clear_cutoff(&cutoffs, Some("workspace-a"), FeedFilter::Logs),
        ];
        let workspace_b_cutoff = [
            feed_clear_cutoff(&cutoffs, Some("workspace-b"), FeedFilter::All),
            feed_clear_cutoff(&cutoffs, Some("workspace-b"), FeedFilter::Attention),
            feed_clear_cutoff(&cutoffs, Some("workspace-b"), FeedFilter::Events),
            feed_clear_cutoff(&cutoffs, Some("workspace-b"), FeedFilter::Logs),
        ];

        assert!(!feed_line_visible_after_clear(
            &workflow,
            FeedFilter::All,
            &workspace_a_cutoff
        ));
        assert!(feed_line_visible_after_clear(
            &workflow,
            FeedFilter::All,
            &workspace_b_cutoff
        ));
    }

    #[test]
    fn combined_feed_caps_logs_when_events_or_notifications_exist() {
        assert_eq!(combined_feed_log_limit(false), FEED_ROW_COUNT);
        assert_eq!(
            combined_feed_log_limit(true),
            FEED_LOG_ROWS_WHEN_EVENTS_EXIST
        );

        let mut lines = vec![FeedLine {
            at_ms: 1,
            source: FeedSource::Workflow,
            body: "Router: running".to_string(),
            status: "workflow".to_string(),
        }];
        let logs = (0..FEED_ROW_COUNT)
            .map(|index| forktty_core::LogEntry {
                id: format!("log-{index}"),
                timestamp_ms: 100 + index as u128,
                level: forktty_core::LogLevel::Info,
                message: format!("Antigravity request {index}"),
            })
            .collect::<Vec<_>>();

        append_log_lines(&mut lines, &logs);
        let lines = finalize_feed_lines(lines);

        assert!(lines.iter().any(|line| line.source == FeedSource::Workflow));
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.source == FeedSource::Log)
                .count(),
            FEED_LOG_ROWS_WHEN_EVENTS_EXIST
        );
    }

    #[test]
    fn feed_lines_ignore_events_from_inactive_workspaces() {
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

        let lines = orchestration_feed_lines_for_workspace(
            Some(&workflow_path),
            Some(&team_path),
            Some("workspace-main"),
        );

        assert!(lines.is_empty());
    }

    #[test]
    fn feed_notification_lines_ignore_inactive_and_closed_surface_notifications() {
        let mut model = forktty_core::WorkspaceModel::new();
        let active = model.create_workspace("main", "/tmp/main");
        let active_surface_id = active.focused_surface_id.clone();
        let inactive = model.create_workspace("other", "/tmp/other");
        let inactive_surface_id = inactive.focused_surface_id.clone();
        model.focus_surface_and_select_workspace(&active_surface_id);

        model.create_notification("Global notice", "", NotificationKind::Info, None, None);
        model.create_notification(
            "Active notice",
            "",
            NotificationKind::Prompt,
            Some(active.id.clone()),
            Some(active_surface_id.clone()),
        );
        model.create_notification(
            "Inactive notice",
            "",
            NotificationKind::Error,
            Some(inactive.id.clone()),
            Some(inactive_surface_id),
        );
        model.create_notification(
            "Closed surface notice",
            "",
            NotificationKind::Prompt,
            Some(active.id.clone()),
            Some("surface-missing".to_string()),
        );

        let lines = feed_notification_lines_for_workspace(&model, Some(&active.id));
        let titles = lines
            .iter()
            .map(|line| line.body.as_str())
            .collect::<Vec<_>>();

        assert_eq!(titles, vec!["Active notice", "Global notice"]);
    }

    #[test]
    fn logs_filter_uses_full_log_capacity_even_when_events_exist() {
        let logs = (0..FEED_ROW_COUNT)
            .map(|index| forktty_core::LogEntry {
                id: format!("log-{index}"),
                timestamp_ms: 100 + index as u128,
                level: forktty_core::LogLevel::Info,
                message: format!("Antigravity request {index}"),
            })
            .collect::<Vec<_>>();
        let lines = vec![FeedLine {
            at_ms: 1,
            source: FeedSource::Workflow,
            body: "Router: running".to_string(),
            status: "workflow".to_string(),
        }];

        let lines = append_log_lines_for_filter(lines, &logs, FeedFilter::Logs);

        assert_eq!(lines.len(), FEED_ROW_COUNT);
        assert!(lines.iter().all(|line| line.source == FeedSource::Log));
    }
}
