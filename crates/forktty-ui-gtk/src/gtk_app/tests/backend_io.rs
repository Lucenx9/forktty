//! GTK terminal backend enqueue, readiness, timeout, and rollback regressions.

use super::*;

#[test]
fn gtk_backend_holds_runtime_lock_through_command_enqueue() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected spawn command"),
    };

    let (enqueued_tx, enqueued_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    backend.set_after_enqueue_hook_for_test(move || {
        enqueued_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });

    let resize_backend = backend.clone();
    let resize = std::thread::spawn(move || resize_backend.resize("surface-1", 120, 40));
    enqueued_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("resize should enqueue while holding the runtime lock");
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        GtkTerminalCommand::Resize {
            surface_id,
            generation: command_generation,
            cols: 120,
            rows: 40,
        } if surface_id == "surface-1" && command_generation == generation
    ));
    assert!(
        backend.runtime_is_locked_for_test(),
        "runtime readers must not observe mutation before its command is durably enqueued"
    );

    release_tx.send(()).unwrap();
    resize.join().unwrap().unwrap();
    let surfaces = backend.surfaces().unwrap();
    assert_eq!((surfaces[0].cols, surfaces[0].rows), (120, 40));
}

#[test]
fn gtk_backend_restores_existing_surface_when_duplicate_spawn_send_fails() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    backend.mark_surface_ready("surface-1").unwrap();
    drop(rx);

    let mut duplicate = test_spawn_request();
    duplicate.shell = "/bin/zsh".to_string();
    duplicate.cwd = PathBuf::from("/tmp/changed");

    let err = backend.spawn(duplicate).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    let mut surfaces = backend.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    let surface = surfaces.remove(0);
    assert_eq!(surface.surface_id, "surface-1");
    assert_eq!(surface.shell, "/bin/sh");
    assert_eq!(surface.cwd, PathBuf::from("/tmp"));
    let err = backend
        .send_text("surface-1", "echo still-ready\n")
        .unwrap_err();
    assert!(matches!(err, TerminalError::Backend(_)));
}

#[test]
fn gtk_backend_rolls_back_resize_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    drop(rx);

    let err = backend.resize("surface-1", 120, 40).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    let mut surfaces = backend.surfaces().unwrap();
    let surface = surfaces.remove(0);
    assert_eq!((surface.cols, surface.rows), (80, 24));
}

#[test]
fn embedded_runtime_size_sync_updates_backend_metadata() {
    let backend = forktty_terminal::HeadlessTerminalBackend::new();
    backend.spawn(test_spawn_request()).unwrap();
    let snapshot =
        TerminalTextSnapshot::from_text("surface-1", "", 166, 42, TerminalTextCapture::Visible, 0);

    let changed =
        sync_terminal_surface_size_from_snapshot(&backend, "surface-1", &snapshot).unwrap();

    assert!(changed);
    let mut surfaces = backend.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    let surface = surfaces.remove(0);
    assert_eq!((surface.cols, surface.rows), (166, 42));
}

#[test]
fn stale_embedded_size_callback_cannot_resize_replacement_runtime() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    let stale_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected first spawn command"),
    };
    backend.close("surface-1").unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
    backend.spawn(test_spawn_request()).unwrap();
    let current_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected replacement spawn command"),
    };
    let snapshot =
        TerminalTextSnapshot::from_text("surface-1", "", 166, 42, TerminalTextCapture::Visible, 0);

    assert!(matches!(
        sync_terminal_surface_size_from_snapshot_for_generation(
            &backend,
            "surface-1",
            stale_generation,
            &snapshot,
        ),
        Err(TerminalError::NotReady(surface_id)) if surface_id == "surface-1"
    ));
    assert_eq!(
        backend
            .surface_size_for_generation("surface-1", current_generation)
            .unwrap(),
        (80, 24)
    );
    assert!(
        rx.try_recv().is_err(),
        "stale size callback must not enqueue resize"
    );

    assert!(sync_terminal_surface_size_from_snapshot_for_generation(
        &backend,
        "surface-1",
        current_generation,
        &snapshot,
    )
    .unwrap());
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Resize {
            generation,
            cols: 166,
            rows: 42,
            ..
        } if generation == current_generation
    ));
}

#[test]
fn gtk_backend_rolls_back_close_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    drop(rx);

    let err = backend.close("surface-1").unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    let surfaces = backend.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].pid, None);
}

#[test]
fn gtk_terminal_backend_blocks_send_until_ready() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));

    assert!(matches!(
        backend.send_text("surface-1", "echo before-ready\n"),
        Err(TerminalError::NotReady(surface)) if surface == "surface-1"
    ));

    backend.mark_surface_ready("surface-1").unwrap();
    drive_backend_send_text(&backend, &rx, "surface-1", "echo ready\n", Ok(())).unwrap();
}

#[test]
fn gtk_terminal_backend_propagates_controller_send_text_failure() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    backend.mark_surface_ready("surface-1").unwrap();

    let err = drive_backend_send_text(
        &backend,
        &rx,
        "surface-1",
        "echo rejected\n",
        Err(TerminalError::Backend(
            "ghostty rejected send-text".to_string(),
        )),
    )
    .unwrap_err();

    assert!(err.contains("ghostty rejected send-text"));
}

#[test]
fn gtk_terminal_backend_pending_send_timeout_cancels_before_widget_access() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new_with_command_timeout_for_test(tx, Duration::ZERO);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    backend.mark_surface_ready("surface-1").unwrap();

    let (timeout_started_tx, timeout_started_rx) = mpsc::channel();
    let (release_timeout_tx, release_timeout_rx) = mpsc::channel();
    backend.set_after_reply_timeout_hook_for_test(move || {
        timeout_started_tx.send(()).unwrap();
        release_timeout_rx.recv().unwrap();
    });

    let send_backend = backend.clone();
    let (send_result_tx, send_result_rx) = mpsc::channel();
    let send = std::thread::spawn(move || {
        send_result_tx
            .send(send_backend.send_text("surface-1", "echo late\n"))
            .unwrap();
    });
    let (surface_id, generation, execution, _reply) =
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            GtkTerminalCommand::SendText {
                surface_id,
                generation,
                execution,
                reply,
                ..
            } => (surface_id, generation, execution, reply),
            _ => panic!("expected send-text command"),
        };
    timeout_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    release_timeout_tx.send(()).unwrap();

    let error = send_result_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap_err();
    send.join().unwrap();
    assert!(error.to_string().contains("send-text reply failed"));

    let widget_accessed = std::sync::atomic::AtomicBool::new(false);
    let result = execute_terminal_command(&execution, &surface_id, generation, || {
        widget_accessed.store(true, Ordering::SeqCst);
        Ok(())
    });
    assert!(
        result.is_none(),
        "cancelled command must not execute on GTK"
    );
    assert!(!widget_accessed.load(Ordering::SeqCst));
}

#[test]
fn gtk_terminal_backend_claimed_send_returns_actual_result_after_deadline() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new_with_command_timeout_for_test(tx, Duration::ZERO);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    backend.mark_surface_ready("surface-1").unwrap();

    let (timeout_started_tx, timeout_started_rx) = mpsc::channel();
    let (release_timeout_tx, release_timeout_rx) = mpsc::channel();
    backend.set_after_reply_timeout_hook_for_test(move || {
        timeout_started_tx.send(()).unwrap();
        release_timeout_rx.recv().unwrap();
    });

    let send_backend = backend.clone();
    let (send_result_tx, send_result_rx) = mpsc::channel();
    let send = std::thread::spawn(move || {
        send_result_tx
            .send(send_backend.send_text("surface-1", "echo claimed\n"))
            .unwrap();
    });
    let (execution, reply) = match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
        GtkTerminalCommand::SendText {
            execution, reply, ..
        } => (execution, reply),
        _ => panic!("expected send-text command"),
    };
    timeout_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(execution.claim());
    release_timeout_tx.send(()).unwrap();
    reply
        .send(Err(TerminalError::Backend(
            "claimed command result".to_string(),
        )))
        .unwrap();

    let error = send_result_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap_err();
    send.join().unwrap();
    assert_eq!(
        error.to_string(),
        "Terminal backend error: claimed command result"
    );
}

#[test]
fn gtk_terminal_backend_pending_read_timeout_cancels_before_widget_access() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new_with_command_timeout_for_test(tx, Duration::ZERO);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    backend.mark_surface_ready("surface-1").unwrap();

    let (timeout_started_tx, timeout_started_rx) = mpsc::channel();
    let (release_timeout_tx, release_timeout_rx) = mpsc::channel();
    backend.set_after_reply_timeout_hook_for_test(move || {
        timeout_started_tx.send(()).unwrap();
        release_timeout_rx.recv().unwrap();
    });

    let read_backend = backend.clone();
    let (read_result_tx, read_result_rx) = mpsc::channel();
    let read = std::thread::spawn(move || {
        read_result_tx
            .send(read_backend.read_text("surface-1", TerminalTextCapture::Visible, 4096))
            .unwrap();
    });
    let (surface_id, generation, execution, _reply) =
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            GtkTerminalCommand::ReadText {
                surface_id,
                generation,
                execution,
                reply,
                ..
            } => (surface_id, generation, execution, reply),
            _ => panic!("expected read-text command"),
        };
    timeout_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    release_timeout_tx.send(()).unwrap();

    let error = read_result_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap_err();
    read.join().unwrap();
    assert!(error.to_string().contains("read-text reply failed"));

    let widget_accessed = std::sync::atomic::AtomicBool::new(false);
    let result = execute_terminal_command(&execution, &surface_id, generation, || {
        widget_accessed.store(true, Ordering::SeqCst);
        Ok(TerminalTextSnapshot::from_text(
            &surface_id,
            "late",
            80,
            24,
            TerminalTextCapture::Visible,
            4096,
        ))
    });
    assert!(result.is_none(), "cancelled read must not execute on GTK");
    assert!(!widget_accessed.load(Ordering::SeqCst));
}

#[test]
fn gtk_terminal_backend_stale_send_does_not_access_replacement_widget() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new_with_command_timeout_for_test(tx, Duration::from_secs(5));
    backend.spawn(test_spawn_request()).unwrap();
    let stale_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected first spawn command"),
    };
    backend
        .mark_surface_ready_for_generation("surface-1", stale_generation)
        .unwrap();

    let send_backend = backend.clone();
    let (send_result_tx, send_result_rx) = mpsc::channel();
    let send = std::thread::spawn(move || {
        send_result_tx
            .send(send_backend.send_text("surface-1", "echo stale\n"))
            .unwrap();
    });
    let (surface_id, generation, execution, reply) =
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            GtkTerminalCommand::SendText {
                surface_id,
                generation,
                execution,
                reply,
                ..
            } => (surface_id, generation, execution, reply),
            _ => panic!("expected send-text command"),
        };
    assert_eq!(generation, stale_generation);

    backend.close("surface-1").unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
    backend.spawn(test_spawn_request()).unwrap();
    let replacement_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected replacement spawn command"),
    };
    backend
        .mark_surface_ready_for_generation("surface-1", replacement_generation)
        .unwrap();

    let replacement_widget_accessed = std::sync::atomic::AtomicBool::new(false);
    let result = execute_terminal_command(&execution, &surface_id, generation, || {
        replacement_widget_accessed.store(true, Ordering::SeqCst);
        Ok(())
    })
    .expect("non-cancelled GTK command should return a result");
    assert!(matches!(
        result,
        Err(TerminalError::NotReady(ref id)) if id == "surface-1"
    ));
    assert!(!replacement_widget_accessed.load(Ordering::SeqCst));
    reply.send(result).unwrap();

    assert!(matches!(
        send_result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(TerminalError::NotReady(id)) if id == "surface-1"
    ));
    send.join().unwrap();
}

#[test]
fn gtk_terminal_backend_stale_read_does_not_access_replacement_widget() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new_with_command_timeout_for_test(tx, Duration::from_secs(5));
    backend.spawn(test_spawn_request()).unwrap();
    let stale_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected first spawn command"),
    };
    backend
        .mark_surface_ready_for_generation("surface-1", stale_generation)
        .unwrap();

    let read_backend = backend.clone();
    let (read_result_tx, read_result_rx) = mpsc::channel();
    let read = std::thread::spawn(move || {
        read_result_tx
            .send(read_backend.read_text("surface-1", TerminalTextCapture::Visible, 4096))
            .unwrap();
    });
    let (surface_id, generation, execution, reply) =
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            GtkTerminalCommand::ReadText {
                surface_id,
                generation,
                execution,
                reply,
                ..
            } => (surface_id, generation, execution, reply),
            _ => panic!("expected read-text command"),
        };
    assert_eq!(generation, stale_generation);

    backend.close("surface-1").unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
    backend.spawn(test_spawn_request()).unwrap();
    let replacement_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected replacement spawn command"),
    };
    backend
        .mark_surface_ready_for_generation("surface-1", replacement_generation)
        .unwrap();

    let replacement_widget_accessed = std::sync::atomic::AtomicBool::new(false);
    let result = execute_terminal_command(&execution, &surface_id, generation, || {
        replacement_widget_accessed.store(true, Ordering::SeqCst);
        Ok(TerminalTextSnapshot::from_text(
            &surface_id,
            "replacement",
            80,
            24,
            TerminalTextCapture::Visible,
            4096,
        ))
    })
    .expect("non-cancelled GTK command should return a result");
    assert!(matches!(
        result,
        Err(TerminalError::NotReady(ref id)) if id == "surface-1"
    ));
    assert!(!replacement_widget_accessed.load(Ordering::SeqCst));
    reply.send(result).unwrap();

    assert!(matches!(
        read_result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(TerminalError::NotReady(id)) if id == "surface-1"
    ));
    read.join().unwrap();
}

#[test]
fn gtk_terminal_backend_rejects_duplicate_spawn_without_clearing_ready() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    backend.mark_surface_ready("surface-1").unwrap();

    let err = backend.spawn(test_spawn_request()).unwrap_err();

    assert!(err.to_string().contains("surface already exists"));
    drive_backend_send_text(&backend, &rx, "surface-1", "echo still-ready\n", Ok(())).unwrap();
}
