use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

const MAX_AGENT_SESSION_ID_LEN: usize = 1024;
const MAX_CODEX_SESSION_SCAN_DEPTH: usize = 5;
const MAX_CODEX_SESSION_CANDIDATES: usize = 64;
const MAX_CODEX_SESSION_META_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Antigravity,
    Grok,
    Pi,
    #[serde(rename = "opencode", alias = "open_code")]
    OpenCode,
    #[serde(other)]
    Custom,
}

pub fn agent_metadata_aliases(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        AgentKind::ClaudeCode => &["claude", "claude-code", "claude_code"],
        AgentKind::Codex => &["codex"],
        AgentKind::Antigravity => &["antigravity", "agy"],
        AgentKind::Grok => &["grok", "grok-build", "grok_build"],
        AgentKind::Pi => &["pi"],
        AgentKind::OpenCode => &["opencode", "open-code", "open_code"],
        AgentKind::Custom => &["custom"],
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResumeCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentResumeError {
    UnsupportedAgent(AgentKind),
    InvalidSessionId,
    InvalidResumeCwd,
}

impl fmt::Display for AgentResumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentResumeError::UnsupportedAgent(agent) => {
                write!(f, "agent {agent:?} does not have a safe resume command")
            }
            AgentResumeError::InvalidSessionId => f.write_str(
                "agent session id is empty, too long, flag-like, or contains control characters",
            ),
            AgentResumeError::InvalidResumeCwd => f.write_str(
                "agent resume cwd is empty, relative, non-UTF-8, or contains control characters",
            ),
        }
    }
}

impl std::error::Error for AgentResumeError {}

pub fn agent_resume_command(
    agent: AgentKind,
    session_id: &str,
) -> Result<AgentResumeCommand, AgentResumeError> {
    agent_resume_command_with_cwd(agent, session_id, None)
}

pub fn agent_resume_command_with_cwd(
    agent: AgentKind,
    session_id: &str,
    resume_cwd: Option<&Path>,
) -> Result<AgentResumeCommand, AgentResumeError> {
    agent_resume_command_with_cwd_and_permission_mode(agent, session_id, resume_cwd, None)
}

pub fn agent_resume_command_with_cwd_and_permission_mode(
    agent: AgentKind,
    session_id: &str,
    resume_cwd: Option<&Path>,
    permission_mode: Option<&str>,
) -> Result<AgentResumeCommand, AgentResumeError> {
    let session_id = safe_resume_session_id(session_id)?;
    let (program, args): (&str, Vec<String>) = match agent {
        AgentKind::Codex => {
            let mut args = Vec::new();
            if agent_permission_mode_is_bypass(permission_mode) {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            args.push("resume".to_string());
            if let Some(resume_cwd) = resume_cwd {
                args.push("-C".to_string());
                args.push(safe_resume_cwd(resume_cwd)?);
            }
            args.push(session_id);
            ("codex", args)
        }
        AgentKind::ClaudeCode => {
            let mut args = Vec::new();
            if agent_permission_mode_is_bypass(permission_mode) {
                args.push("--dangerously-skip-permissions".to_string());
            }
            args.push("--resume".to_string());
            args.push(session_id);
            ("claude", args)
        }
        AgentKind::Antigravity => ("agy", vec!["--conversation".to_string(), session_id]),
        AgentKind::Grok => {
            let mut args = Vec::new();
            if let Some(resume_cwd) = resume_cwd {
                args.push("--cwd".to_string());
                args.push(safe_resume_cwd(resume_cwd)?);
            }
            args.push("--resume".to_string());
            args.push(session_id);
            ("grok", args)
        }
        AgentKind::Pi => ("pi", vec!["--session".to_string(), session_id]),
        AgentKind::OpenCode => ("opencode", vec!["--session".to_string(), session_id]),
        AgentKind::Custom => return Err(AgentResumeError::UnsupportedAgent(agent)),
    };
    Ok(AgentResumeCommand {
        program: program.to_string(),
        args,
    })
}

fn agent_permission_mode_is_bypass(permission_mode: Option<&str>) -> bool {
    permission_mode.is_some_and(|mode| mode.trim() == "bypassPermissions")
}

pub fn codex_session_cwd(session_id: &str) -> Option<PathBuf> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    codex_session_cwd_from_home(&codex_home, session_id)
}

pub fn codex_session_cwd_from_home(codex_home: &Path, session_id: &str) -> Option<PathBuf> {
    let session_id = safe_resume_session_id(session_id).ok()?;
    let mut candidates = Vec::new();
    collect_codex_session_candidates(
        &codex_home.join("sessions"),
        &session_id,
        0,
        &mut candidates,
    );
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    candidates.reverse();
    candidates
        .iter()
        .find_map(|path| codex_session_meta_cwd(path, &session_id))
}

fn collect_codex_session_candidates(
    dir: &Path,
    session_id: &str,
    depth: usize,
    candidates: &mut Vec<PathBuf>,
) {
    if depth > MAX_CODEX_SESSION_SCAN_DEPTH || candidates.len() >= MAX_CODEX_SESSION_CANDIDATES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if candidates.len() >= MAX_CODEX_SESSION_CANDIDATES {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_codex_session_candidates(&path, session_id, depth + 1, candidates);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.ends_with(".jsonl") && file_name.contains(session_id) {
            candidates.push(path);
        }
    }
}

fn codex_session_meta_cwd(path: &Path, session_id: &str) -> Option<PathBuf> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file.take(MAX_CODEX_SESSION_META_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let value: serde_json::Value = serde_json::from_str(&line).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("id").and_then(serde_json::Value::as_str) != Some(session_id) {
        return None;
    }
    let cwd = PathBuf::from(payload.get("cwd")?.as_str()?);
    safe_resume_cwd(&cwd).ok()?;
    cwd.is_dir().then_some(cwd)
}

fn safe_resume_session_id(session_id: &str) -> Result<String, AgentResumeError> {
    let session_id = session_id.trim();
    if session_id.is_empty()
        || session_id.len() > MAX_AGENT_SESSION_ID_LEN
        || session_id.starts_with('-')
        || session_id.chars().any(char::is_control)
    {
        return Err(AgentResumeError::InvalidSessionId);
    }
    Ok(session_id.to_string())
}

fn safe_resume_cwd(resume_cwd: &Path) -> Result<String, AgentResumeError> {
    if resume_cwd.as_os_str().is_empty() || !resume_cwd.is_absolute() {
        return Err(AgentResumeError::InvalidResumeCwd);
    }
    let Some(resume_cwd) = resume_cwd.to_str() else {
        return Err(AgentResumeError::InvalidResumeCwd);
    };
    if resume_cwd.chars().any(char::is_control) {
        return Err(AgentResumeError::InvalidResumeCwd);
    }
    Ok(resume_cwd.to_string())
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
    fn antigravity_provider_key_matches_hook_agent_key() {
        assert_eq!(
            serde_json::to_string(&AgentKind::Antigravity).unwrap(),
            "\"antigravity\""
        );
        assert_eq!(
            serde_json::from_str::<AgentKind>("\"antigravity\"").unwrap(),
            AgentKind::Antigravity
        );
    }

    #[test]
    fn pi_provider_key_matches_documented_spelling() {
        assert_eq!(serde_json::to_string(&AgentKind::Pi).unwrap(), "\"pi\"");
        assert_eq!(
            serde_json::from_str::<AgentKind>("\"pi\"").unwrap(),
            AgentKind::Pi
        );
    }

    #[test]
    fn grok_provider_key_matches_documented_spelling() {
        assert_eq!(serde_json::to_string(&AgentKind::Grok).unwrap(), "\"grok\"");
        assert_eq!(
            serde_json::from_str::<AgentKind>("\"grok\"").unwrap(),
            AgentKind::Grok
        );
    }

    #[test]
    fn builds_provider_resume_commands_as_argv() {
        let cases = [
            (
                AgentKind::Codex,
                "codex",
                &["resume", "codex-session-1"][..],
            ),
            (
                AgentKind::ClaudeCode,
                "claude",
                &["--resume", "claude-session-1"][..],
            ),
            (
                AgentKind::Antigravity,
                "agy",
                &["--conversation", "agy-session-1"][..],
            ),
            (AgentKind::Grok, "grok", &["--resume", "grok-session-1"][..]),
            (
                AgentKind::OpenCode,
                "opencode",
                &["--session", "ses_opencode"][..],
            ),
            (AgentKind::Pi, "pi", &["--session", "pi-session-1"][..]),
        ];

        for (agent, program, expected_args) in cases {
            let session_id = expected_args.last().unwrap();
            let command = super::agent_resume_command(agent, session_id).unwrap();
            assert_eq!(command.program, program);
            assert_eq!(command.args, expected_args);
        }
    }

    #[test]
    fn removed_provider_names_deserialize_as_custom() {
        assert_eq!(
            serde_json::from_str::<AgentKind>("\"gemini\"").unwrap(),
            AgentKind::Custom
        );
    }

    #[test]
    fn codex_resume_command_can_pin_resume_cwd() {
        let command = super::agent_resume_command_with_cwd(
            AgentKind::Codex,
            "codex-session-1",
            Some(std::path::Path::new("/tmp/project")),
        )
        .unwrap();

        assert_eq!(command.program, "codex");
        assert_eq!(
            command.args,
            ["resume", "-C", "/tmp/project", "codex-session-1"]
        );
        assert!(super::agent_resume_command_with_cwd(
            AgentKind::Codex,
            "codex-session-1",
            Some(std::path::Path::new("relative/project")),
        )
        .is_err());
        let claude = super::agent_resume_command_with_cwd(
            AgentKind::ClaudeCode,
            "claude-session-1",
            Some(std::path::Path::new("/tmp/project")),
        )
        .unwrap();
        assert_eq!(claude.args, ["--resume", "claude-session-1"]);
        let grok = super::agent_resume_command_with_cwd(
            AgentKind::Grok,
            "grok-session-1",
            Some(std::path::Path::new("/tmp/project")),
        )
        .unwrap();
        assert_eq!(
            grok.args,
            ["--cwd", "/tmp/project", "--resume", "grok-session-1"]
        );
    }

    #[test]
    fn resume_command_reapplies_hook_reported_bypass_permission_mode() {
        let claude = super::agent_resume_command_with_cwd_and_permission_mode(
            AgentKind::ClaudeCode,
            "claude-session-1",
            Some(std::path::Path::new("/tmp/project")),
            Some("bypassPermissions"),
        )
        .unwrap();
        assert_eq!(claude.program, "claude");
        assert_eq!(
            claude.args,
            [
                "--dangerously-skip-permissions",
                "--resume",
                "claude-session-1"
            ]
        );

        let codex = super::agent_resume_command_with_cwd_and_permission_mode(
            AgentKind::Codex,
            "codex-session-1",
            Some(std::path::Path::new("/tmp/project")),
            Some("bypassPermissions"),
        )
        .unwrap();
        assert_eq!(codex.program, "codex");
        assert_eq!(
            codex.args,
            [
                "--dangerously-bypass-approvals-and-sandbox",
                "resume",
                "-C",
                "/tmp/project",
                "codex-session-1"
            ]
        );
    }

    #[test]
    fn finds_codex_session_cwd_from_local_session_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let sessions_dir = dir.path().join("codex/sessions/2026/06/12");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("rollout-2026-06-12T15-21-07-codex-session-1.jsonl"),
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "timestamp": "2026-06-12T13:22:05.798Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "codex-session-1",
                        "cwd": project.to_string_lossy(),
                    }
                }),
                serde_json::json!({
                    "type": "turn_context",
                    "payload": {
                        "cwd": "/tmp/ignored",
                    }
                })
            ),
        )
        .unwrap();

        assert_eq!(
            super::codex_session_cwd_from_home(&dir.path().join("codex"), "codex-session-1")
                .as_deref(),
            Some(project.as_path())
        );
    }

    #[test]
    fn resume_command_rejects_unsupported_or_flag_like_session_ids() {
        assert!(super::agent_resume_command(AgentKind::Custom, "custom-session").is_err());
        assert!(super::agent_resume_command(AgentKind::Codex, "").is_err());
        assert!(super::agent_resume_command(AgentKind::Codex, "   ").is_err());
        assert!(super::agent_resume_command(AgentKind::Codex, "--last").is_err());
        assert!(super::agent_resume_command(AgentKind::ClaudeCode, "abc\n123").is_err());
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
