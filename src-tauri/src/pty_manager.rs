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

impl PtyManager {
    pub fn new() -> Self {
        Self {
            ptys: HashMap::new(),
            next_id: 1,
        }
    }

    /// Spawn a new PTY with the given shell and dimensions.
    /// Returns (pty_id, reader) where reader is for the background read loop.
    pub fn spawn(
        &mut self,
        shell: &str,
        cols: u16,
        rows: u16,
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
        self.next_id += 1;

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
        writer.flush().map_err(|e| PtyError::Write(e.to_string()))?;
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

    /// Kill a PTY process and remove it.
    pub fn kill(&mut self, id: u32) -> Result<(), PtyError> {
        let handle = self.ptys.remove(&id).ok_or(PtyError::NotFound(id))?;
        if let Ok(mut child) = handle.child.lock() {
            let _ = child.kill();
        }
        Ok(())
    }
}
