use super::*;

pub(super) struct SocketServerHandle {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    completion: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl SocketServerHandle {
    pub(super) fn new(
        shutdown: tokio::sync::oneshot::Sender<()>,
        completion: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        Self {
            shutdown: Some(shutdown),
            completion: Some(completion),
        }
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }

    /// Request cooperative shutdown and wait until the socket runtime has
    /// drained every admitted dispatch and been dropped on its owning thread.
    pub(super) async fn shutdown_and_wait(mut self) {
        self.shutdown();
        if let Some(completion) = self.completion.take() {
            let _ = completion.await;
        }
    }
}

impl Drop for SocketServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct SocketServerCompletion(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for SocketServerCompletion {
    fn drop(&mut self) {
        if let Some(completion) = self.0.take() {
            let _ = completion.send(());
        }
    }
}

pub(super) fn run_socket_server_thread(
    completion: tokio::sync::oneshot::Sender<()>,
    run: impl FnOnce(),
) {
    // The callback owns the Tokio runtime. Its locals are dropped before it
    // returns (or while it unwinds), and only then does this outer guard make
    // asynchronous shutdown completion observable to the GTK main context.
    let _completion = SocketServerCompletion(Some(completion));
    run();
}

pub(super) fn start_socket_server(state: SocketAppState) -> Option<SocketServerHandle> {
    let listener = match bind_socket_listener(&state.socket_path, true) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!(
                "Failed to bind ForkTTY socket {}: {err}",
                state.socket_path.display()
            );
            create_global_notification(
                &state,
                "Automation Unavailable",
                &format!(
                    "Could not bind the ForkTTY socket at {}. Socket automation is disabled. {err}",
                    state.socket_path.display()
                ),
                NotificationKind::Error,
            );
            return None;
        }
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        run_socket_server_thread(completion_tx, move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    eprintln!("Failed to start ForkTTY socket runtime: {err}");
                    create_global_notification(
                        &state,
                        "Automation Unavailable",
                        &format!("Could not start the socket runtime. {err}"),
                        NotificationKind::Error,
                    );
                    return;
                }
            };
            let state_for_error = state.clone();
            let shutdown = async {
                let _ = shutdown_rx.await;
            };
            if let Err(err) = runtime.block_on(serve_until_shutdown(listener, state, shutdown)) {
                eprintln!("ForkTTY socket server stopped: {err}");
                create_global_notification(
                    &state_for_error,
                    "Automation Stopped",
                    &format!("The ForkTTY socket server stopped. {err}"),
                    NotificationKind::Error,
                );
            }
        });
    });
    Some(SocketServerHandle::new(shutdown_tx, completion_rx))
}
