use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Data directory not found")]
    NoDataDir,
}

/// Serializable workspace layout for session restore.
/// Only stores structure — no scrollback, no PTY handles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub workspaces: Vec<WorkspaceSnapshot>,
    pub active_workspace_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub name: String,
    pub working_dir: String,
    pub git_branch: String,
    pub worktree_dir: String,
    pub worktree_name: String,
    pub pane_tree: PaneTreeSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PaneTreeSnapshot {
    #[serde(rename = "leaf")]
    Leaf,
    #[serde(rename = "horizontal")]
    Horizontal {
        children: Vec<PaneTreeSnapshot>,
        sizes: Vec<f64>,
    },
    #[serde(rename = "vertical")]
    Vertical {
        children: Vec<PaneTreeSnapshot>,
        sizes: Vec<f64>,
    },
}

fn data_dir() -> Result<PathBuf, SessionError> {
    dirs::data_local_dir()
        .map(|d| d.join("forktty"))
        .ok_or(SessionError::NoDataDir)
}

fn session_path() -> Result<PathBuf, SessionError> {
    Ok(data_dir()?.join("session.json"))
}

pub fn save_session(data: &SessionData) -> Result<(), SessionError> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(data)?;
    fs::write(session_path()?, json)?;
    Ok(())
}

pub fn load_session() -> Result<Option<SessionData>, SessionError> {
    let path = session_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)?;
    let data: SessionData = serde_json::from_str(&content)?;
    Ok(Some(data))
}

#[allow(dead_code)]
pub fn clear_session() -> Result<(), SessionError> {
    let path = session_path()?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

// --- Logging ---

fn log_dir() -> Result<PathBuf, SessionError> {
    Ok(data_dir()?.join("logs"))
}

pub fn log_path() -> Result<PathBuf, SessionError> {
    let dir = log_dir()?;
    fs::create_dir_all(&dir)?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    Ok(dir.join(format!("forktty-{date}.log")))
}

pub fn write_log(level: &str, message: &str) -> Result<(), SessionError> {
    let path = log_path()?;
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
    let safe_level = level.replace(['\n', '\r'], "_");
    let safe_message = message.replace(['\n', '\r'], " ");
    let line = format!("[{timestamp}] [{safe_level}] {safe_message}\n");
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_roundtrip() {
        let data = SessionData {
            workspaces: vec![WorkspaceSnapshot {
                name: "Test".to_string(),
                working_dir: "/tmp".to_string(),
                git_branch: "main".to_string(),
                worktree_dir: String::new(),
                worktree_name: String::new(),
                pane_tree: PaneTreeSnapshot::Horizontal {
                    children: vec![PaneTreeSnapshot::Leaf, PaneTreeSnapshot::Leaf],
                    sizes: vec![50.0, 50.0],
                },
            }],
            active_workspace_index: 0,
        };

        let json = serde_json::to_string(&data).unwrap();
        let parsed: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workspaces.len(), 1);
        assert_eq!(parsed.workspaces[0].name, "Test");
    }
}
