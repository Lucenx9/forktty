use forktty_core::{SurfaceId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[cfg(feature = "vte")]
pub mod vte;

#[derive(Error, Debug)]
pub enum TerminalError {
    #[error("Terminal surface not found: {0}")]
    NotFound(String),
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
    pub cwd: PathBuf,
    pub socket_path: PathBuf,
    #[serde(default)]
    pub extra_env: Vec<(String, String)>,
}

impl SpawnRequest {
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
}

pub trait TerminalBackend: Send + Sync {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError>;
    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError>;
    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError>;
    fn close(&self, surface_id: &str) -> Result<(), TerminalError>;
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
                },
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
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: vec![
                ("EXTRA".to_string(), "1".to_string()),
                ("TERM".to_string(), "dumb".to_string()),
                ("COLORTERM".to_string(), "8bit".to_string()),
                ("FORKTTY_SURFACE_ID".to_string(), "spoofed".to_string()),
            ],
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
        backend.close("surface-1").unwrap();
        assert!(matches!(
            backend.sent_text("surface-1"),
            Err(TerminalError::NotFound(_))
        ));
    }
}
