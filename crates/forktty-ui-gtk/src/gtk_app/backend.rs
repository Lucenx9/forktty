use super::*;

pub(super) enum GtkTerminalCommand {
    Spawn {
        request: SpawnRequest,
        generation: u64,
    },
    ShowSurface {
        surface_id: String,
        generation: u64,
    },
    SendText {
        surface_id: String,
        generation: u64,
        text: String,
        execution: Arc<CommandExecution>,
        reply: mpsc::Sender<Result<(), TerminalError>>,
    },
    ReadText {
        surface_id: String,
        generation: u64,
        capture: TerminalTextCapture,
        max_bytes: usize,
        execution: Arc<CommandExecution>,
        reply: mpsc::Sender<Result<TerminalTextSnapshot, TerminalError>>,
    },
    Resize {
        surface_id: String,
        generation: u64,
        cols: u16,
        rows: u16,
    },
    Close {
        surface_id: String,
        generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum CommandExecutionState {
    Pending = 0,
    Claimed = 1,
    Cancelled = 2,
}

pub(super) struct CommandExecution {
    state: std::sync::atomic::AtomicU8,
    runtime: Arc<Mutex<GtkTerminalRuntime>>,
    generation_leases_released: Arc<std::sync::Condvar>,
}

impl CommandExecution {
    fn new(
        runtime: Arc<Mutex<GtkTerminalRuntime>>,
        generation_leases_released: Arc<std::sync::Condvar>,
    ) -> Self {
        Self {
            state: std::sync::atomic::AtomicU8::new(CommandExecutionState::Pending as u8),
            runtime,
            generation_leases_released,
        }
    }

    pub(super) fn claim(&self) -> bool {
        self.state
            .compare_exchange(
                CommandExecutionState::Pending as u8,
                CommandExecutionState::Claimed as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn cancel_pending(&self) -> bool {
        self.state
            .compare_exchange(
                CommandExecutionState::Pending as u8,
                CommandExecutionState::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn run_if_current_generation<T>(
        &self,
        surface_id: &str,
        generation: u64,
        operation: impl FnOnce() -> Result<T, TerminalError>,
    ) -> Result<T, TerminalError> {
        let _lease = acquire_surface_generation_lease(
            &self.runtime,
            &self.generation_leases_released,
            LeasedSurfaceGenerations::single(surface_id, generation),
            true,
        )?;
        operation()
    }
}

#[derive(Debug, Clone)]
struct GtkTerminalRuntimeEntry {
    generation: u64,
    state: TerminalSurfaceState,
    ready: bool,
    active_generation_leases: usize,
}

struct SurfaceGenerationLease {
    runtime: Arc<Mutex<GtkTerminalRuntime>>,
    generation_leases_released: Arc<std::sync::Condvar>,
    generations: LeasedSurfaceGenerations,
}

enum LeasedSurfaceGenerations {
    Single((String, u64)),
    Multiple(Vec<(String, u64)>),
}

impl LeasedSurfaceGenerations {
    fn single(surface_id: &str, generation: u64) -> Self {
        Self::Single((surface_id.to_string(), generation))
    }

    fn as_slice(&self) -> &[(String, u64)] {
        match self {
            Self::Single(generation) => std::slice::from_ref(generation),
            Self::Multiple(generations) => generations,
        }
    }
}

impl Drop for SurfaceGenerationLease {
    fn drop(&mut self) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (surface_id, generation) in self.generations.as_slice() {
            let Some(entry) = runtime.surfaces.get_mut(surface_id) else {
                continue;
            };
            if entry.generation == *generation && entry.active_generation_leases > 0 {
                entry.active_generation_leases -= 1;
            }
        }
        self.generation_leases_released.notify_all();
    }
}

fn acquire_surface_generation_lease(
    runtime: &Arc<Mutex<GtkTerminalRuntime>>,
    generation_leases_released: &Arc<std::sync::Condvar>,
    generations: LeasedSurfaceGenerations,
    require_ready: bool,
) -> Result<SurfaceGenerationLease, TerminalError> {
    let mut runtime_guard = runtime.lock().map_err(|_| TerminalError::LockPoisoned)?;
    if let Some((surface_id, _)) = generations
        .as_slice()
        .iter()
        .find(|(surface_id, generation)| {
            !runtime_guard.surfaces.get(surface_id).is_some_and(|entry| {
                entry.generation == *generation && (!require_ready || entry.ready)
            })
        })
    {
        return Err(TerminalError::NotReady(surface_id.clone()));
    }
    for (surface_id, _) in generations.as_slice() {
        let entry = runtime_guard
            .surfaces
            .get_mut(surface_id)
            .expect("surface generation was validated before lease acquisition");
        entry.active_generation_leases += 1;
    }
    drop(runtime_guard);
    Ok(SurfaceGenerationLease {
        runtime: Arc::clone(runtime),
        generation_leases_released: Arc::clone(generation_leases_released),
        generations,
    })
}

#[derive(Debug)]
struct GtkTerminalRuntime {
    next_generation: u64,
    surfaces: BTreeMap<String, GtkTerminalRuntimeEntry>,
}

impl Default for GtkTerminalRuntime {
    fn default() -> Self {
        Self {
            next_generation: 1,
            surfaces: BTreeMap::new(),
        }
    }
}

impl GtkTerminalRuntime {
    fn allocate_generation(&mut self) -> Result<u64, TerminalError> {
        let generation = self.next_generation;
        if generation == 0 {
            return Err(TerminalError::Backend(
                "terminal surface generation space exhausted".to_string(),
            ));
        }
        self.next_generation = generation.checked_add(1).unwrap_or(0);
        Ok(generation)
    }
}

#[cfg(test)]
type OneShotCallback = Box<dyn FnOnce() + Send>;

#[cfg(test)]
#[derive(Clone, Default)]
struct OneShotHook {
    callback: Arc<Mutex<Option<OneShotCallback>>>,
}

#[cfg(test)]
impl std::fmt::Debug for OneShotHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OneShotHook")
    }
}

#[cfg(test)]
impl OneShotHook {
    fn set(&self, callback: impl FnOnce() + Send + 'static) {
        *self.callback.lock().unwrap() = Some(Box::new(callback));
    }

    fn run(&self) {
        let callback = self.callback.lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct GtkTerminalBackend {
    sender: mpsc::Sender<GtkTerminalCommand>,
    runtime: Arc<Mutex<GtkTerminalRuntime>>,
    generation_leases_released: Arc<std::sync::Condvar>,
    command_reply_timeout: Duration,
    #[cfg(test)]
    after_enqueue_hook: OneShotHook,
    #[cfg(test)]
    after_reply_timeout_hook: OneShotHook,
    #[cfg(test)]
    after_generation_lease_hook: OneShotHook,
}

impl GtkTerminalBackend {
    pub(super) fn new(sender: mpsc::Sender<GtkTerminalCommand>) -> Self {
        Self {
            sender,
            runtime: Arc::new(Mutex::new(GtkTerminalRuntime::default())),
            generation_leases_released: Arc::new(std::sync::Condvar::new()),
            command_reply_timeout: Duration::from_secs(2),
            #[cfg(test)]
            after_enqueue_hook: OneShotHook::default(),
            #[cfg(test)]
            after_reply_timeout_hook: OneShotHook::default(),
            #[cfg(test)]
            after_generation_lease_hook: OneShotHook::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn new_with_command_timeout_for_test(
        sender: mpsc::Sender<GtkTerminalCommand>,
        command_reply_timeout: Duration,
    ) -> Self {
        let mut backend = Self::new(sender);
        backend.command_reply_timeout = command_reply_timeout;
        backend
    }

    fn send_command(&self, command: GtkTerminalCommand) -> Result<(), TerminalError> {
        self.sender
            .send(command)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn mark_surface_ready_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
    ) -> Result<(), TerminalError> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if entry.generation != generation {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        entry.ready = true;
        Ok(())
    }

    pub(super) fn surface_generation_is_current(
        &self,
        surface_id: &str,
        generation: u64,
    ) -> Result<bool, TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(runtime
            .surfaces
            .get(surface_id)
            .is_some_and(|entry| entry.generation == generation))
    }

    /// Run a callback while the named surface incarnation owns a lease. The
    /// runtime mutex is released before the callback, while close/replacement
    /// waits for the lease; callbacks may therefore take the model lock without
    /// inverting the model/backend lock order. A callback must not synchronously
    /// close or forget its own leased surface, because that operation waits for
    /// the callback's lease to be released.
    pub(super) fn with_surface_generation<T>(
        &self,
        surface_id: &str,
        generation: u64,
        callback: impl FnOnce() -> T,
    ) -> Result<T, TerminalError> {
        let _lease = acquire_surface_generation_lease(
            &self.runtime,
            &self.generation_leases_released,
            LeasedSurfaceGenerations::single(surface_id, generation),
            false,
        )?;
        #[cfg(test)]
        self.run_after_generation_lease_hook_for_test();
        Ok(callback())
    }

    pub(super) fn with_surface_generations<T>(
        &self,
        generations: Vec<(String, u64)>,
        callback: impl FnOnce() -> T,
    ) -> Result<T, TerminalError> {
        let _lease = acquire_surface_generation_lease(
            &self.runtime,
            &self.generation_leases_released,
            LeasedSurfaceGenerations::Multiple(generations),
            false,
        )?;
        Ok(callback())
    }

    pub(super) fn surface_size_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
    ) -> Result<(u16, u16), TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if entry.generation != generation {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        Ok((entry.state.cols, entry.state.rows))
    }

    pub(super) fn mark_surface_not_ready_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
    ) -> Result<(), TerminalError> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if entry.generation != generation {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        entry.ready = false;
        Ok(())
    }

    pub(super) fn mark_surface_pid_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
        pid: u32,
    ) -> Result<(), TerminalError> {
        self.mark_surface_pid_for_generation_and(surface_id, generation, pid, || ())
    }

    pub(super) fn mark_surface_pid_for_generation_and<T>(
        &self,
        surface_id: &str,
        generation: u64,
        pid: u32,
        commit_observation: impl FnOnce() -> T,
    ) -> Result<T, TerminalError> {
        let _lease = acquire_surface_generation_lease(
            &self.runtime,
            &self.generation_leases_released,
            LeasedSurfaceGenerations::single(surface_id, generation),
            false,
        )?;
        {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            let entry = runtime
                .surfaces
                .get_mut(surface_id)
                .expect("generation lease keeps the runtime entry alive");
            entry.state.pid = Some(pid);
        }
        #[cfg(test)]
        self.run_after_generation_lease_hook_for_test();
        Ok(commit_observation())
    }

    pub(super) fn clear_surface_pid_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
    ) -> Result<(), TerminalError> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if entry.generation != generation {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        entry.state.pid = None;
        Ok(())
    }

    pub(super) fn mark_surface_exited_for_generation_and<T>(
        &self,
        surface_id: &str,
        generation: u64,
        commit_exit: impl FnOnce() -> T,
    ) -> Result<T, TerminalError> {
        let _lease = acquire_surface_generation_lease(
            &self.runtime,
            &self.generation_leases_released,
            LeasedSurfaceGenerations::single(surface_id, generation),
            false,
        )?;
        {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            let entry = runtime
                .surfaces
                .get_mut(surface_id)
                .expect("generation lease keeps the runtime entry alive");
            entry.ready = false;
            entry.state.pid = None;
        }
        #[cfg(test)]
        self.run_after_generation_lease_hook_for_test();
        Ok(commit_exit())
    }

    pub(super) fn resize_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
        cols: u16,
        rows: u16,
    ) -> Result<(), TerminalError> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if entry.generation != generation {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        let previous_size = (entry.state.cols, entry.state.rows);
        entry.state.cols = cols;
        entry.state.rows = rows;
        if let Err(err) = self.send_command(GtkTerminalCommand::Resize {
            surface_id: surface_id.to_string(),
            generation,
            cols,
            rows,
        }) {
            if let Some(entry) = runtime.surfaces.get_mut(surface_id) {
                entry.state.cols = previous_size.0;
                entry.state.rows = previous_size.1;
            }
            return Err(err);
        }
        #[cfg(test)]
        self.run_after_enqueue_hook_for_test();
        Ok(())
    }

    pub(super) fn close_for_generation(
        &self,
        surface_id: &str,
        generation: u64,
    ) -> Result<(), TerminalError> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        loop {
            let entry = runtime
                .surfaces
                .get(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            if entry.generation != generation {
                return Err(TerminalError::NotReady(surface_id.to_string()));
            }
            if entry.active_generation_leases == 0 {
                break;
            }
            runtime = self
                .generation_leases_released
                .wait(runtime)
                .map_err(|_| TerminalError::LockPoisoned)?;
        }
        let removed = runtime
            .surfaces
            .remove(surface_id)
            .expect("generation lease wait kept this terminal runtime entry locked");
        if let Err(err) = self.send_command(GtkTerminalCommand::Close {
            surface_id: surface_id.to_string(),
            generation,
        }) {
            runtime.surfaces.insert(surface_id.to_string(), removed);
            return Err(err);
        }
        #[cfg(test)]
        self.run_after_enqueue_hook_for_test();
        Ok(())
    }

    fn current_surface_generation(&self, surface_id: &str) -> Result<u64, TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        runtime
            .surfaces
            .get(surface_id)
            .map(|entry| entry.generation)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))
    }

    #[cfg(test)]
    pub(super) fn runtime_entry_for_test(
        &self,
        surface_id: &str,
    ) -> Result<(u64, TerminalSurfaceState, bool), TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok((entry.generation, entry.state.clone(), entry.ready))
    }

    #[cfg(test)]
    pub(super) fn runtime_is_locked_for_test(&self) -> bool {
        matches!(
            self.runtime.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }

    #[cfg(test)]
    pub(super) fn set_after_enqueue_hook_for_test(&self, callback: impl FnOnce() + Send + 'static) {
        self.after_enqueue_hook.set(callback);
    }

    #[cfg(test)]
    fn run_after_enqueue_hook_for_test(&self) {
        self.after_enqueue_hook.run();
    }

    #[cfg(test)]
    pub(super) fn set_after_reply_timeout_hook_for_test(
        &self,
        callback: impl FnOnce() + Send + 'static,
    ) {
        self.after_reply_timeout_hook.set(callback);
    }

    #[cfg(test)]
    pub(super) fn set_after_generation_lease_hook_for_test(
        &self,
        callback: impl FnOnce() + Send + 'static,
    ) {
        self.after_generation_lease_hook.set(callback);
    }

    #[cfg(test)]
    fn run_after_generation_lease_hook_for_test(&self) {
        self.after_generation_lease_hook.run();
    }

    #[cfg(test)]
    fn run_after_reply_timeout_hook_for_test(&self) {
        self.after_reply_timeout_hook.run();
    }
}

impl GtkTerminalBackend {
    fn wait_for_gtk_command_reply<T>(
        &self,
        receiver: mpsc::Receiver<Result<T, TerminalError>>,
        execution: &CommandExecution,
        operation: &str,
    ) -> Result<T, TerminalError> {
        // Socket calls run on a multi-threaded tokio runtime. Offload the blocking
        // GTK reply wait so concurrent hook/status reads cannot pin every worker.
        let wait = || -> Result<T, TerminalError> {
            match receiver.recv_timeout(self.command_reply_timeout) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    #[cfg(test)]
                    self.run_after_reply_timeout_hook_for_test();
                    if execution.cancel_pending() {
                        Err(TerminalError::Backend(format!(
                            "{operation} reply failed: timed out waiting on channel"
                        )))
                    } else {
                        receiver.recv().map_err(|err| {
                            TerminalError::Backend(format!(
                                "{operation} reply failed after GTK claimed execution: {err}"
                            ))
                        })?
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    execution.cancel_pending();
                    Err(TerminalError::Backend(format!(
                        "{operation} reply failed: channel is empty and sending half is closed"
                    )))
                }
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(wait)
            }
            _ => wait(),
        }
    }
}

impl TerminalBackend for GtkTerminalBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let surface_id = request.surface_id.clone();
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        if runtime.surfaces.contains_key(&surface_id) {
            return Err(TerminalError::Backend(format!(
                "surface already exists: {surface_id}"
            )));
        }
        let generation = runtime.allocate_generation()?;
        runtime.surfaces.insert(
            request.surface_id.clone(),
            GtkTerminalRuntimeEntry {
                generation,
                state: TerminalSurfaceState {
                    surface_id: request.surface_id.clone(),
                    workspace_id: request.workspace_id.clone(),
                    cwd: request.cwd.clone(),
                    shell: request.shell.clone(),
                    cols: 80,
                    rows: 24,
                    pid: None,
                },
                ready: false,
                active_generation_leases: 0,
            },
        );
        if let Err(err) = self.send_command(GtkTerminalCommand::Spawn {
            request,
            generation,
        }) {
            runtime.surfaces.remove(&surface_id);
            return Err(err);
        }
        #[cfg(test)]
        self.run_after_enqueue_hook_for_test();
        Ok(())
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if !entry.ready {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        let generation = entry.generation;
        drop(runtime);
        let (reply, receiver) = mpsc::channel();
        let execution = Arc::new(CommandExecution::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.generation_leases_released),
        ));
        self.send_command(GtkTerminalCommand::SendText {
            surface_id: surface_id.to_string(),
            generation,
            text: text.to_string(),
            execution: Arc::clone(&execution),
            reply,
        })?;
        self.wait_for_gtk_command_reply(receiver, &execution, "send-text")
    }

    fn show_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let generation = runtime
            .surfaces
            .get(surface_id)
            .map(|entry| entry.generation)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        self.send_command(GtkTerminalCommand::ShowSurface {
            surface_id: surface_id.to_string(),
            generation,
        })
    }

    fn read_text(
        &self,
        surface_id: &str,
        capture: TerminalTextCapture,
        max_bytes: usize,
    ) -> Result<TerminalTextSnapshot, TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let entry = runtime
            .surfaces
            .get(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        if !entry.ready {
            return Err(TerminalError::NotReady(surface_id.to_string()));
        }
        let generation = entry.generation;
        drop(runtime);
        let (reply, receiver) = mpsc::channel();
        let execution = Arc::new(CommandExecution::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.generation_leases_released),
        ));
        self.send_command(GtkTerminalCommand::ReadText {
            surface_id: surface_id.to_string(),
            generation,
            capture,
            max_bytes,
            execution: Arc::clone(&execution),
            reply,
        })?;
        self.wait_for_gtk_command_reply(receiver, &execution, "read-text")
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let generation = self.current_surface_generation(surface_id)?;
        self.resize_for_generation(surface_id, generation, cols, rows)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let generation = self.current_surface_generation(surface_id)?;
        self.close_for_generation(surface_id, generation)
    }

    fn mark_surface_ready(&self, surface_id: &str) -> Result<(), TerminalError> {
        let generation = {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            runtime
                .surfaces
                .get(surface_id)
                .map(|entry| entry.generation)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?
        };
        self.mark_surface_ready_for_generation(surface_id, generation)
    }

    fn mark_surface_not_ready(&self, surface_id: &str) -> Result<(), TerminalError> {
        let generation = self.current_surface_generation(surface_id)?;
        self.mark_surface_not_ready_for_generation(surface_id, generation)
    }

    fn mark_surface_pid(&self, surface_id: &str, pid: u32) -> Result<(), TerminalError> {
        let generation = self.current_surface_generation(surface_id)?;
        self.mark_surface_pid_for_generation(surface_id, generation, pid)
    }

    fn clear_surface_pid(&self, surface_id: &str) -> Result<(), TerminalError> {
        let generation = self.current_surface_generation(surface_id)?;
        self.clear_surface_pid_for_generation(surface_id, generation)
    }

    fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        loop {
            let entry = runtime
                .surfaces
                .get(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            if entry.active_generation_leases == 0 {
                break;
            }
            runtime = self
                .generation_leases_released
                .wait(runtime)
                .map_err(|_| TerminalError::LockPoisoned)?;
        }
        runtime
            .surfaces
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(())
    }

    fn surface_ready(&self, surface_id: &str) -> Result<bool, TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(runtime
            .surfaces
            .get(surface_id)
            .is_some_and(|entry| entry.ready))
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(runtime
            .surfaces
            .values()
            .map(|entry| entry.state.clone())
            .collect())
    }
}
