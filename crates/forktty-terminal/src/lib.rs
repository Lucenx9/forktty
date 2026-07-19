use forktty_core::{Surface, SurfaceId, Workspace, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[cfg(feature = "ghostty-vt")]
pub mod ghostty;
pub mod spawn;

#[derive(Error, Debug)]
pub enum TerminalError {
    #[error("Terminal surface not found: {0}")]
    NotFound(String),
    #[error("Terminal surface not ready: {0}")]
    NotReady(String),
    #[error("Terminal backend error: {0}")]
    Backend(String),
    #[error("Lock poisoned")]
    LockPoisoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnRequest {
    pub surface_id: SurfaceId,
    pub workspace_id: WorkspaceId,
    pub shell: String,
    /// Extra arguments appended after `shell` when building the argv.
    /// Empty for a normal interactive shell; `["user@host"]` for SSH.
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub socket_path: PathBuf,
    #[serde(default)]
    pub extra_env: Vec<(String, String)>,
    /// Whether this spawn is an explicitly plain interactive terminal shell
    /// eligible for opt-in PTY process persistence. Command launches that are
    /// not ordinary shells (project actions, remote commands, SSH, and agent
    /// resumes) must keep this false so their process trees do not survive the
    /// visible pane lifecycle unexpectedly.
    #[serde(default)]
    pub eligible_for_pty_persistence: bool,
}

impl SpawnRequest {
    pub fn for_workspace(
        workspace: &Workspace,
        shell: impl Into<String>,
        socket_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            surface_id: workspace.focused_surface_id.clone(),
            workspace_id: workspace.id.clone(),
            shell: shell.into(),
            args: Vec::new(),
            cwd: workspace.working_dir.clone(),
            socket_path: socket_path.into(),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: true,
        }
    }

    pub fn for_surface(
        surface: &Surface,
        shell: impl Into<String>,
        socket_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            surface_id: surface.id.clone(),
            workspace_id: surface.workspace_id.clone(),
            shell: shell.into(),
            args: Vec::new(),
            cwd: surface.cwd.clone(),
            socket_path: socket_path.into(),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: true,
        }
    }

    /// Return a clone of this request with `args` set to `args`.
    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self.eligible_for_pty_persistence = false;
        self
    }

    /// Resolve the full argv: `[shell, ...args]`.
    pub fn argv(&self) -> Vec<&str> {
        let mut argv = Vec::with_capacity(1 + self.args.len());
        argv.push(self.shell.as_str());
        argv.extend(self.args.iter().map(String::as_str));
        argv
    }

    pub fn forktty_env(&self) -> Vec<(String, String)> {
        let mut env = self
            .extra_env
            .iter()
            .filter(|(key, _)| !is_reserved_terminal_env(key))
            .cloned()
            .collect::<Vec<_>>();
        env.push((
            "FORKTTY_WORKSPACE_ID".to_string(),
            self.workspace_id.clone(),
        ));
        env.push(("FORKTTY_SURFACE_ID".to_string(), self.surface_id.clone()));
        env.push((
            "FORKTTY_SOCKET_PATH".to_string(),
            self.socket_path.to_string_lossy().to_string(),
        ));
        env.push(("TERM".to_string(), "xterm-256color".to_string()));
        env.push(("COLORTERM".to_string(), "truecolor".to_string()));
        env.push(("TERM_PROGRAM".to_string(), "ForkTTY".to_string()));
        env.push((
            "TERM_PROGRAM_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ));
        env
    }
}

fn is_reserved_terminal_env(key: &str) -> bool {
    matches!(
        key,
        "TERM" | "COLORTERM" | "TERM_PROGRAM" | "TERM_PROGRAM_VERSION"
    ) || key.starts_with("FORKTTY_")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalSurfaceState {
    pub surface_id: SurfaceId,
    pub workspace_id: WorkspaceId,
    pub cwd: PathBuf,
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalTextCapture {
    Visible,
    All,
    Tail { lines: usize },
}

impl TerminalTextCapture {
    fn scope(&self) -> &'static str {
        match self {
            TerminalTextCapture::Visible => "visible",
            TerminalTextCapture::All => "all",
            TerminalTextCapture::Tail { .. } => "tail",
        }
    }

    fn truncates_from_end(&self) -> bool {
        matches!(self, TerminalTextCapture::Tail { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalTextSnapshot {
    pub surface_id: SurfaceId,
    pub scope: String,
    pub text: String,
    pub cols: u16,
    pub rows: u16,
    pub total_lines: usize,
    pub lines: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTextSnapshotParts {
    pub surface_id: SurfaceId,
    pub scope: String,
    pub text: String,
    pub cols: u16,
    pub rows: u16,
    pub total_lines: usize,
    pub max_bytes: usize,
    pub truncate_from_end: bool,
}

impl TerminalTextSnapshot {
    pub fn from_text(
        surface_id: impl Into<SurfaceId>,
        text: impl Into<String>,
        cols: u16,
        rows: u16,
        capture: TerminalTextCapture,
        max_bytes: usize,
    ) -> Self {
        let scope = capture.scope().to_string();
        let truncates_from_end = capture.truncates_from_end();
        let source = text.into();
        let total_lines = text_line_count(&source);
        let captured = match capture {
            TerminalTextCapture::Visible | TerminalTextCapture::All => source,
            TerminalTextCapture::Tail { lines } => tail_text_lines(&source, lines),
        };
        Self::from_captured_text(TerminalTextSnapshotParts {
            surface_id: surface_id.into(),
            scope,
            text: captured,
            cols,
            rows,
            total_lines,
            max_bytes,
            truncate_from_end: truncates_from_end,
        })
    }

    pub fn from_captured_text(parts: TerminalTextSnapshotParts) -> Self {
        let (text, truncated) = truncate_text(parts.text, parts.max_bytes, parts.truncate_from_end);
        let lines = text_line_count(&text);
        Self {
            surface_id: parts.surface_id,
            scope: parts.scope,
            text,
            cols: parts.cols,
            rows: parts.rows,
            total_lines: parts.total_lines,
            lines,
            truncated,
        }
    }
}

fn text_line_count(text: &str) -> usize {
    text.lines().count()
}

fn tail_text_lines(text: &str, lines: usize) -> String {
    if lines == 0 || text.is_empty() {
        return String::new();
    }
    let chunks = text.split_inclusive('\n').collect::<Vec<_>>();
    let start = chunks.len().saturating_sub(lines);
    chunks[start..].concat()
}

fn truncate_text(text: String, max_bytes: usize, from_end: bool) -> (String, bool) {
    if max_bytes == 0 {
        return (String::new(), !text.is_empty());
    }
    if text.len() <= max_bytes {
        return (text, false);
    }
    if from_end {
        let mut start = text.len().saturating_sub(max_bytes);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        (text[start..].to_string(), true)
    } else {
        let mut end = max_bytes.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (text[..end].to_string(), true)
    }
}

pub trait TerminalBackend: Send + Sync {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError>;
    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError>;
    fn send_enter(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.send_text(surface_id, "\r")
    }
    fn show_surface(&self, _surface_id: &str) -> Result<(), TerminalError> {
        Ok(())
    }
    fn read_text(
        &self,
        surface_id: &str,
        _capture: TerminalTextCapture,
        _max_bytes: usize,
    ) -> Result<TerminalTextSnapshot, TerminalError> {
        Err(TerminalError::NotReady(surface_id.to_string()))
    }
    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError>;
    fn close(&self, surface_id: &str) -> Result<(), TerminalError>;
    fn mark_surface_ready(&self, _surface_id: &str) -> Result<(), TerminalError> {
        Ok(())
    }
    fn mark_surface_not_ready(&self, _surface_id: &str) -> Result<(), TerminalError> {
        Ok(())
    }
    fn mark_surface_pid(&self, _surface_id: &str, _pid: u32) -> Result<(), TerminalError> {
        Ok(())
    }
    fn clear_surface_pid(&self, _surface_id: &str) -> Result<(), TerminalError> {
        Ok(())
    }
    fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.close(surface_id)
    }
    fn surface_ready(&self, surface_id: &str) -> Result<bool, TerminalError> {
        Ok(self
            .surfaces()?
            .iter()
            .any(|surface| surface.surface_id == surface_id))
    }
    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError>;
}

pub type SharedTerminalBackend = Arc<dyn TerminalBackend>;

#[derive(Debug, Default)]
pub struct HeadlessTerminalBackend {
    surfaces: Mutex<BTreeMap<SurfaceId, HeadlessSurface>>,
}

#[derive(Debug, Clone)]
struct HeadlessSurface {
    state: TerminalSurfaceState,
    /// The `args` from the original [`SpawnRequest`].
    args: Vec<String>,
    sent_text: Vec<String>,
    env: Vec<(String, String)>,
}

impl HeadlessTerminalBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sent_text(&self, surface_id: &str) -> Result<Vec<String>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(surface.sent_text.clone())
    }

    pub fn env(&self, surface_id: &str) -> Result<Vec<(String, String)>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(surface.env.clone())
    }

    /// Return the `args` recorded when this surface was spawned.
    pub fn spawn_args(&self, surface_id: &str) -> Result<Vec<String>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(surface.args.clone())
    }

    /// Return the `shell` recorded when this surface was spawned.
    pub fn spawn_shell(&self, surface_id: &str) -> Result<String, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(surface.state.shell.clone())
    }
}

impl TerminalBackend for HeadlessTerminalBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let env = request.forktty_env();
        surfaces.insert(
            request.surface_id.clone(),
            HeadlessSurface {
                state: TerminalSurfaceState {
                    surface_id: request.surface_id,
                    workspace_id: request.workspace_id,
                    cwd: request.cwd,
                    shell: request.shell,
                    cols: 80,
                    rows: 24,
                    pid: None,
                },
                args: request.args,
                sent_text: Vec::new(),
                env,
            },
        );
        Ok(())
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        surface.sent_text.push(text.to_string());
        Ok(())
    }

    fn read_text(
        &self,
        surface_id: &str,
        capture: TerminalTextCapture,
        max_bytes: usize,
    ) -> Result<TerminalTextSnapshot, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(TerminalTextSnapshot::from_text(
            surface_id,
            surface.sent_text.concat(),
            surface.state.cols,
            surface.state.rows,
            capture,
            max_bytes,
        ))
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        surface.state.cols = cols;
        surface.state.rows = rows;
        Ok(())
    }

    fn mark_surface_pid(&self, surface_id: &str, pid: u32) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        surface.state.pid = Some(pid);
        Ok(())
    }

    fn clear_surface_pid(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        surface.state.pid = None;
        Ok(())
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        surfaces
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(surfaces
            .values()
            .map(|surface| surface.state.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_backend_injects_forktty_env_and_records_text() {
        let backend = HeadlessTerminalBackend::new();
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: vec![
                ("EXTRA".to_string(), "1".to_string()),
                ("TERM".to_string(), "dumb".to_string()),
                ("COLORTERM".to_string(), "8bit".to_string()),
                ("FORKTTY_SURFACE_ID".to_string(), "spoofed".to_string()),
            ],
            eligible_for_pty_persistence: false,
        };

        backend.spawn(request).unwrap();
        backend.send_text("surface-1", "echo ok\n").unwrap();

        let env = backend.env("surface-1").unwrap();
        assert!(env.contains(&(
            "FORKTTY_WORKSPACE_ID".to_string(),
            "workspace-1".to_string()
        )));
        assert!(env.contains(&("FORKTTY_SURFACE_ID".to_string(), "surface-1".to_string())));
        assert!(env.contains(&(
            "FORKTTY_SOCKET_PATH".to_string(),
            "/tmp/forktty.sock".to_string()
        )));
        assert!(env.contains(&("TERM".to_string(), "xterm-256color".to_string())));
        assert!(env.contains(&("COLORTERM".to_string(), "truecolor".to_string())));
        assert!(env.contains(&("TERM_PROGRAM".to_string(), "ForkTTY".to_string())));
        assert!(env.contains(&(
            "TERM_PROGRAM_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string()
        )));
        assert!(env.contains(&("EXTRA".to_string(), "1".to_string())));
        assert!(!env.contains(&("FORKTTY_SURFACE_ID".to_string(), "spoofed".to_string())));
        assert!(!env.contains(&("TERM".to_string(), "dumb".to_string())));
        assert!(!env.contains(&("COLORTERM".to_string(), "8bit".to_string())));
        assert_eq!(backend.sent_text("surface-1").unwrap(), vec!["echo ok\n"]);
        assert_eq!(
            backend
                .read_text("surface-1", TerminalTextCapture::Visible, 1024)
                .unwrap()
                .text,
            "echo ok\n"
        );
        backend.close("surface-1").unwrap();
        assert!(matches!(
            backend.sent_text("surface-1"),
            Err(TerminalError::NotFound(_))
        ));
    }

    #[test]
    fn terminal_text_snapshot_zero_max_bytes_returns_empty_and_marks_truncated() {
        let visible = TerminalTextSnapshot::from_text(
            "surface-1",
            "hello",
            80,
            24,
            TerminalTextCapture::Visible,
            0,
        );
        assert_eq!(visible.text, "");
        assert!(visible.truncated);

        let tail = TerminalTextSnapshot::from_text(
            "surface-1",
            "hello",
            80,
            24,
            TerminalTextCapture::Tail { lines: 1 },
            0,
        );
        assert_eq!(tail.text, "");
        assert!(tail.truncated);
    }

    #[test]
    fn terminal_text_snapshot_zero_max_bytes_empty_input_is_not_truncated() {
        let snapshot = TerminalTextSnapshot::from_text(
            "surface-1",
            "",
            80,
            24,
            TerminalTextCapture::Visible,
            0,
        );

        assert_eq!(snapshot.text, "");
        assert!(!snapshot.truncated);
    }

    #[test]
    fn spawn_request_builders_preserve_workspace_and_surface_identity() {
        let mut model = forktty_core::WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp/main");
        let surface = model.surface(&workspace.focused_surface_id).unwrap();

        let workspace_request =
            SpawnRequest::for_workspace(&workspace, "/bin/sh", "/tmp/forktty.sock");
        assert_eq!(workspace_request.surface_id, workspace.focused_surface_id);
        assert_eq!(workspace_request.workspace_id, workspace.id);
        assert_eq!(workspace_request.cwd, PathBuf::from("/tmp/main"));
        assert_eq!(workspace_request.extra_env, Vec::<(String, String)>::new());
        assert_eq!(workspace_request.args, Vec::<String>::new());
        assert!(workspace_request.eligible_for_pty_persistence);

        let surface_request = SpawnRequest::for_surface(surface, "/bin/zsh", "/tmp/other.sock");
        assert_eq!(surface_request.surface_id, surface.id);
        assert_eq!(surface_request.workspace_id, surface.workspace_id);
        assert_eq!(surface_request.cwd, surface.cwd);
        assert_eq!(surface_request.shell, "/bin/zsh");
        assert!(surface_request.eligible_for_pty_persistence);
    }

    #[test]
    fn spawn_request_argv_assembles_shell_plus_args() {
        let request = SpawnRequest {
            surface_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            shell: "/usr/bin/ssh".to_string(),
            args: vec!["user@example.com".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        };
        assert_eq!(request.argv(), vec!["/usr/bin/ssh", "user@example.com"]);
    }

    #[test]
    fn spawn_request_argv_with_no_args_is_just_shell() {
        let request = SpawnRequest {
            surface_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            shell: "/bin/zsh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        };
        assert_eq!(request.argv(), vec!["/bin/zsh"]);
    }

    #[test]
    fn with_args_sets_args_and_leaves_other_fields_intact() {
        let mut model = forktty_core::WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp/main");
        let request = SpawnRequest::for_workspace(&workspace, "/usr/bin/ssh", "/tmp/forktty.sock")
            .with_args(["user@host"]);
        assert_eq!(request.shell, "/usr/bin/ssh");
        assert_eq!(request.args, vec!["user@host"]);
        assert!(!request.eligible_for_pty_persistence);
    }

    #[test]
    fn headless_backend_records_args_on_spawn() {
        let backend = HeadlessTerminalBackend::new();
        let request = SpawnRequest {
            surface_id: "surface-ssh".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/usr/bin/ssh".to_string(),
            args: vec!["user@example.com".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        };
        backend.spawn(request).unwrap();
        assert_eq!(
            backend.spawn_args("surface-ssh").unwrap(),
            vec!["user@example.com"]
        );
        assert_eq!(backend.spawn_shell("surface-ssh").unwrap(), "/usr/bin/ssh");
    }

    #[test]
    fn child_environment_is_available_without_terminal_ui_feature() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        };

        let env = crate::spawn::child_environment(&request);

        assert!(env
            .iter()
            .any(|entry| entry == "TERM=xterm-256color" || entry == "TERM=xterm-ghostty"));
        assert!(env.iter().any(|entry| entry == "COLORTERM=truecolor"));
        assert!(env.iter().any(|entry| entry == "TERM_PROGRAM=ForkTTY"));
    }
}
