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

/// Delete log files older than `max_age_days`. Called on startup.
pub fn prune_old_logs(max_age_days: u32) -> Result<(), SessionError> {
    let dir = log_dir()?;
    if !dir.exists() {
        return Ok(());
    }
    let cutoff = chrono::Local::now() - chrono::Duration::days(i64::from(max_age_days));
    let cutoff_str = cutoff.format("forktty-%Y-%m-%d.log").to_string();

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("forktty-") && name_str.ends_with(".log") && *name_str < *cutoff_str
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
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

    // --- Test 3: prune_old_logs date comparison ---

    /// Helper: run the same pruning logic as `prune_old_logs` but against a custom directory.
    /// This avoids coupling to `log_dir()` which depends on `dirs::data_local_dir()`.
    fn prune_logs_in_dir(dir: &std::path::Path, max_age_days: u32) {
        let cutoff = chrono::Local::now() - chrono::Duration::days(i64::from(max_age_days));
        let cutoff_str = cutoff.format("forktty-%Y-%m-%d.log").to_string();

        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("forktty-") && name_str.ends_with(".log") && *name_str < *cutoff_str
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    #[test]
    fn prune_deletes_old_logs_and_keeps_recent_and_future() {
        let tmp = std::env::temp_dir().join(format!("forktty-prune-test-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        // Use today's date to compute a "recent" file that won't be pruned
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let old_file = tmp.join("forktty-2020-01-01.log");
        let recent_file = tmp.join(format!("forktty-{today}.log"));
        let future_file = tmp.join("forktty-9999-12-31.log");
        let non_matching = tmp.join("other.txt");

        for f in [&old_file, &recent_file, &future_file, &non_matching] {
            fs::write(f, "test content").unwrap();
        }

        // Prune with 30 day retention (2020-01-01 is definitely old)
        prune_logs_in_dir(&tmp, 30);

        // Old file should be deleted
        assert!(
            !old_file.exists(),
            "forktty-2020-01-01.log should have been pruned"
        );
        // Today's file and future file should remain
        assert!(
            recent_file.exists(),
            "Today's log file should NOT have been pruned"
        );
        assert!(
            future_file.exists(),
            "forktty-9999-12-31.log should NOT have been pruned"
        );
        // Non-matching file should not be touched
        assert!(
            non_matching.exists(),
            "other.txt should NOT have been deleted"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn prune_does_not_delete_non_matching_files() {
        let tmp = std::env::temp_dir().join(format!("forktty-prune-nomatch-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        let files = [
            "other.txt",
            "forktty.log",           // missing date
            "forktty-abc.log",       // non-date pattern
            "readme.md",
        ];
        for name in &files {
            fs::write(tmp.join(name), "content").unwrap();
        }

        prune_logs_in_dir(&tmp, 0); // max_age_days=0 means prune everything matching

        // None of these should be deleted because they don't match the pattern
        for name in &files {
            assert!(
                tmp.join(name).exists(),
                "{name} should NOT have been deleted"
            );
        }

        let _ = fs::remove_dir_all(&tmp);
    }
}
