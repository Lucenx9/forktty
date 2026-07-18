//! Generation-bound GTK backend and embedded callback regressions.

use super::*;

#[test]
fn gtk_backend_rolls_back_spawn_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let backend = GtkTerminalBackend::new(tx);

    let err = backend.spawn(test_spawn_request()).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    assert!(backend.surfaces().unwrap().is_empty());
}

#[test]
fn gtk_backend_generations_are_nonzero_and_monotonic_across_surface_reuse() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);

    backend.spawn(test_spawn_request()).unwrap();
    let first_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn {
            request,
            generation,
        } => {
            assert_eq!(request.surface_id, "surface-1");
            generation
        }
        _ => panic!("expected first spawn command"),
    };
    assert_ne!(first_generation, 0);

    backend.show_surface("surface-1").unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::ShowSurface {
            surface_id,
            generation,
        } if surface_id == "surface-1" && generation == first_generation
    ));

    backend.close("surface-1").unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close {
            surface_id,
            generation,
        } if surface_id == "surface-1" && generation == first_generation
    ));

    backend.spawn(test_spawn_request()).unwrap();
    let second_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn {
            request,
            generation,
        } => {
            assert_eq!(request.surface_id, "surface-1");
            generation
        }
        _ => panic!("expected replacement spawn command"),
    };
    assert!(second_generation > first_generation);
}

#[test]
fn gtk_backend_runtime_entry_combines_generation_state_and_readiness() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);

    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected spawn command"),
    };
    backend
        .mark_surface_ready_for_generation("surface-1", generation)
        .unwrap();

    let (entry_generation, state, ready) = backend.runtime_entry_for_test("surface-1").unwrap();
    assert_eq!(entry_generation, generation);
    assert_eq!(state.surface_id, "surface-1");
    assert_eq!(state.cwd, PathBuf::from("/tmp"));
    assert!(ready);
}

#[test]
fn gtk_backend_close_deadline_keeps_leased_surface_registered() {
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(
        GtkTerminalBackend::new_with_generation_lease_timeout_for_test(tx, Duration::ZERO),
    );
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected spawn command"),
    };

    let (lease_acquired_tx, lease_acquired_rx) = mpsc::channel();
    let (release_lease_tx, release_lease_rx) = mpsc::channel();
    let lease_backend = backend.clone();
    let lease = std::thread::spawn(move || {
        lease_backend
            .with_surface_generation("surface-1", generation, || {
                lease_acquired_tx.send(()).unwrap();
                release_lease_rx.recv().unwrap();
            })
            .unwrap();
    });
    lease_acquired_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("generation lease should be acquired");

    let error = backend
        .close_for_generation("surface-1", generation)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("timed out waiting for generation leases"));
    assert!(backend.runtime_entry_for_test("surface-1").is_ok());
    assert!(rx.try_recv().is_err(), "timed-out close must not enqueue");

    release_lease_tx.send(()).unwrap();
    lease.join().unwrap();
    backend
        .close_for_generation("surface-1", generation)
        .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
}

#[test]
fn gtk_backend_forget_deadline_keeps_leased_surface_registered() {
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(
        GtkTerminalBackend::new_with_generation_lease_timeout_for_test(tx, Duration::ZERO),
    );
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected spawn command"),
    };

    let (lease_acquired_tx, lease_acquired_rx) = mpsc::channel();
    let (release_lease_tx, release_lease_rx) = mpsc::channel();
    let lease_backend = backend.clone();
    let lease = std::thread::spawn(move || {
        lease_backend
            .with_surface_generation("surface-1", generation, || {
                lease_acquired_tx.send(()).unwrap();
                release_lease_rx.recv().unwrap();
            })
            .unwrap();
    });
    lease_acquired_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("generation lease should be acquired");

    let error = backend.forget_surface("surface-1").unwrap_err();
    assert!(error
        .to_string()
        .contains("timed out waiting for generation leases"));
    assert!(backend.runtime_entry_for_test("surface-1").is_ok());
    assert!(rx.try_recv().is_err(), "timed-out forget must not enqueue");

    release_lease_tx.send(()).unwrap();
    lease.join().unwrap();
    backend.forget_surface("surface-1").unwrap();
    assert!(matches!(
        backend.runtime_entry_for_test("surface-1"),
        Err(TerminalError::NotFound(surface_id)) if surface_id == "surface-1"
    ));
}

#[test]
fn gtk_backend_stale_ready_callback_cannot_mark_replacement_ready() {
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

    assert!(matches!(
        backend.mark_surface_ready_for_generation("surface-1", stale_generation),
        Err(TerminalError::NotReady(surface_id)) if surface_id == "surface-1"
    ));
    assert!(!backend.surface_ready("surface-1").unwrap());

    backend
        .mark_surface_ready_for_generation("surface-1", current_generation)
        .unwrap();
    assert!(backend.surface_ready("surface-1").unwrap());
}

#[test]
fn gtk_backend_stale_generation_callbacks_leave_replacement_runtime_unchanged() {
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

    for result in [
        backend.mark_surface_ready_for_generation("surface-1", stale_generation),
        backend.mark_surface_not_ready_for_generation("surface-1", stale_generation),
        backend.mark_surface_pid_for_generation("surface-1", stale_generation, 1001),
        backend.clear_surface_pid_for_generation("surface-1", stale_generation),
        backend.resize_for_generation("surface-1", stale_generation, 132, 43),
        backend.close_for_generation("surface-1", stale_generation),
    ] {
        assert!(matches!(
            result,
            Err(TerminalError::NotReady(ref surface_id)) if surface_id == "surface-1"
        ));
    }

    let (generation, state, ready) = backend.runtime_entry_for_test("surface-1").unwrap();
    assert_eq!(generation, current_generation);
    assert_eq!(state.pid, None);
    assert_eq!((state.cols, state.rows), (80, 24));
    assert!(!ready);
}

#[test]
fn stale_embedded_model_and_close_callbacks_are_quietly_ignored() {
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

    let current_updates = Cell::new(0);
    let title_updates = Cell::new(0);
    let scrollback_updates = Cell::new(0);
    let child_exit_updates = Cell::new(0);
    let close_requests = Cell::new(0);
    assert!(run_embedded_callback_for_generation(
        &backend,
        "surface-1",
        current_generation,
        || current_updates.set(current_updates.get() + 1),
    )
    .is_some());
    assert!(
        run_embedded_callback_for_generation(&backend, "surface-1", stale_generation, || {
            title_updates.set(title_updates.get() + 1)
        },)
        .is_none()
    );
    assert!(
        run_embedded_callback_for_generation(&backend, "surface-1", stale_generation, || {
            scrollback_updates.set(scrollback_updates.get() + 1)
        },)
        .is_none()
    );
    assert!(
        run_embedded_callback_for_generation(&backend, "surface-1", stale_generation, || {
            child_exit_updates.set(child_exit_updates.get() + 1)
        },)
        .is_none()
    );
    assert!(
        run_embedded_callback_for_generation(&backend, "surface-1", stale_generation, || {
            close_requests.set(close_requests.get() + 1)
        },)
        .is_none()
    );

    assert_eq!(current_updates.get(), 1);
    assert_eq!(title_updates.get(), 0);
    assert_eq!(scrollback_updates.get(), 0);
    assert_eq!(child_exit_updates.get(), 0);
    assert_eq!(close_requests.get(), 0);
}

#[test]
fn embedded_title_callback_commits_before_a_replacement_generation_can_land() {
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let workspace = model
        .lock()
        .unwrap()
        .create_workspace("alpha", PathBuf::from("/tmp"));
    let surface_id = workspace.focused_surface_id;
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected initial spawn command"),
    };

    let lease_claimed = Arc::new(std::sync::Barrier::new(2));
    let release_lease = Arc::new(std::sync::Barrier::new(2));
    backend.set_after_generation_lease_hook_for_test({
        let backend = backend.clone();
        let lease_claimed = lease_claimed.clone();
        let release_lease = release_lease.clone();
        move || {
            assert!(
                !backend.runtime_is_locked_for_test(),
                "a generation lease must not hold the runtime mutex during model mutation"
            );
            lease_claimed.wait();
            release_lease.wait();
        }
    });

    let callback = {
        let backend = backend.clone();
        let model = model.clone();
        let surface_id = surface_id.clone();
        std::thread::spawn(move || {
            apply_embedded_title_for_generation(
                &backend,
                &model,
                &surface_id,
                generation,
                "generation one",
            )
        })
    };
    lease_claimed.wait();

    let (replacement_attempting_tx, replacement_attempting_rx) = mpsc::channel();
    let (replacement_done_tx, replacement_done_rx) = mpsc::channel();
    let replacement = {
        let backend = backend.clone();
        std::thread::spawn(move || {
            replacement_attempting_tx.send(()).unwrap();
            backend
                .close_for_generation("surface-1", generation)
                .unwrap();
            backend.spawn(test_spawn_request()).unwrap();
            replacement_done_tx.send(()).unwrap();
        })
    };
    replacement_attempting_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("replacement should reach the generation-bound close");
    let replacement_finished_while_callback_was_claimed = replacement_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_ok();
    release_lease.wait();

    assert!(callback.join().unwrap());
    if !replacement_finished_while_callback_was_claimed {
        replacement_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("replacement should complete after the title lease releases");
    }
    replacement.join().unwrap();
    assert!(
        !replacement_finished_while_callback_was_claimed,
        "replacement must wait for the title callback's generation lease"
    );
    assert_eq!(
        model.lock().unwrap().surface(&surface_id).unwrap().title,
        "generation one"
    );
}

#[test]
fn embedded_pid_timer_commits_backend_and_observation_before_replacement() {
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected initial spawn command"),
    };
    let observed = Arc::new(Mutex::new(BTreeMap::<String, SurfacePid>::new()));
    let lease_claimed = Arc::new(std::sync::Barrier::new(2));
    let release_lease = Arc::new(std::sync::Barrier::new(2));
    backend.set_after_generation_lease_hook_for_test({
        let lease_claimed = lease_claimed.clone();
        let release_lease = release_lease.clone();
        move || {
            lease_claimed.wait();
            release_lease.wait();
        }
    });

    let record = {
        let backend = backend.clone();
        let observed = observed.clone();
        std::thread::spawn(move || {
            commit_embedded_surface_pid_for_generation(
                &backend,
                "surface-1",
                generation,
                4242,
                |entry| {
                    observed
                        .lock()
                        .unwrap()
                        .insert("surface-1".to_string(), entry);
                },
            )
            .unwrap()
        })
    };
    lease_claimed.wait();

    let (replacement_attempting_tx, replacement_attempting_rx) = mpsc::channel();
    let (replacement_done_tx, replacement_done_rx) = mpsc::channel();
    let replacement = {
        let backend = backend.clone();
        std::thread::spawn(move || {
            replacement_attempting_tx.send(()).unwrap();
            backend
                .close_for_generation("surface-1", generation)
                .unwrap();
            backend.spawn(test_spawn_request()).unwrap();
            replacement_done_tx.send(()).unwrap();
        })
    };
    replacement_attempting_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("replacement should reach the generation-bound close");
    let replacement_finished_before_pid_commit = replacement_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_ok();
    release_lease.wait();

    assert!(record.join().unwrap());
    if !replacement_finished_before_pid_commit {
        replacement_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("replacement should complete after the PID lease releases");
    }
    replacement.join().unwrap();
    assert!(!replacement_finished_before_pid_commit);
    let entry = observed.lock().unwrap()["surface-1"];
    assert_eq!(
        entry,
        SurfacePid {
            pid: 4242,
            generation
        }
    );
    assert_eq!(
        current_embedded_surface_pid(&backend, "surface-1", entry),
        None,
        "surface-id consumers must reject a PID from an older generation"
    );
}

#[test]
fn listening_port_refresh_rejects_stale_embedded_pid_observations() {
    let _ = crate::test_env::with_gtk_test(|| {
        let (tx, rx) = mpsc::channel();
        let backend = Arc::new(GtkTerminalBackend::new(tx));
        let mut initial_model = WorkspaceModel::new();
        let workspace = initial_model.create_workspace("main", "/tmp");
        let workspace_id = workspace.id;
        let surface_id = workspace.focused_surface_id;
        assert!(initial_model.set_listening_ports(&workspace_id, vec![54321]));
        let model = Arc::new(Mutex::new(initial_model));
        backend.spawn(test_spawn_request()).unwrap();
        let stale_generation = match rx.recv().unwrap() {
            GtkTerminalCommand::Spawn { generation, .. } => generation,
            _ => panic!("expected initial spawn command"),
        };
        backend
            .close_for_generation(&surface_id, stale_generation)
            .unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            GtkTerminalCommand::Close { .. }
        ));
        backend.spawn(test_spawn_request()).unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            GtkTerminalCommand::Spawn { .. }
        ));

        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let parent = adw::ApplicationWindow::builder().build();
        let controller = Rc::new(RefCell::new(TerminalController::new(
            container,
            parent.clone(),
            model.clone(),
            backend,
        )));
        controller.borrow().surface_pids.borrow_mut().insert(
            surface_id,
            SurfacePid {
                pid: std::process::id() as i32,
                generation: stale_generation,
            },
        );
        let in_flight = Arc::new(AtomicBool::new(false));
        let scan_generation = Arc::new(AtomicU64::new(0));

        refresh_listening_ports(&controller, in_flight.clone(), scan_generation.clone());

        assert!(!in_flight.load(Ordering::SeqCst));
        assert_eq!(scan_generation.load(Ordering::SeqCst), 1);
        assert!(model
            .lock()
            .unwrap()
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
            .unwrap()
            .listening_ports
            .is_empty());
        parent.close();
    });
}

#[test]
fn embedded_child_exit_commits_runtime_pid_and_model_before_replacement() {
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    let mut initial_model = WorkspaceModel::new();
    let workspace = initial_model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id;
    let surface_id = workspace.focused_surface_id;
    let model = Arc::new(Mutex::new(initial_model));
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected initial spawn command"),
    };
    backend
        .mark_surface_ready_for_generation(&surface_id, generation)
        .unwrap();
    backend
        .mark_surface_pid_for_generation(&surface_id, generation, 4242)
        .unwrap();
    let observed = Arc::new(Mutex::new(BTreeMap::from([(
        surface_id.clone(),
        SurfacePid {
            pid: 4242,
            generation,
        },
    )])));
    let lease_claimed = Arc::new(std::sync::Barrier::new(2));
    let release_lease = Arc::new(std::sync::Barrier::new(2));
    backend.set_after_generation_lease_hook_for_test({
        let lease_claimed = lease_claimed.clone();
        let release_lease = release_lease.clone();
        move || {
            lease_claimed.wait();
            release_lease.wait();
        }
    });

    let child_exit = {
        let backend = backend.clone();
        let model = model.clone();
        let observed = observed.clone();
        let workspace_id = workspace_id.clone();
        let surface_id = surface_id.clone();
        std::thread::spawn(move || {
            commit_embedded_child_exit_for_generation(
                &backend,
                &model,
                &workspace_id,
                &surface_id,
                generation,
                Some(7),
                || {
                    remove_surface_pid_for_generation(
                        &mut observed.lock().unwrap(),
                        &surface_id,
                        generation,
                    );
                },
            )
        })
    };
    lease_claimed.wait();

    let (replacement_attempting_tx, replacement_attempting_rx) = mpsc::channel();
    let (replacement_done_tx, replacement_done_rx) = mpsc::channel();
    let replacement = {
        let backend = backend.clone();
        std::thread::spawn(move || {
            replacement_attempting_tx.send(()).unwrap();
            backend
                .close_for_generation("surface-1", generation)
                .unwrap();
            backend.spawn(test_spawn_request()).unwrap();
            replacement_done_tx.send(()).unwrap();
        })
    };
    replacement_attempting_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("replacement should reach the generation-bound close");
    let replacement_finished_before_exit_commit = replacement_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_ok();
    release_lease.wait();

    let notification = child_exit.join().unwrap().unwrap().unwrap();
    if !replacement_finished_before_exit_commit {
        replacement_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("replacement should complete after the child-exit lease releases");
    }
    replacement.join().unwrap();
    assert!(!replacement_finished_before_exit_commit);
    assert!(observed.lock().unwrap().is_empty());
    assert_eq!(notification.notification.title, "Terminal exited");
    let status = model
        .lock()
        .unwrap()
        .list_status(&workspace_id)
        .into_iter()
        .find(|entry| entry.key == surface_status_key(&surface_id))
        .unwrap();
    assert_eq!(status.value, "Exited (7)");
}

#[test]
fn stale_embedded_focus_and_scrollback_signal_seams_preserve_replacement_model() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let mut initial_model = WorkspaceModel::new();
    let workspace = initial_model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id;
    assert!(initial_model.mark_surface_unread(&surface_id, true));
    assert!(initial_model
        .set_surface_persisted_scrollback(&surface_id, Some("replacement output".to_string()),));
    let model = Arc::new(Mutex::new(initial_model));
    backend.spawn(test_spawn_request()).unwrap();
    let stale_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected initial spawn command"),
    };
    backend
        .close_for_generation(&surface_id, stale_generation)
        .unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
    backend.spawn(test_spawn_request()).unwrap();
    let current_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected replacement spawn command"),
    };

    assert!(!apply_embedded_focus_for_generation(
        &backend,
        &model,
        &surface_id,
        stale_generation,
    ));
    assert!(!commit_embedded_scrollback_for_generation(
        &backend,
        &model,
        &surface_id,
        stale_generation,
        "stale output".to_string(),
    ));
    let replacement = model.lock().unwrap().surface(&surface_id).unwrap().clone();
    assert!(replacement.unread);
    assert_eq!(
        replacement.persisted_scrollback.as_deref(),
        Some("replacement output")
    );

    assert!(apply_embedded_focus_for_generation(
        &backend,
        &model,
        &surface_id,
        current_generation,
    ));
    assert!(commit_embedded_scrollback_for_generation(
        &backend,
        &model,
        &surface_id,
        current_generation,
        "current output".to_string(),
    ));
    let current = model.lock().unwrap().surface(&surface_id).unwrap().clone();
    assert!(!current.unread);
    assert_eq!(
        current.persisted_scrollback.as_deref(),
        Some("current output")
    );
}

fn spawn_snapshot_test_surface(
    backend: &GtkTerminalBackend,
    rx: &mpsc::Receiver<GtkTerminalCommand>,
    surface_id: &str,
    workspace_id: &str,
) -> u64 {
    backend
        .spawn(SpawnRequest {
            surface_id: surface_id.to_string(),
            workspace_id: workspace_id.to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        })
        .unwrap();
    match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected snapshot test spawn command"),
    }
}

#[test]
fn owned_multi_surface_generation_lease_validates_and_releases_every_surface() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let first_generation = spawn_snapshot_test_surface(&backend, &rx, "surface-1", "workspace-1");
    let second_generation = spawn_snapshot_test_surface(&backend, &rx, "surface-2", "workspace-1");
    let callback_calls = Cell::new(0);

    backend
        .with_surface_generations(
            vec![
                ("surface-1".to_string(), first_generation),
                ("surface-2".to_string(), second_generation),
            ],
            || callback_calls.set(callback_calls.get() + 1),
        )
        .unwrap();

    assert_eq!(callback_calls.get(), 1);
    backend
        .close_for_generation("surface-1", first_generation)
        .unwrap();
    backend
        .close_for_generation("surface-2", second_generation)
        .unwrap();
}

#[test]
fn final_embedded_scrollback_snapshot_updates_all_successful_panes() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let mut initial_model = WorkspaceModel::new();
    let workspace = initial_model.create_workspace("main", "/tmp");
    let first_surface_id = workspace.focused_surface_id;
    let second_surface_id = initial_model
        .split_surface(&first_surface_id, SplitAxis::Horizontal)
        .unwrap()
        .id;
    let model = Arc::new(Mutex::new(initial_model));
    let first_generation =
        spawn_snapshot_test_surface(&backend, &rx, &first_surface_id, &workspace.id);
    let second_generation =
        spawn_snapshot_test_surface(&backend, &rx, &second_surface_id, &workspace.id);

    snapshot_current_generation_scrollback_with(
        &backend,
        &model,
        vec![
            (first_surface_id.clone(), first_generation, "first tail"),
            (second_surface_id.clone(), second_generation, "second tail"),
        ],
        |_, tail| {
            assert!(
                model.try_lock().is_ok(),
                "scrollback reads must not hold the model lock"
            );
            Some((*tail).to_string())
        },
    );

    let model = model.lock().unwrap();
    assert_eq!(
        model
            .surface(&first_surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some("first tail")
    );
    assert_eq!(
        model
            .surface(&second_surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some("second tail")
    );
}

#[test]
fn final_embedded_scrollback_snapshot_retains_failed_pane_tail() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let mut initial_model = WorkspaceModel::new();
    let workspace = initial_model.create_workspace("main", "/tmp");
    let first_surface_id = workspace.focused_surface_id;
    let second_surface_id = initial_model
        .split_surface(&first_surface_id, SplitAxis::Horizontal)
        .unwrap()
        .id;
    assert!(initial_model
        .set_surface_persisted_scrollback(&first_surface_id, Some("old first tail".to_string())));
    assert!(initial_model
        .set_surface_persisted_scrollback(&second_surface_id, Some("old second tail".to_string())));
    let model = Arc::new(Mutex::new(initial_model));
    let first_generation =
        spawn_snapshot_test_surface(&backend, &rx, &first_surface_id, &workspace.id);
    let second_generation =
        spawn_snapshot_test_surface(&backend, &rx, &second_surface_id, &workspace.id);

    snapshot_current_generation_scrollback_with(
        &backend,
        &model,
        vec![
            (first_surface_id.clone(), first_generation, true),
            (second_surface_id.clone(), second_generation, false),
        ],
        |surface_id, succeeds| succeeds.then(|| format!("new {surface_id} tail")),
    );

    let model = model.lock().unwrap();
    assert_eq!(
        model
            .surface(&first_surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some(format!("new {first_surface_id} tail").as_str())
    );
    assert_eq!(
        model
            .surface(&second_surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some("old second tail")
    );
}

#[test]
fn final_embedded_scrollback_snapshot_omits_stale_generation() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let mut initial_model = WorkspaceModel::new();
    let workspace = initial_model.create_workspace("main", "/tmp");
    let stale_surface_id = workspace.focused_surface_id;
    let current_surface_id = initial_model
        .split_surface(&stale_surface_id, SplitAxis::Horizontal)
        .unwrap()
        .id;
    assert!(initial_model
        .set_surface_persisted_scrollback(&stale_surface_id, Some("replacement tail".to_string())));
    let model = Arc::new(Mutex::new(initial_model));
    let stale_generation =
        spawn_snapshot_test_surface(&backend, &rx, &stale_surface_id, &workspace.id);
    let current_generation =
        spawn_snapshot_test_surface(&backend, &rx, &current_surface_id, &workspace.id);
    backend
        .close_for_generation(&stale_surface_id, stale_generation)
        .unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
    let replacement_generation =
        spawn_snapshot_test_surface(&backend, &rx, &stale_surface_id, &workspace.id);
    assert!(replacement_generation > stale_generation);

    snapshot_current_generation_scrollback_with(
        &backend,
        &model,
        vec![
            (stale_surface_id.clone(), stale_generation, "stale tail"),
            (
                current_surface_id.clone(),
                current_generation,
                "current tail",
            ),
        ],
        |surface_id, tail| {
            assert_ne!(
                surface_id, &stale_surface_id,
                "stale-generation pane must be omitted before reading"
            );
            Some((*tail).to_string())
        },
    );

    let model = model.lock().unwrap();
    assert_eq!(
        model
            .surface(&stale_surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some("replacement tail")
    );
    assert_eq!(
        model
            .surface(&current_surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some("current tail")
    );
}

#[test]
fn stale_embedded_layout_tick_does_not_queue_replacement_layout() {
    let _ = crate::test_env::with_gtk_test(|| {
        let (tx, rx) = mpsc::channel();
        let backend = GtkTerminalBackend::new(tx);
        backend.spawn(test_spawn_request()).unwrap();
        let stale_generation = match rx.recv().unwrap() {
            GtkTerminalCommand::Spawn { generation, .. } => generation,
            _ => panic!("expected initial spawn command"),
        };
        backend
            .close_for_generation("surface-1", stale_generation)
            .unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            GtkTerminalCommand::Close { .. }
        ));
        backend.spawn(test_spawn_request()).unwrap();
        let current_generation = match rx.recv().unwrap() {
            GtkTerminalCommand::Spawn { generation, .. } => generation,
            _ => panic!("expected replacement spawn command"),
        };
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let attempts = Cell::new(0);

        assert_eq!(
            run_embedded_initial_layout_tick(
                &backend,
                "surface-1",
                stale_generation,
                container.upcast_ref(),
                widget.upcast_ref(),
                &attempts,
            ),
            glib::ControlFlow::Break
        );
        assert_eq!(attempts.get(), 0);
        assert_eq!(
            run_embedded_initial_layout_tick(
                &backend,
                "surface-1",
                current_generation,
                container.upcast_ref(),
                widget.upcast_ref(),
                &attempts,
            ),
            glib::ControlFlow::Continue
        );
        assert_eq!(attempts.get(), 1);
    });
}

#[test]
fn stale_embedded_send_read_resize_and_close_do_not_touch_replacement_pane() {
    let current_calls = Cell::new(0);
    let send_calls = Cell::new(0);
    let read_calls = Cell::new(0);
    let resize_calls = Cell::new(0);
    let close_calls = Cell::new(0);

    assert!(run_embedded_pane_command_for_generation(2, 2, || {
        current_calls.set(current_calls.get() + 1)
    })
    .is_some());
    assert!(run_embedded_pane_command_for_generation(2, 1, || {
        send_calls.set(send_calls.get() + 1)
    })
    .is_none());
    assert!(run_embedded_pane_command_for_generation(2, 1, || {
        read_calls.set(read_calls.get() + 1)
    })
    .is_none());
    assert!(run_embedded_pane_command_for_generation(2, 1, || {
        resize_calls.set(resize_calls.get() + 1)
    })
    .is_none());
    assert!(run_embedded_pane_command_for_generation(2, 1, || {
        close_calls.set(close_calls.get() + 1)
    })
    .is_none());

    assert_eq!(current_calls.get(), 1);
    assert_eq!(send_calls.get(), 0);
    assert_eq!(read_calls.get(), 0);
    assert_eq!(resize_calls.get(), 0);
    assert_eq!(close_calls.get(), 0);
}

#[test]
fn stale_generation_bound_root_close_does_not_stage_a_replacement() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let surface_id = model
        .lock()
        .unwrap()
        .create_workspace("main", Path::new("/tmp"))
        .focused_surface_id;
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    spawn_focused_surface_if_needed(&state).unwrap();
    let stale_generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected first spawn command"),
    };
    backend.close(&surface_id).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Close { .. }
    ));
    let replacement_request = {
        let model = model.lock().unwrap();
        SpawnRequest::for_surface(
            model.surface(&surface_id).unwrap(),
            state.shell.clone(),
            state.socket_path.clone(),
        )
    };
    backend.spawn(replacement_request).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));

    let outcome =
        glib::MainContext::new().block_on(close_surface_by_id_for_generation_transaction(
            &state,
            &backend,
            &surface_id,
            stale_generation,
        ));

    assert_eq!(outcome, GenerationCloseOutcome::Stale);
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    let prepared = model
        .lock()
        .unwrap()
        .prepare_root_surface_replacement(&surface_id)
        .unwrap();
    assert_eq!(prepared.id, "surface-2");
}

#[test]
fn generation_bound_root_close_holds_surface_guard_through_commit() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let surface_id = model
        .lock()
        .unwrap()
        .create_workspace("main", Path::new("/tmp"))
        .focused_surface_id;
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    spawn_focused_surface_if_needed(&state).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected first spawn command"),
    };

    let (replacement_staged_tx, replacement_staged_rx) = mpsc::channel();
    let (release_replacement_tx, release_replacement_rx) = mpsc::channel();
    backend.set_after_enqueue_hook_for_test(move || {
        replacement_staged_tx.send(()).unwrap();
        release_replacement_rx.recv().unwrap();
    });
    let close = {
        let state = state.clone();
        let backend = backend.clone();
        let surface_id = surface_id.clone();
        std::thread::spawn(move || {
            glib::MainContext::new().block_on(close_surface_by_id_for_generation_transaction(
                &state,
                &backend,
                &surface_id,
                generation,
            ))
        })
    };
    replacement_staged_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("close should stage the root replacement");

    let (contender_attempting_tx, contender_attempting_rx) = mpsc::channel();
    let (contender_acquired_tx, contender_acquired_rx) = mpsc::channel();
    let contender = {
        let state = state.clone();
        std::thread::spawn(move || {
            glib::MainContext::new().block_on(async move {
                contender_attempting_tx.send(()).unwrap();
                let _guard = state.surface_set_guard().await;
                contender_acquired_tx.send(()).unwrap();
            });
        })
    };
    contender_attempting_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("contender should reach the shared surface guard");
    assert!(
        contender_acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "a competing topology transaction must wait through close commit"
    );

    release_replacement_tx.send(()).unwrap();
    assert_eq!(close.join().unwrap(), GenerationCloseOutcome::Closed);
    contender_acquired_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("surface guard should release after close commit");
    contender.join().unwrap();

    let replacement_id = model
        .lock()
        .unwrap()
        .active_workspace()
        .unwrap()
        .focused_surface_id;
    assert_ne!(replacement_id, surface_id);
    assert!(model.lock().unwrap().surface(&surface_id).is_none());
    let runtime_surfaces = backend.surfaces().unwrap();
    assert!(!runtime_surfaces
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    assert!(runtime_surfaces
        .iter()
        .any(|surface| surface.surface_id == replacement_id));
}

#[test]
fn generation_bound_close_evicts_hook_prompt_state_after_model_commit() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let workspace = model
        .lock()
        .unwrap()
        .create_workspace("main", Path::new("/tmp"));
    let workspace_id = workspace.id;
    let surface_id = workspace.focused_surface_id;
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    spawn_focused_surface_if_needed(&state).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected initial spawn command"),
    };
    let context = glib::MainContext::new();
    let prompt = context
        .block_on(forktty_socket::dispatch(
            &state,
            "notification.create",
            serde_json::json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "hook_session_id": "generation-close-session",
                "hook_event_name": "permission-request",
                "hook_event_clock": "boottime-ns",
                "hook_event_order": 100,
                "title": "Permission",
                "body": "Approve?",
                "kind": "prompt",
                "hook_agent": "opencode",
                "hook_prompt_kind": "permission",
                "hook_prompt_id": "opencode/session/permission/id:generation-close",
                "hook_correlation_id": "id:generation-close",
            }),
        ))
        .unwrap();

    let outcome = context.block_on(close_surface_by_id_for_generation_transaction(
        &state,
        &backend,
        &surface_id,
        generation,
    ));

    assert_eq!(outcome, GenerationCloseOutcome::Closed);
    let retained = model
        .lock()
        .unwrap()
        .list_notifications()
        .into_iter()
        .find(|notification| notification.id == prompt["id"])
        .expect("closed-surface prompt should remain in history");
    assert!(
        retained.read,
        "closed-surface prompt correlation must retire"
    );
}

#[test]
fn generation_bound_root_close_rolls_back_runtime_when_close_enqueue_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let surface_id = model
        .lock()
        .unwrap()
        .create_workspace("main", Path::new("/tmp"))
        .focused_surface_id;
    let (tx, rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    spawn_focused_surface_if_needed(&state).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected first spawn command"),
    };

    let (replacement_staged_tx, replacement_staged_rx) = mpsc::channel();
    let (release_replacement_tx, release_replacement_rx) = mpsc::channel();
    backend.set_after_enqueue_hook_for_test(move || {
        replacement_staged_tx.send(()).unwrap();
        release_replacement_rx.recv().unwrap();
    });
    let close = {
        let state = state.clone();
        let backend = backend.clone();
        let surface_id = surface_id.clone();
        std::thread::spawn(move || {
            glib::MainContext::new().block_on(close_surface_by_id_for_generation_transaction(
                &state,
                &backend,
                &surface_id,
                generation,
            ))
        })
    };
    replacement_staged_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("close should enqueue the replacement before closing the old runtime");
    drop(rx);
    release_replacement_tx.send(()).unwrap();

    assert_eq!(close.join().unwrap(), GenerationCloseOutcome::Failed);
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    let (restored_generation, _, _) = backend.runtime_entry_for_test(&surface_id).unwrap();
    assert_eq!(restored_generation, generation);
    assert_eq!(backend.surfaces().unwrap().len(), 1);
}
