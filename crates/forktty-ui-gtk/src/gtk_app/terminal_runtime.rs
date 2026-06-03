use super::*;
use forktty_terminal::ghostty::{
    core::{GhosttyCore, GhosttyCoreOptions},
    events::GhosttyEvent,
    pty::{PtySession, PtySize},
};
use std::os::unix::process::ExitStatusExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalSpawnPid(pub i32);

#[derive(Debug)]
pub(super) struct TerminalRuntime {
    core: GhosttyCore,
    pty: PtySession,
    size: PtySize,
    exit_reported: bool,
    #[cfg(test)]
    pty_writes: Vec<Vec<u8>>,
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
        Ok(Self {
            core,
            pty,
            size,
            exit_reported: false,
            #[cfg(test)]
            pty_writes: Vec::new(),
        })
    }

    pub(super) fn child_pid(&self) -> TerminalSpawnPid {
        TerminalSpawnPid(self.pty.child_id() as i32)
    }

    pub(super) fn write_text(&mut self, text: &str) -> Result<(), TerminalError> {
        self.write_bytes(text.as_bytes())
    }

    pub(super) fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), TerminalError> {
        #[cfg(test)]
        self.pty_writes.push(bytes.to_vec());
        self.pty
            .write_all(bytes)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn resize_pixels(
        &mut self,
        width_px: i32,
        height_px: i32,
        cell_width_px: i32,
        cell_height_px: i32,
    ) -> Result<(), TerminalError> {
        let cols = pixel_cells(width_px, cell_width_px);
        let rows = pixel_cells(height_px, cell_height_px);
        self.resize_cells(cols, rows)
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

    pub(super) fn paste_text(&mut self, text: &str) -> Result<(), TerminalError> {
        let bytes = self
            .core
            .paste_bytes(text)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.write_bytes(&bytes)
    }

    pub(super) fn pump_pty(&mut self) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let mut events = Vec::new();
        let bytes = self
            .pty
            .read_available()
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        if !bytes.is_empty() {
            events.extend(self.feed_pty_bytes(&bytes)?);
        }
        if !self.exit_reported {
            if let Some(status) = self
                .pty
                .try_wait()
                .map_err(|err| TerminalError::Backend(err.to_string()))?
            {
                self.exit_reported = true;
                events.push(GhosttyEvent::ChildExit {
                    status: exit_status_code(status),
                });
            }
        }
        Ok(events)
    }

    pub(super) fn feed_pty_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let events = self
            .core
            .feed(bytes)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        for event in &events {
            if let GhosttyEvent::PtyWrite(bytes) = event {
                self.write_bytes(bytes)?;
            }
        }
        Ok(events)
    }

    pub(super) fn visible_text(&self) -> String {
        self.core.visible_text().unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn size(&self) -> PtySize {
        self.size
    }

    #[cfg(test)]
    pub(super) fn pty_writes(&self) -> Vec<Vec<u8>> {
        self.pty_writes.clone()
    }
}

fn pixel_cells(pixels: i32, cell_pixels: i32) -> u16 {
    let cell_pixels = cell_pixels.max(1);
    ((pixels.max(1) / cell_pixels).max(1)).min(i32::from(u16::MAX)) as u16
}

fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(test)]
pub(super) struct TestTerminalRuntimeHarness {
    backend: GtkTerminalBackend,
    _receiver: mpsc::Receiver<GtkTerminalCommand>,
    runtimes: RefCell<BTreeMap<String, TerminalRuntime>>,
    statuses: RefCell<BTreeMap<String, String>>,
}

#[cfg(test)]
impl TestTerminalRuntimeHarness {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            backend: GtkTerminalBackend::new(sender),
            _receiver: receiver,
            runtimes: RefCell::new(BTreeMap::new()),
            statuses: RefCell::new(BTreeMap::new()),
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

    pub(super) fn new_ready(surface_id: &str) -> Self {
        let harness = Self::new();
        let mut request = SpawnRequest {
            surface_id: surface_id.to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        harness.spawn(request);
        harness
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

    pub(super) fn controller_send_text(&self, surface_id: &str, text: &str) {
        self.runtimes
            .borrow_mut()
            .get_mut(surface_id)
            .unwrap()
            .write_text(text)
            .unwrap();
    }

    pub(super) fn pty_writes(&self, surface_id: &str) -> Vec<Vec<u8>> {
        self.runtimes
            .borrow()
            .get(surface_id)
            .map(TerminalRuntime::pty_writes)
            .unwrap_or_default()
    }

    pub(super) fn resize_pixels(
        &self,
        surface_id: &str,
        width_px: i32,
        height_px: i32,
        cell_width_px: i32,
        cell_height_px: i32,
    ) {
        self.runtimes
            .borrow_mut()
            .get_mut(surface_id)
            .unwrap()
            .resize_pixels(width_px, height_px, cell_width_px, cell_height_px)
            .unwrap();
    }

    pub(super) fn runtime_size(&self, surface_id: &str) -> Option<(u16, u16)> {
        self.runtimes.borrow().get(surface_id).map(|runtime| {
            let size = runtime.size();
            (size.cols, size.rows)
        })
    }

    pub(super) fn simulate_child_exit(&self, surface_id: &str, status: i32) {
        self.backend.mark_surface_not_ready(surface_id).unwrap();
        self.statuses
            .borrow_mut()
            .insert(surface_id.to_string(), format!("Exited ({status})"));
    }

    pub(super) fn status_text(&self, surface_id: &str) -> String {
        self.statuses
            .borrow()
            .get(surface_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn restart_pane(&self, surface_id: &str) {
        self.runtimes.borrow_mut().remove(surface_id);
        self.statuses.borrow_mut().remove(surface_id);
        let mut request = SpawnRequest {
            surface_id: surface_id.to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        self.spawn(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_send_text_writes_to_runtime_pty() {
        let harness = TestTerminalRuntimeHarness::new_ready("surface-1");

        harness.controller_send_text("surface-1", "echo ok\n");

        assert_eq!(
            harness.pty_writes("surface-1"),
            vec![b"echo ok\n".to_vec()]
        );
    }

    #[test]
    fn allocation_resize_updates_pty_and_core() {
        let harness = TestTerminalRuntimeHarness::new_ready("surface-1");

        harness.resize_pixels("surface-1", 800, 480, 10, 20);

        assert_eq!(harness.runtime_size("surface-1"), Some((80, 24)));
    }

    #[test]
    fn child_exit_marks_surface_not_ready_and_restart_respawns() {
        let harness = TestTerminalRuntimeHarness::new_ready("surface-1");

        harness.simulate_child_exit("surface-1", 7);

        assert!(!harness.backend_ready("surface-1"));
        assert!(harness.status_text("surface-1").contains("Exited (7)"));

        harness.restart_pane("surface-1");

        assert!(harness.backend_ready("surface-1"));
    }
}
