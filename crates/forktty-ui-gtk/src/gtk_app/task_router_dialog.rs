//! Read-only task router planner dialog for the GTK workbench.

use super::*;
use serde_json::{json, Value};

pub(super) fn show_task_router_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Router")
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(560)
        .default_height(430)
        .build();
    dialog.add_css_class("ft-dialog");
    dialog.add_css_class("task-router-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Router").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Plan the visible workflow before choosing workers.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("ft-dialog-body");
    body.add_css_class("task-router-body");

    let goal = gtk::Entry::builder()
        .placeholder_text("Describe the task")
        .hexpand(true)
        .build();
    goal.add_css_class("task-router-goal");
    goal.update_property(&[gtk::accessible::Property::Label("Task goal")]);

    let hints = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hints.add_css_class("task-router-hints");
    let review = gtk::CheckButton::with_label("Review");
    review.set_tooltip_text(Some("Prefer reviewer-oriented routing"));
    let parallel = gtk::CheckButton::with_label("Parallel");
    parallel.set_tooltip_text(Some("Prefer multi-worker routing when safe"));
    hints.append(&review);
    hints.append(&parallel);

    let result = gtk::Box::new(gtk::Orientation::Vertical, 0);
    result.add_css_class("task-router-result");
    let strategy = task_router_result_row(&result, "strategy", false);
    let profile = task_router_result_row(&result, "profile", false);
    let approvals = task_router_result_row(&result, "approvals", false);
    let assignments = task_router_result_row(&result, "assignments", true);
    let reasons = task_router_result_row(&result, "reason", true);
    let status = gtk::Label::builder()
        .label("No plan yet")
        .xalign(0.0)
        .wrap(true)
        .build();
    status.add_css_class("task-router-status");

    body.append(&goal);
    body.append(&hints);
    body.append(&result);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let close = gtk::Button::with_label("Close");
    close.add_css_class("settings-inline-action");
    close.add_css_class("subtle");
    set_accessible_button_text(&close, "Close Router", Some("Esc"));
    let plan = gtk::Button::with_label("Plan");
    plan.add_css_class("settings-inline-action");
    plan.add_css_class("primary");
    set_accessible_button_text(&plan, "Plan task strategy", None);
    footer.append(&spacer);
    footer.append(&close);
    footer.append(&plan);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    root.append(&footer);
    dialog.set_default_widget(Some(&plan));
    dialog.set_child(Some(&root));

    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| dialog.close());
    }
    {
        let plan = plan.clone();
        goal.connect_activate(move |_| plan.emit_clicked());
    }
    {
        let state = state.clone();
        let goal = goal.clone();
        let review = review.clone();
        let parallel = parallel.clone();
        plan.connect_clicked(move |button| {
            let goal_text = goal.text().trim().to_string();
            if goal_text.is_empty() {
                status.set_label("Enter a task goal first.");
                goal.grab_focus();
                return;
            }
            button.set_sensitive(false);
            status.set_label("Planning...");
            let state = state.clone();
            let status = status.clone();
            let strategy = strategy.clone();
            let profile = profile.clone();
            let approvals = approvals.clone();
            let assignments = assignments.clone();
            let reasons = reasons.clone();
            let button = button.clone();
            let review_requested = review.is_active();
            let parallel_requested = parallel.is_active();
            glib::spawn_future_local(async move {
                let mut params = json!({ "goal": goal_text });
                if review_requested {
                    params["user_requested_review"] = json!(true);
                }
                if parallel_requested {
                    params["user_requested_parallelism"] = json!(true);
                }
                match forktty_socket::dispatch(&state, "task.strategy.plan", params).await {
                    Ok(value) => {
                        let summary = format_task_router_plan_summary(&value);
                        strategy.set_label(&summary.strategy);
                        profile.set_label(&summary.profile);
                        approvals.set_label(&summary.approvals);
                        set_task_router_multiline_result(&assignments, &summary.assignments);
                        set_task_router_multiline_result(&reasons, &summary.reason);
                        status.set_label("Plan ready. Apply remains explicit through CLI, MCP, or a reviewed agent action.");
                    }
                    Err(err) => {
                        status.set_label(&format!("Planning failed: {err}"));
                    }
                }
                button.set_sensitive(true);
            });
        });
    }

    dialog.present();
    goal.grab_focus();
}

fn task_router_result_row(parent: &gtk::Box, label: &str, multiline: bool) -> gtk::Label {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("task-router-result-row");
    let key = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    key.add_css_class("task-router-result-key");
    let value = gtk::Label::builder().label("-").hexpand(true).build();
    value.set_xalign(1.0);
    if multiline {
        value.set_wrap(true);
        value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        value.set_lines(2);
        value.set_max_width_chars(44);
    } else {
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        value.set_single_line_mode(true);
    }
    value.add_css_class("task-router-result-value");
    row.append(&key);
    row.append(&value);
    parent.append(&row);
    value
}

fn set_task_router_multiline_result(label: &gtk::Label, text: &str) {
    label.set_xalign(0.0);
    label.set_label(text);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskRouterPlanSummary {
    strategy: String,
    profile: String,
    approvals: String,
    assignments: String,
    reason: String,
}

fn format_task_router_plan_summary(value: &Value) -> TaskRouterPlanSummary {
    let strategy = value
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let task_class = value
        .get("task_class")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let profile = value
        .get("router_profile")
        .or_else(|| value.get("selected_router_profile"))
        .and_then(Value::as_str)
        .unwrap_or("balanced");
    let approvals = value
        .get("approvals")
        .and_then(Value::as_array)
        .map(|items| {
            if items.is_empty() {
                "none".to_string()
            } else {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        })
        .unwrap_or_else(|| "none".to_string());
    let assignments = value
        .get("assignments")
        .and_then(Value::as_array)
        .map(|assignments| format_assignment_summary(assignments))
        .unwrap_or_else(|| "none".to_string());
    let reason = value
        .get("reasons")
        .and_then(Value::as_array)
        .map(|items| {
            let reasons = items
                .iter()
                .filter_map(Value::as_str)
                .take(2)
                .collect::<Vec<_>>();
            if reasons.is_empty() {
                "-".to_string()
            } else {
                reasons.join("; ")
            }
        })
        .unwrap_or_else(|| "-".to_string());

    TaskRouterPlanSummary {
        strategy: format!("{strategy} ({task_class})"),
        profile: profile.to_string(),
        approvals,
        assignments,
        reason,
    }
}

fn format_assignment_summary(assignments: &[Value]) -> String {
    let summary = assignments
        .iter()
        .filter_map(|assignment| {
            let role = assignment.get("role")?.as_str()?;
            let harness = assignment.get("harness_id")?.as_str()?;
            Some(format!("{role}:{harness}"))
        })
        .collect::<Vec<_>>();
    if summary.is_empty() {
        "none".to_string()
    } else {
        summary.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_router_plan_summary_formats_assignments_and_approvals() {
        let summary = format_task_router_plan_summary(&json!({
            "strategy": "parallel_research",
            "task_class": "parallel_research",
            "router_profile": "parallel",
            "approvals": ["start_run", "launch_parallel_workers"],
            "assignments": [
                {"role": "researcher", "harness_id": "codex"},
                {"role": "reviewer", "harness_id": "grok"}
            ],
            "reasons": ["parallel intent", "ready harnesses"]
        }));

        assert_eq!(summary.strategy, "parallel_research (parallel_research)");
        assert_eq!(summary.profile, "parallel");
        assert_eq!(summary.approvals, "start_run, launch_parallel_workers");
        assert_eq!(summary.assignments, "researcher:codex, reviewer:grok");
        assert_eq!(summary.reason, "parallel intent; ready harnesses");
    }

    #[test]
    fn task_router_plan_summary_handles_sparse_plan_json() {
        let summary = format_task_router_plan_summary(&json!({}));

        assert_eq!(summary.strategy, "unknown (unknown)");
        assert_eq!(summary.profile, "balanced");
        assert_eq!(summary.approvals, "none");
        assert_eq!(summary.assignments, "none");
        assert_eq!(summary.reason, "-");
    }
}
