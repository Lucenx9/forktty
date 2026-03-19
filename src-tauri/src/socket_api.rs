use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::oneshot;

use crate::pty_manager::PtyManager;

/// Maximum request size (1 MiB).
const MAX_REQUEST_SIZE: usize = 1_048_576;

/// Returns the default socket path, preferring XDG_RUNTIME_DIR for security.
pub fn default_socket_path() -> String {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        format!("{runtime_dir}/forktty.sock")
    } else {
        "/tmp/forktty.sock".to_string()
    }
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Pending frontend bridge requests: request_id -> response sender.
pub type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// Resolve a pending bridge request (called from Tauri command).
pub fn resolve_request(pending: &PendingRequests, id: &str, result: Value) {
    if let Ok(mut map) = pending.lock() {
        if let Some(tx) = map.remove(id) {
            let _ = tx.send(result);
        }
    }
}

/// Start the Unix socket JSON-RPC server.
pub async fn run(
    socket_path: String,
    app_handle: tauri::AppHandle,
    pty_manager: Arc<Mutex<PtyManager>>,
    pending: PendingRequests,
) {
    // Remove stale socket file
    if Path::new(&socket_path).exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind socket at {socket_path}: {e}");
            return;
        }
    };

    // Restrict socket to owner only (mode 0600)
    if let Err(e) = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("Failed to set socket permissions: {e}");
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app_handle.clone();
                let ptys = pty_manager.clone();
                let pend = pending.clone();
                tokio::spawn(async move {
                    handle_connection(stream, app, ptys, pend).await;
                });
            }
            Err(e) => {
                eprintln!("Socket accept error: {e}");
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    app: tauri::AppHandle,
    pty_manager: Arc<Mutex<PtyManager>>,
    pending: PendingRequests,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // `BufReader::lines()` buffers the full line before yielding it, so the
    // 1 MiB check below is post-read. Fixing that requires length-prefixed
    // framing instead of newline-delimited JSON, which is out of scope here.
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_REQUEST_SIZE {
            let resp = json!({"id": null, "ok": false, "error": {"code": "request_too_large", "message": "Request exceeds 1 MiB"}});
            let _ = write_response(&mut writer, &resp).await;
            break;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = json!({"id": null, "ok": false, "error": {"code": "parse_error", "message": e.to_string()}});
                let _ = write_response(&mut writer, &resp).await;
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        let result = dispatch(method, params, &app, &pty_manager, &pending).await;

        let resp = match result {
            Ok(val) => json!({"id": id, "ok": true, "result": val}),
            Err(msg) => json!({"id": id, "ok": false, "error": {"code": "error", "message": msg}}),
        };

        if write_response(&mut writer, &resp).await.is_err() {
            break;
        }
    }
}

async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &Value,
) -> Result<(), std::io::Error> {
    let mut bytes = serde_json::to_vec(resp)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn dispatch(
    method: &str,
    params: Value,
    app: &tauri::AppHandle,
    pty_manager: &Arc<Mutex<PtyManager>>,
    pending: &PendingRequests,
) -> Result<Value, String> {
    match method {
        // --- Direct backend handlers ---
        "system.ping" => Ok(json!("pong")),

        "surface.send_text" => {
            let pty_id = params
                .get("pty_id")
                .and_then(|v| v.as_u64())
                .ok_or("Missing pty_id")? as u32;
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("Missing text")?;
            write_surface_text(pty_manager, pty_id, text)?;
            Ok(json!(true))
        }

        "worktree.create" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let prompt = params
                .get("prompt")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let cwd = current_dir_string()?;
            let layout = crate::config::load_config()
                .ok()
                .map(|c| c.general.worktree_layout)
                .filter(|layout| !layout.is_empty())
                .unwrap_or_else(|| "nested".to_string());

            let info = crate::worktree::create(&cwd, name, &layout).map_err(|e| e.to_string())?;
            // Intentional: setup hook failure is advisory and should not block worktree creation
            let _ = crate::worktree::run_hook(&info.path, "setup");

            let workspace = bridge_to_frontend(
                app,
                pending,
                "workspace.create",
                json!({
                    "name": &info.name,
                    "workingDir": &info.path,
                    "gitBranch": &info.branch,
                    "worktreeDir": &info.path,
                    "worktreeName": &info.name,
                    "prompt": &prompt,
                }),
            )
            .await?;

            if let (Some(prompt), Some(pty_id)) = (
                prompt.as_deref(),
                workspace.get("pty_id").and_then(|v| v.as_u64()),
            ) {
                write_surface_text(pty_manager, pty_id as u32, prompt)?;
            }

            Ok(json!({
                "id": workspace.get("id").cloned().unwrap_or(Value::Null),
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
            }))
        }

        "worktree.remove" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let cwd = current_dir_string()?;

            if let Ok(worktrees) = crate::worktree::list(&cwd) {
                if let Some(wt) = worktrees.iter().find(|w| w.name == name) {
                    // Intentional: teardown hook failure is advisory and should not block removal
                    let _ = crate::worktree::run_hook(&wt.path, "teardown");
                }
            }

            crate::worktree::remove(&cwd, name, true).map_err(|e| e.to_string())?;
            let _ =
                bridge_to_frontend(app, pending, "workspace.close", json!({ "name": name })).await;

            Ok(json!(format!("Removed '{name}'")))
        }

        "worktree.merge" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            crate::worktree::merge(&cwd.to_string_lossy(), name).map_err(|e| e.to_string())?;
            Ok(json!(format!("Merged '{name}'")))
        }

        // --- Bridged to frontend ---
        "workspace.list"
        | "workspace.create"
        | "workspace.select"
        | "workspace.close"
        | "surface.list"
        | "surface.split"
        | "surface.close"
        | "notification.create"
        | "notification.list"
        | "notification.clear" => bridge_to_frontend(app, pending, method, params).await,

        _ => Err(format!("Unknown method: {method}")),
    }
}

fn current_dir_string() -> Result<String, String> {
    std::env::current_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

fn write_surface_text(
    pty_manager: &Arc<Mutex<PtyManager>>,
    pty_id: u32,
    text: &str,
) -> Result<(), String> {
    if text.len() > MAX_REQUEST_SIZE {
        return Err("Text exceeds 1 MiB".to_string());
    }
    let mgr = pty_manager.lock().map_err(|e| format!("Lock: {e}"))?;
    mgr.write(pty_id, text.as_bytes())
        .map_err(|e| e.to_string())
}

/// Forward a request to the frontend via Tauri events, wait for response.
async fn bridge_to_frontend(
    app: &tauri::AppHandle,
    pending: &PendingRequests,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    use tauri::Emitter;

    let req_id = format!("sr-{}", NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = oneshot::channel();

    {
        let mut map = pending.lock().map_err(|e| format!("Lock: {e}"))?;
        map.insert(req_id.clone(), tx);
    }

    app.emit(
        "socket-request",
        json!({ "id": req_id, "method": method, "params": params }),
    )
    .map_err(|e| format!("Emit failed: {e}"))?;

    match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
        Ok(Ok(result)) => {
            if let Some(err) = result.get("error").and_then(|e| e.as_str()) {
                Err(err.to_string())
            } else {
                Ok(result.get("result").cloned().unwrap_or(result))
            }
        }
        Ok(Err(_)) => Err("Bridge channel closed".to_string()),
        Err(_) => {
            // Clean up timed-out request
            if let Ok(mut map) = pending.lock() {
                map.remove(&req_id);
            }
            Err("Request timed out (frontend not responding)".to_string())
        }
    }
}
