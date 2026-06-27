//! Surface kinds, persisted surface state, and agent-session metadata.
//!
//! These types cross the model/session/socket/UI boundary, so keep serde field
//! names and enum tags stable when moving or extending them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::agents::AgentKind;

pub const MAX_PERSISTED_SCROLLBACK_BYTES: usize = 64 * 1024;

/// What a surface renders. Defaults to `Terminal` so sessions persisted
/// before this field existed load every surface as a terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceKind {
    #[default]
    Terminal,
    Browser {
        url: String,
        #[serde(default)]
        profile: crate::profile::ProfileId,
    },
    /// The surface's shell process is `ssh <host>`.
    Ssh {
        /// Full ssh target, e.g. `user@example.com` or `[::1]`.
        host: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Surface {
    pub id: super::SurfaceId,
    pub workspace_id: super::WorkspaceId,
    pub cwd: PathBuf,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub needs_attention: bool,
    #[serde(default)]
    pub kind: SurfaceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted_scrollback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSession {
    pub agent: AgentKind,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub lifecycle: AgentSessionLifecycle,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub last_activity_ms: u64,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionLifecycle {
    Running,
    Idle,
    NeedsInput,
    Suspended,
    Ended,
    #[default]
    Unknown,
}

pub(super) fn normalize_persisted_scrollback(text: String) -> Option<String> {
    let text = text
        .chars()
        .filter(|ch| !ch.is_control() || matches!(*ch, '\n' | '\r' | '\t'))
        .collect::<String>();
    if text.is_empty() {
        return None;
    }
    if text.len() <= MAX_PERSISTED_SCROLLBACK_BYTES {
        return Some(text);
    }
    let mut start = text.len() - MAX_PERSISTED_SCROLLBACK_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(text[start..].to_string())
}
