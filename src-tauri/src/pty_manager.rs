use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PtyError {
    #[error("PTY creation failed: {0}")]
    Creation(String),
    #[error("PTY spawn failed: {0}")]
    Spawn(String),
    #[error("PTY cwd lookup failed: {0}")]
    Cwd(String),
    #[error("PTY not found: {0}")]
    NotFound(u32),
    #[error("PTY write failed: {0}")]
    Write(String),
    #[error("PTY resize failed: {0}")]
    Resize(String),
    #[error("Lock poisoned")]
    LockPoisoned,
}

pub struct PtyHandle {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    #[allow(dead_code)] // Used in kill() for PTY lifecycle (Phase 2+)
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
}

pub struct PtyManager {
    ptys: HashMap<u32, PtyHandle>,
    next_id: u32,
}

fn fallback_spawn_cwd() -> String {
    let home = std::env::var("HOME").ok();
    if let Some(home_dir) = home {
        let path = std::path::Path::new(&home_dir);
        if path.is_absolute() && path.exists() {
            return home_dir;
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        let cwd = current_dir.to_string_lossy().to_string();
        if !cwd.starts_with("/tmp/.mount_") {
            return cwd;
        }
    }

    "/".to_string()
}

fn resolve_spawn_cwd(dir: &str) -> String {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return fallback_spawn_cwd();
    }

    // AppImage mounts at /tmp/.mount_*, which is not a valid shell CWD.
    if trimmed.starts_with("/tmp/.mount_") {
        return fallback_spawn_cwd();
    }

    let path = std::path::Path::new(trimmed);
    if path.is_absolute() && path.exists() {
        trimmed.to_string()
    } else {
        fallback_spawn_cwd()
    }
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            ptys: HashMap::new(),
            next_id: 1,
        }
    }

    /// Spawn a new PTY with the given shell, dimensions, optional working directory,
    /// and optional env vars for workspace/surface/socket identification.
    pub fn spawn(
        &mut self,
        shell: &str,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        env_vars: Option<&[(&str, &str)]>,
    ) -> Result<(u32, Box<dyn Read + Send>), PtyError> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Creation(e.to_string()))?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        if let Some(dir) = cwd {
            // Sessions can contain stale worktree paths; fall back to a safe directory
            // instead of bricking the terminal on startup.
            let effective_dir = resolve_spawn_cwd(dir);
            cmd.cwd(&effective_dir);
        }
        if let Some(vars) = env_vars {
            for (key, val) in vars {
                cmd.env(key, val);
            }
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        // Drop slave so master gets EOF when child exits
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Creation(e.to_string()))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Creation(e.to_string()))?;

        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(PtyError::Creation("PTY ID overflow".to_string()))?;

        self.ptys.insert(
            id,
            PtyHandle {
                writer: Arc::new(Mutex::new(writer)),
                master: Arc::new(Mutex::new(pair.master)),
                child: Arc::new(Mutex::new(child)),
            },
        );

        Ok((id, reader))
    }

    /// Write data to a PTY's stdin.
    pub fn write(&self, id: u32, data: &[u8]) -> Result<(), PtyError> {
        let handle = self.ptys.get(&id).ok_or(PtyError::NotFound(id))?;
        let mut writer = handle.writer.lock().map_err(|_| PtyError::LockPoisoned)?;
        writer
            .write_all(data)
            .map_err(|e| PtyError::Write(e.to_string()))?;
        Ok(())
    }

    /// Resize a PTY.
    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), PtyError> {
        let handle = self.ptys.get(&id).ok_or(PtyError::NotFound(id))?;
        let master = handle.master.lock().map_err(|_| PtyError::LockPoisoned)?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Resize(e.to_string()))?;
        Ok(())
    }

    /// Return the current working directory of the PTY's shell process.
    pub fn cwd(&self, id: u32) -> Result<String, PtyError> {
        let handle = self.ptys.get(&id).ok_or(PtyError::NotFound(id))?;
        let child = handle.child.lock().map_err(|_| PtyError::LockPoisoned)?;
        let pid = child
            .process_id()
            .ok_or_else(|| PtyError::Cwd("Child process has no PID".to_string()))?;
        drop(child);

        #[cfg(target_os = "linux")]
        {
            let path = std::fs::read_link(format!("/proc/{pid}/cwd"))
                .map_err(|e| PtyError::Cwd(e.to_string()))?;
            let cwd = path.to_string_lossy().to_string();
            if cwd.starts_with("/tmp/.mount_") {
                return std::env::var("HOME").map_err(|e| PtyError::Cwd(format!("No HOME: {e}")));
            }
            return Ok(cwd);
        }

        #[allow(unreachable_code)]
        Err(PtyError::Cwd(
            "PTY cwd lookup is only supported on Linux".to_string(),
        ))
    }

    /// Kill a PTY process and remove it.
    pub fn kill(&mut self, id: u32) -> Result<(), PtyError> {
        let handle = self.ptys.remove(&id).ok_or(PtyError::NotFound(id))?;
        if let Ok(mut child) = handle.child.lock() {
            let _ = child.kill();
            // Reap the process to prevent zombies
            let _ = child.wait();
        }
        Ok(())
    }

    /// Reap a PTY after the child exits and remove it from the manager.
    pub fn reap(&mut self, id: u32) {
        if let Some(handle) = self.ptys.remove(&id) {
            if let Ok(mut child) = handle.child.lock() {
                let _ = child.wait();
            }
        }
    }

    /// Kill and reap all PTY processes (used on shutdown).
    pub fn kill_all(&mut self) {
        for (_id, handle) in self.ptys.drain() {
            if let Ok(mut child) = handle.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_spawn_cwd;
    use std::fs;
    use std::path::Path;

    #[test]
    fn resolve_spawn_cwd_keeps_existing_absolute_path() {
        let dir = std::env::temp_dir().join(format!("forktty-pty-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let resolved = resolve_spawn_cwd(dir.to_str().unwrap());
        assert_eq!(resolved, dir.to_string_lossy());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_spawn_cwd_falls_back_for_missing_path() {
        let missing = format!("/tmp/forktty-missing-{}", std::process::id());
        let resolved = resolve_spawn_cwd(&missing);

        assert_ne!(resolved, missing);
        assert!(Path::new(&resolved).is_absolute());
        assert!(Path::new(&resolved).exists());
    }
}
