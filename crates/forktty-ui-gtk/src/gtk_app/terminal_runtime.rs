use super::*;
use forktty_terminal::ghostty::{
    core::{GhosttyCore, GhosttyCoreOptions},
    pty::{PtySession, PtySize},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalSpawnPid(pub i32);

#[derive(Debug)]
pub(super) struct TerminalRuntime {
    core: GhosttyCore,
    pty: PtySession,
    size: PtySize,
}

impl TerminalRuntime {
    pub(super) fn spawn(request: &SpawnRequest, size: PtySize) -> Result<Self, TerminalError> {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: size.cols,
            rows: size.rows,
            scrollback_lines: config::AppConfig::default().appearance.scrollback_lines as usize,
        })
        .map_err(|err| TerminalError::Backend(err.to_string()))?;
        let pty = PtySession::spawn(request, size)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        Ok(Self { core, pty, size })
    }

    pub(super) fn child_pid(&self) -> TerminalSpawnPid {
        TerminalSpawnPid(self.pty.child_id() as i32)
    }

    pub(super) fn write_text(&mut self, text: &str) -> Result<(), TerminalError> {
        self.pty
            .write_all(text.as_bytes())
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn resize_cells(&mut self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let size = PtySize { cols, rows };
        self.pty
            .resize(size)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.core
            .resize(cols, rows, 10, 20)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.size = size;
        Ok(())
    }

    pub(super) fn reset_and_clear(&mut self) -> Result<(), TerminalError> {
        self.core
            .reset()
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.write_text("\x0c")
    }

    pub(super) fn visible_text(&self) -> String {
        self.core.visible_text().unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn size(&self) -> PtySize {
        self.size
    }
}

#[cfg(test)]
pub(super) struct TestTerminalRuntimeHarness {
    backend: GtkTerminalBackend,
    _receiver: mpsc::Receiver<GtkTerminalCommand>,
    runtimes: RefCell<BTreeMap<String, TerminalRuntime>>,
}

#[cfg(test)]
impl TestTerminalRuntimeHarness {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            backend: GtkTerminalBackend::new(sender),
            _receiver: receiver,
            runtimes: RefCell::new(BTreeMap::new()),
        }
    }

    pub(super) fn spawn(&self, request: SpawnRequest) {
        self.backend.spawn(request.clone()).unwrap();
        let runtime = TerminalRuntime::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
        self.backend.mark_surface_ready(&request.surface_id).unwrap();
        self.runtimes
            .borrow_mut()
            .insert(request.surface_id.clone(), runtime);
    }

    pub(super) fn backend_ready(&self, surface_id: &str) -> bool {
        self.backend.send_text(surface_id, "").is_ok()
    }

    pub(super) fn child_pid(&self, surface_id: &str) -> Option<i32> {
        self.runtimes
            .borrow()
            .get(surface_id)
            .map(|runtime| runtime.child_pid().0)
    }
}
