use super::*;

pub(super) enum GtkTerminalCommand {
    Spawn(SpawnRequest),
    SendText {
        surface_id: String,
        text: String,
    },
    ReadText {
        surface_id: String,
        capture: TerminalTextCapture,
        max_bytes: usize,
        reply: mpsc::Sender<Result<TerminalTextSnapshot, TerminalError>>,
    },
    Resize {
        surface_id: String,
        cols: u16,
        rows: u16,
    },
    Close {
        surface_id: String,
    },
}

#[derive(Debug, Clone)]
pub(super) struct GtkTerminalBackend {
    sender: mpsc::Sender<GtkTerminalCommand>,
    surfaces: Arc<Mutex<BTreeMap<String, TerminalSurfaceState>>>,
    ready_surfaces: Arc<Mutex<BTreeSet<String>>>,
}

impl GtkTerminalBackend {
    pub(super) fn new(sender: mpsc::Sender<GtkTerminalCommand>) -> Self {
        Self {
            sender,
            surfaces: Arc::new(Mutex::new(BTreeMap::new())),
            ready_surfaces: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    fn send_command(&self, command: GtkTerminalCommand) -> Result<(), TerminalError> {
        self.sender
            .send(command)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }
}

impl TerminalBackend for GtkTerminalBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let surface_id = request.surface_id.clone();
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        if surfaces.contains_key(&surface_id) {
            return Err(TerminalError::Backend(format!(
                "surface already exists: {surface_id}"
            )));
        }
        let previous = surfaces.insert(
            request.surface_id.clone(),
            TerminalSurfaceState {
                surface_id: request.surface_id.clone(),
                workspace_id: request.workspace_id.clone(),
                cwd: request.cwd.clone(),
                shell: request.shell.clone(),
                cols: 80,
                rows: 24,
                pid: None,
            },
        );
        drop(surfaces);
        let was_ready = match self.ready_surfaces.lock() {
            Ok(mut ready_surfaces) => ready_surfaces.remove(&surface_id),
            Err(_) => {
                if let Ok(mut surfaces) = self.surfaces.lock() {
                    if let Some(previous) = previous {
                        surfaces.insert(surface_id, previous);
                    } else {
                        surfaces.remove(&surface_id);
                    }
                }
                return Err(TerminalError::LockPoisoned);
            }
        };
        if let Err(err) = self.send_command(GtkTerminalCommand::Spawn(request)) {
            if let Ok(mut surfaces) = self.surfaces.lock() {
                if let Some(previous) = previous {
                    surfaces.insert(surface_id.clone(), previous);
                } else {
                    surfaces.remove(&surface_id);
                }
            }
            if let Ok(mut ready_surfaces) = self.ready_surfaces.lock() {
                if was_ready {
                    ready_surfaces.insert(surface_id.clone());
                } else {
                    ready_surfaces.remove(&surface_id);
                }
            }
            return Err(err);
        }
        Ok(())
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        {
            let surfaces = self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            if !surfaces.contains_key(surface_id) {
                return Err(TerminalError::NotFound(surface_id.to_string()));
            }
        }
        if !self
            .ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains(surface_id)
        {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        self.send_command(GtkTerminalCommand::SendText {
            surface_id: surface_id.to_string(),
            text: text.to_string(),
        })
    }

    fn read_text(
        &self,
        surface_id: &str,
        capture: TerminalTextCapture,
        max_bytes: usize,
    ) -> Result<TerminalTextSnapshot, TerminalError> {
        {
            let surfaces = self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            if !surfaces.contains_key(surface_id) {
                return Err(TerminalError::NotFound(surface_id.to_string()));
            }
        }
        if !self
            .ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains(surface_id)
        {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        let (reply, receiver) = mpsc::channel();
        self.send_command(GtkTerminalCommand::ReadText {
            surface_id: surface_id.to_string(),
            capture,
            max_bytes,
            reply,
        })?;
        // Blocks until the GTK main loop services the command. The socket
        // server calls this from its multi-threaded tokio runtime, where a bare
        // blocking wait pins a worker thread — N concurrent read_text calls
        // (agent hooks issue them constantly) would starve every other socket
        // task, including accept(). Offload the wait via block_in_place so tokio
        // can keep those workers busy. Off a multi-thread runtime (GTK thread or
        // a current-thread runtime) there is no worker pool to protect and
        // block_in_place would panic, so wait directly.
        let wait = move || -> Result<TerminalTextSnapshot, TerminalError> {
            receiver
                .recv_timeout(Duration::from_secs(2))
                .map_err(|err| TerminalError::Backend(format!("read-text reply failed: {err}")))?
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(wait)
            }
            _ => wait(),
        }
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        let previous_size = (surface.cols, surface.rows);
        surface.cols = cols;
        surface.rows = rows;
        drop(surfaces);
        if let Err(err) = self.send_command(GtkTerminalCommand::Resize {
            surface_id: surface_id.to_string(),
            cols,
            rows,
        }) {
            if let Ok(mut surfaces) = self.surfaces.lock() {
                if let Some(surface) = surfaces.get_mut(surface_id) {
                    surface.cols = previous_size.0;
                    surface.rows = previous_size.1;
                }
            }
            return Err(err);
        }
        Ok(())
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let removed = surfaces
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        drop(surfaces);
        let was_ready = self
            .ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id);
        if let Err(err) = self.send_command(GtkTerminalCommand::Close {
            surface_id: surface_id.to_string(),
        }) {
            if let Ok(mut surfaces) = self.surfaces.lock() {
                surfaces.insert(surface_id.to_string(), removed);
            }
            if was_ready {
                if let Ok(mut ready_surfaces) = self.ready_surfaces.lock() {
                    ready_surfaces.insert(surface_id.to_string());
                }
            }
            return Err(err);
        }
        Ok(())
    }

    fn mark_surface_ready(&self, surface_id: &str) -> Result<(), TerminalError> {
        if !self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains_key(surface_id)
        {
            return Err(TerminalError::NotFound(surface_id.to_string()));
        }
        self.ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .insert(surface_id.to_string());
        Ok(())
    }

    fn mark_surface_not_ready(&self, surface_id: &str) -> Result<(), TerminalError> {
        if !self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains_key(surface_id)
        {
            return Err(TerminalError::NotFound(surface_id.to_string()));
        }
        self.ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id);
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
        surface.pid = Some(pid);
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
        surface.pid = None;
        Ok(())
    }

    fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        self.ready_surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id);
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(surfaces.values().cloned().collect())
    }
}
