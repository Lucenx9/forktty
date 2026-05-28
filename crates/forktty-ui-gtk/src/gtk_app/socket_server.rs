use super::*;

pub(super) fn start_socket_server(state: SocketAppState) {
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
            return;
        }
    };

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
        if let Err(err) = runtime.block_on(serve(listener, state)) {
            eprintln!("ForkTTY socket server stopped: {err}");
            create_global_notification(
                &state_for_error,
                "Automation Stopped",
                &format!("The ForkTTY socket server stopped. {err}"),
                NotificationKind::Error,
            );
        }
    });
}
