//! Team state validation and store runtime regression tests.

use super::*;

#[tokio::test]
async fn team_upsert_rejects_leader_surface_from_another_workspace() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let first_surface_id = first[0]["focused_surface_id"].as_str().unwrap();
    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    let err = dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": other["id"].clone(),
            "leader_surface_id": first_surface_id
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("leader_surface_id"));
}

#[test]
fn team_store_update_does_not_block_current_thread_runtime() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("team-v1.json");
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store_path.with_extension("lock"))
        .unwrap();
    lock_file.lock().unwrap();
    state.team_store_path = Some(store_path);

    let (ping_tx, ping_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let runtime_state = state.clone();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let update_state = runtime_state.clone();
            let update = tokio::spawn(async move {
                dispatch(
                    &update_state,
                    "team.upsert",
                    json!({"team_id": "team-1", "name": "Runtime", "status": "active"}),
                )
                .await
            });
            tokio::task::yield_now().await;
            let ping = dispatch(&runtime_state, "system.ping", json!({})).await;
            ping_tx.send(ping.is_ok()).unwrap();
            done_tx.send(update.await.unwrap().is_ok()).unwrap();
        });
    });

    let ping_before_unlock = ping_rx
        .recv_timeout(Duration::from_millis(200))
        .unwrap_or(false);
    drop(lock_file);
    assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
    thread.join().unwrap();
    assert!(
        ping_before_unlock,
        "team store I/O must not block unrelated socket work on a current-thread runtime"
    );
}
