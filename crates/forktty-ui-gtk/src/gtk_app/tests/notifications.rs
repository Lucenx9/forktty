//! Notification panel styling regression tests.

use super::*;

#[test]
fn notification_panel_truncated_labels_keep_full_tooltips() {
    let source = include_str!("../notifications_panel.rs");

    assert!(source.contains(".tooltip_text(&notification.title)"));
    assert!(source.contains(".tooltip_text(target)"));
}

#[test]
fn notification_panel_uses_human_custom_kind_label() {
    assert_eq!(notification_kind_label(NotificationKind::Custom), "App");
}

#[test]
fn notification_panel_terminal_action_buttons_have_accessible_names() {
    let source = include_str!("../notifications_panel.rs");

    assert!(source.contains("set_accessible_button_text(&button, label, None);"));
}

#[test]
fn notification_panel_css_only_styles_real_kind_classes() {
    let source = include_str!("../../style.css");

    assert!(source.contains(".notification-actions"));
    assert!(source.contains(".notification-row.actionable.unread"));
    assert!(source.contains(".notification-row.current.unread"));
    assert!(!source.contains(".notification-kind.success"));
    assert!(!source.contains(".notification-kind.warning"));
}

#[test]
fn notification_panel_css_matches_quiet_agent_hud_tone() {
    let source = include_str!("../../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(block("\n.notification-row {").contains("border: 1px solid transparent;"));
    assert!(block("\n.notification-row {").contains("background: @ft_bg_control;"));
    assert!(block(".notification-actions {").contains("border-top: 1px solid transparent;"));

    let kind = block(".notification-kind {");
    assert!(!kind.contains("text-transform: uppercase;"));
    assert!(!kind.contains("font-weight: 700;"));

    // After further quieting: actionable hovers use neutral gray; focus uses light ring not thick bars
    assert!(
        block(".notification-list row:hover .notification-row.actionable {")
            .contains("background: @ft_bg_selected;")
    );
    assert!(
        block(".notification-list row:focus-visible .notification-row.unread {")
            .contains("0 0 0 1px alpha(@accent_color, 0.12)")
    );
    assert!(block(".notification-kind.prompt {").contains("color: @ft_warning;"));
}
