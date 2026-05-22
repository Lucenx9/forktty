use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    OpenCode,
    Gemini,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentCapabilities {
    pub hooks: bool,
    pub status_events: bool,
    pub permission_requests: bool,
    pub project_context_files: bool,
    pub mcp: bool,
    pub headless_json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfigLocation {
    pub scope: &'static str,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub kind: AgentKind,
    pub label: &'static str,
    pub install_hint: &'static str,
    pub launch_hint: &'static str,
    pub context_files: &'static [&'static str],
    pub capabilities: AgentCapabilities,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Running,
    NeedsInput,
    PermissionRequest,
    ToolRunning,
    TestsRunning,
    Done,
    Failed,
    Cancelled,
    Unknown,
}

pub fn normalize_agent_status(raw: &str) -> AgentStatus {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return AgentStatus::Unknown;
    }
    if lower.contains("permission") || lower.contains("approval") {
        return AgentStatus::PermissionRequest;
    }
    if lower.contains("needs input") || lower.contains("prompt") || lower.contains("waiting") {
        return AgentStatus::NeedsInput;
    }
    if lower.contains("test") {
        return AgentStatus::TestsRunning;
    }
    if lower.contains("tool") || lower.contains("execut") {
        return AgentStatus::ToolRunning;
    }
    if lower.contains("running") || lower.contains("working") || lower.contains("in progress") {
        return AgentStatus::Running;
    }
    if lower.contains("done") || lower.contains("complete") || lower.contains("success") {
        return AgentStatus::Done;
    }
    if lower.contains("cancel") || lower.contains("interrupt") {
        return AgentStatus::Cancelled;
    }
    if lower.contains("fail") || lower.contains("error") {
        return AgentStatus::Failed;
    }
    if lower == "idle" || lower == "ready" {
        return AgentStatus::Idle;
    }
    AgentStatus::Unknown
}

#[cfg(test)]
mod tests {
    use super::{normalize_agent_status, AgentStatus};

    #[test]
    fn normalizes_provider_status_strings() {
        assert_eq!(normalize_agent_status("Ready"), AgentStatus::Idle);
        assert_eq!(
            normalize_agent_status("running tool: bash"),
            AgentStatus::ToolRunning
        );
        assert_eq!(
            normalize_agent_status("needs input"),
            AgentStatus::NeedsInput
        );
        assert_eq!(
            normalize_agent_status("Permission required"),
            AgentStatus::PermissionRequest
        );
        assert_eq!(
            normalize_agent_status("tests running"),
            AgentStatus::TestsRunning
        );
        assert_eq!(normalize_agent_status("completed"), AgentStatus::Done);
        assert_eq!(normalize_agent_status("failed"), AgentStatus::Failed);
        assert_eq!(normalize_agent_status("cancelled"), AgentStatus::Cancelled);
    }
}
