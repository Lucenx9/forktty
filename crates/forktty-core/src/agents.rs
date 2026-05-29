use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    #[serde(rename = "opencode", alias = "open_code")]
    OpenCode,
    Gemini,
    Custom,
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
    if lower.contains("fail") || lower.contains("error") {
        return AgentStatus::Failed;
    }
    if lower.contains("cancel") || lower.contains("interrupt") {
        return AgentStatus::Cancelled;
    }
    if lower.contains("done") || lower.contains("complete") || lower.contains("success") {
        return AgentStatus::Done;
    }
    if lower.contains("permission") || lower.contains("approval") {
        return AgentStatus::PermissionRequest;
    }
    if lower.contains("needs input")
        || lower.contains("input needed")
        || lower.contains("user input")
        || lower.contains("waiting for user")
        || lower.contains("waiting on user")
        || lower.contains("prompt")
    {
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
    if lower == "idle" || lower == "ready" {
        return AgentStatus::Idle;
    }
    AgentStatus::Unknown
}

#[cfg(test)]
mod tests {
    use super::{normalize_agent_status, AgentKind, AgentStatus};

    #[test]
    fn opencode_provider_key_matches_documented_spelling() {
        assert_eq!(
            serde_json::to_string(&AgentKind::OpenCode).unwrap(),
            "\"opencode\""
        );
        assert_eq!(
            serde_json::from_str::<AgentKind>("\"opencode\"").unwrap(),
            AgentKind::OpenCode
        );
        assert_eq!(
            serde_json::from_str::<AgentKind>("\"open_code\"").unwrap(),
            AgentKind::OpenCode
        );
    }

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
        assert_eq!(normalize_agent_status(""), AgentStatus::Unknown);
        assert_eq!(
            normalize_agent_status("provider-specific queued"),
            AgentStatus::Unknown
        );
    }

    #[test]
    fn terminal_states_take_priority_over_activity_words() {
        assert_eq!(normalize_agent_status("tests failed"), AgentStatus::Failed);
        assert_eq!(normalize_agent_status("tool error"), AgentStatus::Failed);
        assert_eq!(normalize_agent_status("tests completed"), AgentStatus::Done);
        assert_eq!(
            normalize_agent_status("tool cancelled"),
            AgentStatus::Cancelled
        );
        assert_eq!(
            normalize_agent_status("waiting for user input"),
            AgentStatus::NeedsInput
        );
        assert_eq!(
            normalize_agent_status("waiting for tests"),
            AgentStatus::TestsRunning
        );
        assert_eq!(normalize_agent_status("waiting"), AgentStatus::Unknown);
    }
}
