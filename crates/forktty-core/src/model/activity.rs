//! Runtime activity metadata on top of the workspace/surface graph.

use std::collections::BTreeSet;

use crate::agents::{agent_metadata_aliases, AgentKind};

use super::hook_status::{
    merge_hook_metadata, should_ignore_hook_status,
    should_ignore_hook_status_after_serialized_ordering,
};
use super::{
    AgentSession, AgentSessionLifecycle, HookPromptMetadata, HookPromptResolution, LogEntry,
    LogLevel, NotificationCreation, NotificationItem, NotificationKind, ProgressEntry, StatusEntry,
    StatusHookMetadata, Surface, SurfaceId, TerminalNotificationMetadata, WorkspaceId,
    WorkspaceModel,
};

const MAX_LOG_ENTRIES: usize = 200;
/// Upper bound on retained notifications. Notifications accumulate for the life
/// of the process (they are never persisted), so without a cap a long-running
/// instance with a flapping agent would grow this `Vec` without bound. When the
/// cap is exceeded the oldest entries are dropped, keeping the newest.
pub(super) const MAX_NOTIFICATIONS: usize = 1_000;
/// Upper bound on distinct status/progress keys retained per workspace. These
/// maps are keyed and updated in place, so they only grow when a client posts
/// new keys; without a cap a same-uid socket client could grow them without
/// bound (a slow memory-exhaustion DoS), unlike logs and notifications which
/// are already capped. When the cap is exceeded the oldest entry is dropped.
pub(super) const MAX_STATUS_ENTRIES: usize = 256;
pub(super) const MAX_PROGRESS_ENTRIES: usize = 256;

#[derive(Clone, Copy)]
enum HookStatusOrdering {
    CompareInModel,
    AlreadySerialized,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClearedAgentMetadata {
    pub(super) statuses: Vec<ClearedStatusEntry>,
    pub(super) progress: Vec<ProgressEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClearedAgentSession {
    pub(super) agent_session: AgentSession,
    pub(super) metadata: ClearedAgentMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClearedStatusEntry {
    entry: StatusEntry,
    hook: Option<StatusHookMetadata>,
}

pub(super) fn agent_session_lifecycle_keeps_metadata(lifecycle: AgentSessionLifecycle) -> bool {
    matches!(
        lifecycle,
        AgentSessionLifecycle::Running
            | AgentSessionLifecycle::Idle
            | AgentSessionLifecycle::NeedsInput
            | AgentSessionLifecycle::Unknown
    )
}

impl WorkspaceModel {
    pub fn mark_surface_unread(&mut self, surface_id: &str, unread: bool) -> bool {
        if !self.surfaces.contains_key(surface_id) {
            return false;
        };
        if unread {
            self.output_unread_surface_ids
                .insert(surface_id.to_string());
        } else {
            self.output_unread_surface_ids.remove(surface_id);
        }
        let notification_unread = self.notification_unread_surface_ids.contains(surface_id);
        let surface = self
            .surfaces
            .get_mut(surface_id)
            .expect("surface existence checked above");
        surface.unread = unread || notification_unread;
        surface.needs_attention = surface.unread;
        let workspace_id = surface.workspace_id.clone();
        self.recompute_workspace_attention(&workspace_id);
        true
    }

    pub fn create_notification(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
        workspace_id: Option<WorkspaceId>,
        surface_id: Option<SurfaceId>,
    ) -> NotificationItem {
        self.create_notification_with_evictions(title, body, kind, workspace_id, surface_id)
            .notification
    }

    pub fn create_notification_with_evictions(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
        workspace_id: Option<WorkspaceId>,
        surface_id: Option<SurfaceId>,
    ) -> NotificationCreation {
        self.create_notification_with_hook_prompt(title, body, kind, workspace_id, surface_id, None)
    }

    pub fn create_hook_prompt_notification(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
        workspace_id: Option<WorkspaceId>,
        surface_id: Option<SurfaceId>,
        hook_prompt: HookPromptMetadata,
    ) -> NotificationItem {
        self.create_hook_prompt_notification_with_evictions(
            title,
            body,
            kind,
            workspace_id,
            surface_id,
            hook_prompt,
        )
        .notification
    }

    pub fn create_hook_prompt_notification_with_evictions(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
        workspace_id: Option<WorkspaceId>,
        surface_id: Option<SurfaceId>,
        hook_prompt: HookPromptMetadata,
    ) -> NotificationCreation {
        self.create_notification_with_hook_prompt(
            title,
            body,
            kind,
            workspace_id,
            surface_id,
            Some(hook_prompt),
        )
    }

    fn create_notification_with_hook_prompt(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
        workspace_id: Option<WorkspaceId>,
        surface_id: Option<SurfaceId>,
        hook_prompt: Option<HookPromptMetadata>,
    ) -> NotificationCreation {
        let title = title.into();
        let body = body.into();
        let existing = hook_prompt.as_ref().and_then(|prompt| {
            self.notifications.iter().position(|notification| {
                notification.workspace_id == workspace_id
                    && notification.surface_id == surface_id
                    && self
                        .hook_prompt_correlations
                        .get(&notification.id)
                        .is_some_and(|existing| {
                            existing.id == prompt.id
                                && existing.provider == prompt.provider
                                && existing.session_id == prompt.session_id
                                && existing.kind == prompt.kind
                                && existing.correlation_id == prompt.correlation_id
                        })
            })
        });
        if let Some(index) = existing {
            let mut item = self.notifications.remove(index);
            item.title = title;
            item.body = body;
            item.kind = kind;
            item.created_at_ms = super::now_ms().max(item.created_at_ms.saturating_add(1));
            item.read = false;
            item.workspace_id = workspace_id;
            item.surface_id = surface_id;
            item.terminal_metadata = None;
            if let Some(hook_prompt) = hook_prompt {
                self.hook_prompt_correlations
                    .insert(item.id.clone(), hook_prompt);
            }
            self.mark_notification_target_unread(
                item.workspace_id.as_deref(),
                item.surface_id.as_deref(),
            );
            self.notifications.push(item.clone());
            return NotificationCreation {
                notification: item,
                evicted_desktop_notification_ids: Vec::new(),
            };
        }
        let item = NotificationItem {
            id: self.next_notification_id(),
            title,
            body,
            kind,
            created_at_ms: super::now_ms(),
            read: false,
            workspace_id,
            surface_id,
            terminal_metadata: None,
        };
        if let Some(hook_prompt) = hook_prompt {
            self.hook_prompt_correlations
                .insert(item.id.clone(), hook_prompt);
        }
        self.mark_notification_target_unread(
            item.workspace_id.as_deref(),
            item.surface_id.as_deref(),
        );
        self.notifications.push(item.clone());
        let evicted_desktop_notification_ids = if self.notifications.len() > MAX_NOTIFICATIONS {
            let overflow = self.notifications.len() - MAX_NOTIFICATIONS;
            let evicted_desktop_notification_ids = self
                .notifications
                .drain(0..overflow)
                .map(|notification| notification.id)
                .collect::<Vec<_>>();
            for notification_id in &evicted_desktop_notification_ids {
                self.hook_prompt_correlations.remove(notification_id);
            }
            self.recompute_notification_attention();
            evicted_desktop_notification_ids
        } else {
            Vec::new()
        };
        NotificationCreation {
            notification: item,
            evicted_desktop_notification_ids,
        }
    }

    pub fn update_notification(
        &mut self,
        notification_id: &str,
        title: impl Into<String>,
        body: impl Into<String>,
        kind: NotificationKind,
    ) -> Option<NotificationItem> {
        let index = self
            .notifications
            .iter()
            .position(|notification| notification.id == notification_id)?;
        let mut item = self.notifications.remove(index);
        item.title = title.into();
        item.body = body.into();
        item.kind = kind;
        item.created_at_ms = super::now_ms().max(item.created_at_ms.saturating_add(1));
        item.read = false;
        self.mark_notification_target_unread(
            item.workspace_id.as_deref(),
            item.surface_id.as_deref(),
        );
        self.notifications.push(item.clone());
        Some(item)
    }

    pub fn list_notifications(&self) -> Vec<NotificationItem> {
        self.notifications.clone()
    }

    /// Borrows one notification page in chronological order.
    ///
    /// `before_id` is an exclusive cursor. A missing cursor selects the newest
    /// entries, while an ID that is no longer retained returns `None`.
    pub fn notification_page(
        &self,
        limit: usize,
        before_id: Option<&str>,
    ) -> Option<&[NotificationItem]> {
        let end = match before_id {
            Some(before_id) => self
                .notifications
                .iter()
                .position(|notification| notification.id == before_id)?,
            None => self.notifications.len(),
        };
        let start = end.saturating_sub(limit);
        Some(&self.notifications[start..end])
    }

    pub fn set_notification_terminal_metadata(
        &mut self,
        notification_id: &str,
        metadata: Option<TerminalNotificationMetadata>,
    ) -> Option<NotificationItem> {
        let notification = self
            .notifications
            .iter_mut()
            .find(|notification| notification.id == notification_id)?;
        notification.terminal_metadata = metadata;
        Some(notification.clone())
    }

    pub fn unread_notification_count(&self) -> usize {
        self.notifications
            .iter()
            .filter(|notification| !notification.read)
            .count()
    }

    pub fn mark_notifications_read(&mut self) {
        for notification in &mut self.notifications {
            notification.read = true;
        }
        self.recompute_notification_attention();
    }

    pub fn dismiss_notification(&mut self, notification_id: &str) -> bool {
        let before = self.notifications.len();
        self.notifications
            .retain(|notification| notification.id != notification_id);
        let removed = self.notifications.len() != before;
        if removed {
            self.hook_prompt_correlations.remove(notification_id);
            self.recompute_notification_attention();
        }
        removed
    }

    pub fn resolve_hook_prompt(
        &mut self,
        resolution: &HookPromptResolution,
    ) -> Option<NotificationItem> {
        let index = self
            .notifications
            .iter()
            .enumerate()
            .filter_map(|(index, notification)| {
                let prompt = self.hook_prompt_correlations.get(&notification.id)?;
                let matches_target = notification.workspace_id == resolution.workspace_id
                    && notification.surface_id == resolution.surface_id;
                let matches_identity = prompt.provider == resolution.provider
                    && prompt.session_id == resolution.session_id
                    && prompt.kind == resolution.kind;
                let matches_correlation = prompt.event_order <= resolution.event_order
                    && resolution
                        .correlation_id
                        .as_ref()
                        .is_none_or(|correlation_id| {
                            prompt.correlation_id.as_ref() == Some(correlation_id)
                        });
                (matches_target && matches_identity && matches_correlation)
                    .then_some((index, prompt.event_order))
            })
            .max_by_key(|(_, event_order)| *event_order)
            .map(|(index, _)| index)?;
        let resolved = {
            let notification = &mut self.notifications[index];
            notification.read = true;
            notification.clone()
        };
        self.hook_prompt_correlations.remove(&resolved.id);
        self.recompute_notification_attention();
        Some(resolved)
    }

    pub fn evict_hook_prompt_correlations_for_surfaces(
        &mut self,
        surface_ids: &[SurfaceId],
    ) -> Vec<String> {
        let surface_ids = surface_ids.iter().collect::<BTreeSet<_>>();
        self.evict_hook_prompt_correlations_matching(|notification, _| {
            notification
                .surface_id
                .as_ref()
                .is_some_and(|surface_id| surface_ids.contains(surface_id))
        })
    }

    pub fn evict_hook_prompt_correlations_for_session_target(
        &mut self,
        provider: &str,
        session_id: &str,
        surface_id: &str,
    ) -> Vec<String> {
        self.evict_hook_prompt_correlations_matching(|notification, prompt| {
            notification.surface_id.as_deref() == Some(surface_id)
                && prompt.provider == provider
                && prompt.session_id == session_id
        })
    }

    fn evict_hook_prompt_correlations_matching(
        &mut self,
        mut should_evict: impl FnMut(&NotificationItem, &HookPromptMetadata) -> bool,
    ) -> Vec<String> {
        let mut evicted_ids = Vec::new();
        for notification in &mut self.notifications {
            let evict = self
                .hook_prompt_correlations
                .get(&notification.id)
                .is_some_and(|prompt| should_evict(notification, prompt));
            if evict {
                notification.read = true;
                evicted_ids.push(notification.id.clone());
            }
        }
        for notification_id in &evicted_ids {
            self.hook_prompt_correlations.remove(notification_id);
        }
        self.recompute_notification_attention();
        evicted_ids
    }

    pub fn clear_notifications(&mut self) {
        self.notifications.clear();
        self.hook_prompt_correlations.clear();
        self.recompute_notification_attention();
    }

    pub fn set_status(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
    ) -> Option<StatusEntry> {
        self.set_status_ordered(workspace_id, key, label, value, color, None)
    }

    pub fn set_status_ordered(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        order: Option<u128>,
    ) -> Option<StatusEntry> {
        self.set_status_with_hook_metadata(
            workspace_id,
            key,
            label,
            value,
            color,
            order.map(StatusHookMetadata::from_order),
        )
    }

    pub fn set_status_with_hook_metadata(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        hook: Option<StatusHookMetadata>,
    ) -> Option<StatusEntry> {
        self.set_status_with_hook_metadata_applied(workspace_id, key, label, value, color, hook)
            .map(|(entry, _applied)| entry)
    }

    pub fn set_status_with_hook_metadata_applied(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        hook: Option<StatusHookMetadata>,
    ) -> Option<(StatusEntry, bool)> {
        self.set_status_with_hook_metadata_applied_ordering(
            workspace_id,
            key,
            label,
            value,
            color,
            hook,
            HookStatusOrdering::CompareInModel,
        )
    }

    pub fn set_status_with_hook_metadata_applied_after_serialized_ordering(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        hook: Option<StatusHookMetadata>,
    ) -> Option<(StatusEntry, bool)> {
        self.set_status_with_hook_metadata_applied_ordering(
            workspace_id,
            key,
            label,
            value,
            color,
            hook,
            HookStatusOrdering::AlreadySerialized,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn set_status_with_hook_metadata_applied_ordering(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
        color: Option<String>,
        hook: Option<StatusHookMetadata>,
        ordering: HookStatusOrdering,
    ) -> Option<(StatusEntry, bool)> {
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let entry = StatusEntry {
            key: key.into(),
            label: label.into(),
            value: value.into(),
            color,
        };
        if let Some(hook) = hook {
            let current_hook = self
                .status_hooks
                .get(workspace_id)
                .and_then(|hooks| hooks.get(&entry.key));
            let ignored = match ordering {
                HookStatusOrdering::CompareInModel => {
                    should_ignore_hook_status(current_hook, &hook)
                }
                HookStatusOrdering::AlreadySerialized => {
                    should_ignore_hook_status_after_serialized_ordering(current_hook, &hook)
                }
            };
            if ignored {
                return self
                    .statuses
                    .get(workspace_id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|existing| existing.key == entry.key)
                            .cloned()
                    })
                    .or(Some(entry))
                    .map(|entry| (entry, false));
            }
            let next_hook = merge_hook_metadata(current_hook, hook);
            self.status_hooks
                .entry(workspace_id.to_string())
                .or_default()
                .insert(entry.key.clone(), next_hook);
        } else if let Some(hooks) = self.status_hooks.get_mut(workspace_id) {
            hooks.remove(&entry.key);
            let remove_workspace_hooks = hooks.is_empty();
            if remove_workspace_hooks {
                self.status_hooks.remove(workspace_id);
            }
        }
        let entries = self.statuses.entry(workspace_id.to_string()).or_default();
        let evicted_key = if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            *existing = entry.clone();
            None
        } else {
            // A new key grows the map; cap it so a misbehaving client cannot
            // exhaust memory. Drop the oldest entry when at capacity.
            let evicted = (entries.len() >= MAX_STATUS_ENTRIES).then(|| entries.remove(0).key);
            entries.push(entry.clone());
            evicted
        };
        // Keep the parallel hook map from outliving its evicted status entry.
        if let Some(evicted_key) = evicted_key {
            if let Some(hooks) = self.status_hooks.get_mut(workspace_id) {
                hooks.remove(&evicted_key);
                if hooks.is_empty() {
                    self.status_hooks.remove(workspace_id);
                }
            }
        }
        Some((entry, true))
    }

    pub fn list_status(&self, workspace_id: &str) -> Vec<StatusEntry> {
        self.statuses.get(workspace_id).cloned().unwrap_or_default()
    }

    pub fn list_status_limited(&self, workspace_id: &str, limit: usize) -> Vec<StatusEntry> {
        self.statuses
            .get(workspace_id)
            .map(|entries| entries[entries.len().saturating_sub(limit)..].to_vec())
            .unwrap_or_default()
    }

    pub fn clear_status(&mut self, workspace_id: &str, key: Option<&str>) -> bool {
        self.clear_status_ordered(workspace_id, key, None)
    }

    pub fn clear_status_ordered(
        &mut self,
        workspace_id: &str,
        key: Option<&str>,
        order: Option<u128>,
    ) -> bool {
        self.clear_status_with_hook_metadata(
            workspace_id,
            key,
            order.map(StatusHookMetadata::from_order),
        )
    }

    pub fn clear_status_with_hook_metadata(
        &mut self,
        workspace_id: &str,
        key: Option<&str>,
        hook: Option<StatusHookMetadata>,
    ) -> bool {
        self.clear_status_with_hook_metadata_ordering(
            workspace_id,
            key,
            hook,
            HookStatusOrdering::CompareInModel,
        )
    }

    pub fn clear_status_with_hook_metadata_after_serialized_ordering(
        &mut self,
        workspace_id: &str,
        key: Option<&str>,
        hook: Option<StatusHookMetadata>,
    ) -> bool {
        self.clear_status_with_hook_metadata_ordering(
            workspace_id,
            key,
            hook,
            HookStatusOrdering::AlreadySerialized,
        )
    }

    fn clear_status_with_hook_metadata_ordering(
        &mut self,
        workspace_id: &str,
        key: Option<&str>,
        hook: Option<StatusHookMetadata>,
        ordering: HookStatusOrdering,
    ) -> bool {
        if !self.workspaces.contains_key(workspace_id) {
            return false;
        }
        match (key, hook) {
            (Some(key), Some(hook)) => {
                let current_hook = self
                    .status_hooks
                    .get(workspace_id)
                    .and_then(|hooks| hooks.get(key));
                let ignored = match ordering {
                    HookStatusOrdering::CompareInModel => {
                        should_ignore_hook_status(current_hook, &hook)
                    }
                    HookStatusOrdering::AlreadySerialized => {
                        should_ignore_hook_status_after_serialized_ordering(current_hook, &hook)
                    }
                };
                if ignored {
                    return true;
                }
                let next_hook = merge_hook_metadata(current_hook, hook);
                self.status_hooks
                    .entry(workspace_id.to_string())
                    .or_default()
                    .insert(key.to_string(), next_hook);
            }
            (Some(key), None) => {
                if let Some(hooks) = self.status_hooks.get_mut(workspace_id) {
                    hooks.remove(key);
                    let remove_workspace_hooks = hooks.is_empty();
                    if remove_workspace_hooks {
                        self.status_hooks.remove(workspace_id);
                    }
                }
            }
            (None, None) => {
                self.status_hooks.remove(workspace_id);
            }
            (None, Some(_)) => {
                self.status_hooks.remove(workspace_id);
            }
        }
        let Some(entries) = self.statuses.get_mut(workspace_id) else {
            return true;
        };
        if let Some(key) = key {
            entries.retain(|entry| entry.key != key);
        } else {
            entries.clear();
        }
        true
    }

    pub fn set_progress(
        &mut self,
        workspace_id: &str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: f64,
        total: Option<f64>,
    ) -> Option<ProgressEntry> {
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let total = total.filter(|total| *total > 0.0);
        let value = total
            .map(|total| value.min(total))
            .unwrap_or(value)
            .max(0.0);
        let entry = ProgressEntry {
            key: key.into(),
            label: label.into(),
            value,
            total,
        };
        let entries = self.progress.entry(workspace_id.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            *existing = entry.clone();
        } else {
            // A new key grows the map; cap it so a misbehaving client cannot
            // exhaust memory. Drop the oldest entry when at capacity.
            if entries.len() >= MAX_PROGRESS_ENTRIES {
                entries.remove(0);
            }
            entries.push(entry.clone());
        }
        Some(entry)
    }

    pub fn list_progress(&self, workspace_id: &str) -> Vec<ProgressEntry> {
        self.progress.get(workspace_id).cloned().unwrap_or_default()
    }

    pub fn list_progress_limited(&self, workspace_id: &str, limit: usize) -> Vec<ProgressEntry> {
        self.progress
            .get(workspace_id)
            .map(|entries| entries[entries.len().saturating_sub(limit)..].to_vec())
            .unwrap_or_default()
    }

    pub fn clear_progress(&mut self, workspace_id: &str, key: Option<&str>) -> bool {
        let Some(entries) = self.progress.get_mut(workspace_id) else {
            return self.workspaces.contains_key(workspace_id);
        };
        if let Some(key) = key {
            entries.retain(|entry| entry.key != key);
        } else {
            entries.clear();
        }
        true
    }

    pub fn append_log(
        &mut self,
        workspace_id: &str,
        level: LogLevel,
        message: impl Into<String>,
    ) -> Option<LogEntry> {
        if !self.workspaces.contains_key(workspace_id) {
            return None;
        }
        let entry = LogEntry {
            id: self.next_log_id(),
            timestamp_ms: super::now_ms(),
            level,
            message: message.into(),
        };
        let entries = self.logs.entry(workspace_id.to_string()).or_default();
        entries.insert(0, entry.clone());
        entries.truncate(MAX_LOG_ENTRIES);
        Some(entry)
    }

    pub fn list_logs(&self, workspace_id: &str) -> Vec<LogEntry> {
        self.logs.get(workspace_id).cloned().unwrap_or_default()
    }

    pub fn clear_logs(&mut self, workspace_id: &str) -> bool {
        let Some(entries) = self.logs.get_mut(workspace_id) else {
            return self.workspaces.contains_key(workspace_id);
        };
        entries.clear();
        true
    }

    pub fn restore_agent_metadata(
        &mut self,
        workspace_id: &str,
        metadata: ClearedAgentMetadata,
    ) -> bool {
        if !self.workspaces.contains_key(workspace_id) {
            return false;
        }
        for status in metadata.statuses {
            let entry = status.entry;
            let _ = self.set_status_with_hook_metadata(
                workspace_id,
                entry.key,
                entry.label,
                entry.value,
                entry.color,
                status.hook,
            );
        }
        for entry in metadata.progress {
            let _ = self.set_progress(
                workspace_id,
                entry.key,
                entry.label,
                entry.value,
                entry.total,
            );
        }
        true
    }

    pub(super) fn clear_removed_surface_metadata(&mut self, surface: &Surface) {
        self.output_unread_surface_ids.remove(&surface.id);
        self.notification_unread_surface_ids.remove(&surface.id);
        let surface_prefix = format!("surface:{}:", surface.id);
        self.clear_status_keys_matching(&surface.workspace_id, |key| {
            key.starts_with(&surface_prefix)
        });
        self.clear_progress_keys_matching(&surface.workspace_id, |key| {
            key.starts_with(&surface_prefix)
        });
        if let Some(agent_session) = surface.agent_session.as_ref() {
            self.clear_agent_metadata_if_no_active_session(
                &surface.workspace_id,
                agent_session.agent,
            );
        }
    }

    pub(super) fn clear_agent_metadata_if_no_active_session(
        &mut self,
        workspace_id: &str,
        agent: AgentKind,
    ) -> ClearedAgentMetadata {
        if self.workspace_has_active_agent_metadata_session(workspace_id, agent) {
            return ClearedAgentMetadata::default();
        }
        let metadata = self.agent_metadata_snapshot(workspace_id, agent);
        self.clear_status_keys_matching(workspace_id, |key| {
            agent_metadata_status_key_matches(agent, key)
        });
        self.clear_progress_keys_matching(workspace_id, |key| {
            agent_metadata_progress_key_matches(agent, key)
        });
        metadata
    }

    fn agent_metadata_snapshot(
        &self,
        workspace_id: &str,
        agent: AgentKind,
    ) -> ClearedAgentMetadata {
        let statuses = self
            .statuses
            .get(workspace_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| agent_metadata_status_key_matches(agent, &entry.key))
                    .map(|entry| ClearedStatusEntry {
                        entry: entry.clone(),
                        hook: self
                            .status_hooks
                            .get(workspace_id)
                            .and_then(|hooks| hooks.get(&entry.key))
                            .cloned(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let progress = self
            .progress
            .get(workspace_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| agent_metadata_progress_key_matches(agent, &entry.key))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ClearedAgentMetadata { statuses, progress }
    }

    fn workspace_has_active_agent_metadata_session(
        &self,
        workspace_id: &str,
        agent: AgentKind,
    ) -> bool {
        self.surfaces.values().any(|surface| {
            surface.workspace_id == workspace_id
                && surface.agent_session.as_ref().is_some_and(|session| {
                    session.agent == agent
                        && agent_session_lifecycle_keeps_metadata(session.lifecycle)
                })
        })
    }

    fn clear_status_keys_matching(
        &mut self,
        workspace_id: &str,
        mut should_clear: impl FnMut(&str) -> bool,
    ) {
        let mut keys = BTreeSet::new();
        if let Some(entries) = self.statuses.get(workspace_id) {
            keys.extend(
                entries
                    .iter()
                    .filter(|entry| should_clear(&entry.key))
                    .map(|entry| entry.key.clone()),
            );
        }
        if let Some(hooks) = self.status_hooks.get(workspace_id) {
            keys.extend(hooks.keys().filter(|key| should_clear(key)).cloned());
        }
        for key in keys {
            let _ = self.clear_status(workspace_id, Some(&key));
        }
    }

    fn clear_progress_keys_matching(
        &mut self,
        workspace_id: &str,
        mut should_clear: impl FnMut(&str) -> bool,
    ) {
        let keys = self
            .progress
            .get(workspace_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| should_clear(&entry.key))
                    .map(|entry| entry.key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for key in keys {
            let _ = self.clear_progress(workspace_id, Some(&key));
        }
    }

    fn next_notification_id(&mut self) -> String {
        self.next_notification += 1;
        format!("notification-{}", self.next_notification)
    }

    fn mark_notification_target_unread(
        &mut self,
        workspace_id: Option<&str>,
        surface_id: Option<&str>,
    ) {
        if let Some(surface_id) = surface_id {
            if self.surfaces.contains_key(surface_id) {
                self.notification_unread_surface_ids
                    .insert(surface_id.to_string());
                let surface = self
                    .surfaces
                    .get_mut(surface_id)
                    .expect("surface existence checked above");
                surface.unread = true;
                surface.needs_attention = true;
                let workspace_id = surface.workspace_id.clone();
                self.recompute_workspace_attention(&workspace_id);
            } else {
                if let Some(workspace_id) = workspace_id {
                    if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                        workspace.needs_attention = true;
                    }
                }
            }
        } else if let Some(workspace_id) = workspace_id {
            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.needs_attention = true;
            }
        }
    }

    pub(super) fn recompute_workspace_attention(&mut self, workspace_id: &str) {
        let has_unread_surface = self
            .surfaces
            .values()
            .any(|surface| surface.workspace_id == workspace_id && surface.unread);
        let has_unread_workspace_notification = self.notifications.iter().any(|notification| {
            !notification.read
                && notification.surface_id.is_none()
                && notification.workspace_id.as_deref() == Some(workspace_id)
        });
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            workspace.needs_attention = has_unread_surface || has_unread_workspace_notification;
        }
    }

    fn recompute_notification_attention(&mut self) {
        self.notification_unread_surface_ids = self
            .notifications
            .iter()
            .filter(|notification| !notification.read)
            .filter_map(|notification| notification.surface_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        let unread_workspace_ids = self
            .notifications
            .iter()
            .filter(|notification| !notification.read)
            .filter(|notification| notification.surface_id.is_none())
            .filter_map(|notification| notification.workspace_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        for surface in self.surfaces.values_mut() {
            let unread = self.output_unread_surface_ids.contains(&surface.id)
                || self.notification_unread_surface_ids.contains(&surface.id);
            surface.unread = unread;
            surface.needs_attention = unread;
        }
        for workspace in self.workspaces.values_mut() {
            workspace.needs_attention = unread_workspace_ids.contains(&workspace.id)
                || self.surfaces.values().any(|surface| {
                    surface.workspace_id == workspace.id
                        && (surface.unread || surface.needs_attention)
                });
        }
    }

    fn next_log_id(&mut self) -> String {
        self.next_log += 1;
        format!("log-{}", self.next_log)
    }
}

fn agent_metadata_status_key_matches(agent: AgentKind, key: &str) -> bool {
    let Some(rest) = key.strip_prefix("agent:") else {
        return false;
    };
    agent_metadata_aliases(agent).iter().any(|alias| {
        rest == *alias
            || rest
                .strip_prefix(alias)
                .is_some_and(|suffix| suffix == ":permission")
    })
}

fn agent_metadata_progress_key_matches(agent: AgentKind, key: &str) -> bool {
    let Some(rest) = key.strip_prefix("agent:") else {
        return false;
    };
    agent_metadata_aliases(agent).iter().any(|alias| {
        rest.strip_prefix(alias)
            .is_some_and(|suffix| suffix.starts_with(':'))
    })
}
