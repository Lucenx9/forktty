use super::*;

pub(super) struct SocketServerHandle {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl SocketServerHandle {
    fn new(shutdown: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            shutdown: Some(shutdown),
        }
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for SocketServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
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
    std::thread::spawn(move || {
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
    Some(SocketServerHandle::new(shutdown_tx))
}
