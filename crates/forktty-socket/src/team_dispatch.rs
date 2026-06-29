use crate::{DispatchError, SocketAppState};
use forktty_terminal::TerminalError;
use std::time::{Duration, Instant};

const TEAM_MESSAGE_SURFACE_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TEAM_MESSAGE_SURFACE_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Fresh provider TUIs can accept PTY bytes before their input prompt is ready.
/// Keep the first visible team prompt from landing in startup output.
const TEAM_PROVIDER_INITIAL_PROMPT_SETTLE: Duration = Duration::from_secs(1);
/// When a provider needs Enter as a distinct submit action, avoid issuing it
/// in the same scheduler turn as the prompt body so embedded PTYs are less
/// likely to coalesce the writes into one paste-like input chunk.
const TEAM_SEPARATE_SUBMIT_ENTER_SETTLE: Duration = Duration::from_millis(100);

pub(crate) async fn dispatch_team_message_text(
    state: &SocketAppState,
    surface_id: &str,
    body: &str,
    submit: bool,
    agent: Option<&str>,
    launched_at_ms: Option<u64>,
) -> Result<(), DispatchError> {
    show_team_message_surface(state, surface_id)?;
    wait_for_provider_initial_prompt_settle(agent, launched_at_ms).await;
    let (text, separate_enter) = terminal_text_and_separate_enter(body, submit, agent);
    send_team_message_body_when_ready(state, surface_id, &text).await?;
    if separate_enter {
        send_team_submit_enter_after_settle(state, surface_id).await?;
    }
    Ok(())
}

pub(crate) async fn send_team_submit_enter_after_settle(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    tokio::time::sleep(TEAM_SEPARATE_SUBMIT_ENTER_SETTLE).await;
    state
        .terminal
        .send_enter(surface_id)
        .map_err(DispatchError::from)
}

pub(crate) fn terminal_text_with_submit_enter(text: &str, submit: bool) -> String {
    if submit && !text.ends_with('\r') {
        let mut submitted = String::with_capacity(text.len() + 1);
        submitted.push_str(text);
        submitted.push('\r');
        submitted
    } else {
        text.to_string()
    }
}

pub(crate) fn terminal_text_and_separate_enter(
    text: &str,
    submit: bool,
    agent: Option<&str>,
) -> (String, bool) {
    if submit && !text.ends_with('\r') && agent_uses_separate_submit_enter(agent) {
        (text.to_string(), true)
    } else {
        (terminal_text_with_submit_enter(text, submit), false)
    }
}

fn agent_uses_separate_submit_enter(agent: Option<&str>) -> bool {
    matches!(
        normalized_agent(agent).as_str(),
        "claude" | "claude_code" | "claude-code" | "pi"
    )
}

async fn wait_for_provider_initial_prompt_settle(agent: Option<&str>, launched_at_ms: Option<u64>) {
    if !agent_needs_initial_prompt_settle(agent) {
        return;
    }
    let Some(launched_at_ms) = launched_at_ms else {
        return;
    };
    let elapsed_ms = forktty_core::team_now_ms().saturating_sub(launched_at_ms);
    let settle_ms = TEAM_PROVIDER_INITIAL_PROMPT_SETTLE.as_millis() as u64;
    if elapsed_ms < settle_ms {
        tokio::time::sleep(Duration::from_millis(settle_ms - elapsed_ms)).await;
    }
}

fn agent_needs_initial_prompt_settle(agent: Option<&str>) -> bool {
    matches!(
        normalized_agent(agent).as_str(),
        "claude" | "claude_code" | "claude-code" | "pi"
    )
}

fn normalized_agent(agent: Option<&str>) -> String {
    agent
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .to_string()
}

fn show_team_message_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if !model.focus_surface_and_select_workspace(surface_id) {
            return Err(DispatchError::NotFound("surface".to_string()));
        }
    }
    state
        .terminal
        .show_surface(surface_id)
        .map_err(DispatchError::from)
}

async fn send_team_message_body_when_ready(
    state: &SocketAppState,
    surface_id: &str,
    body: &str,
) -> Result<(), DispatchError> {
    let started = Instant::now();
    loop {
        match state.terminal.send_text(surface_id, body) {
            Ok(()) => return Ok(()),
            Err(TerminalError::NotReady(_))
                if started.elapsed() < TEAM_MESSAGE_SURFACE_READY_TIMEOUT =>
            {
                tokio::time::sleep(TEAM_MESSAGE_SURFACE_READY_POLL_INTERVAL).await;
            }
            Err(err) => return Err(DispatchError::from(err)),
        }
    }
}
