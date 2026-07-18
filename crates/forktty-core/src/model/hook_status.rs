//! Ordering and merge rules for status updates reported by agent hooks.

use super::StatusHookMetadata;

pub(super) const HOOK_TERMINAL_PROMPT_GUARD_NS: u128 = 2_000_000_000;

pub(super) fn should_ignore_hook_status(
    current: Option<&StatusHookMetadata>,
    incoming: &StatusHookMetadata,
) -> bool {
    let Some(current) = current else {
        return false;
    };
    if current.session_id != incoming.session_id {
        return false;
    }
    if let (Some(incoming_order), Some(current_order)) = (incoming.order, current.order) {
        // Orders are only comparable when both sides used the same clock; a
        // stored order from a different clock (e.g. a wall-clock order kept
        // from before an upgrade to boottime ordering) must not drop newer
        // updates forever, so mismatched clocks accept the incoming update.
        if same_order_clock(current, incoming) && incoming_order < current_order {
            return true;
        }
    }
    should_ignore_hook_status_after_serialized_ordering(Some(current), incoming)
}

pub(super) fn should_ignore_hook_status_after_serialized_ordering(
    current: Option<&StatusHookMetadata>,
    incoming: &StatusHookMetadata,
) -> bool {
    let Some(current) = current else {
        return false;
    };
    if current.session_id != incoming.session_id {
        return false;
    }
    if incoming.event == "prompt-submit" && is_terminal_hook_event(&current.event) {
        if incoming
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| current.turn_id.as_deref() == Some(turn_id))
        {
            return true;
        }
        if incoming.turn_id.is_none()
            && same_monotonic_clock(current, incoming)
            && incoming
                .order
                .zip(current.order)
                .is_some_and(|(incoming_order, current_order)| {
                    incoming_order >= current_order
                        && incoming_order - current_order <= HOOK_TERMINAL_PROMPT_GUARD_NS
                })
        {
            return true;
        }
    }
    false
}

pub(super) fn merge_hook_metadata(
    current: Option<&StatusHookMetadata>,
    mut incoming: StatusHookMetadata,
) -> StatusHookMetadata {
    if is_terminal_hook_event(&incoming.event)
        && incoming.turn_id.is_none()
        && current.is_some_and(|current| current.session_id == incoming.session_id)
    {
        incoming.turn_id = current.and_then(|current| current.turn_id.clone());
    }
    incoming
}

fn is_terminal_hook_event(event: &str) -> bool {
    matches!(event, "stop" | "stop-failure" | "session-end")
}

fn same_order_clock(current: &StatusHookMetadata, incoming: &StatusHookMetadata) -> bool {
    current.clock == incoming.clock
}

fn same_monotonic_clock(current: &StatusHookMetadata, incoming: &StatusHookMetadata) -> bool {
    const MONOTONIC_CLOCKS: &[&str] = &["monotonic-ns", "boottime-ns"];
    match (current.clock.as_deref(), incoming.clock.as_deref()) {
        (Some(current), Some(incoming)) => {
            current == incoming && MONOTONIC_CLOCKS.contains(&current)
        }
        _ => false,
    }
}
