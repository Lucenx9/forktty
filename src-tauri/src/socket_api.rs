use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{oneshot, Notify};

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
const FRONTEND_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Pending frontend bridge requests: request_id -> response sender.
pub type PendingRequests = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

pub struct FrontendState {
    ready: AtomicBool,
    notify: Notify,
}

impl Default for FrontendState {
    fn default() -> Self {
        Self {
            ready: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
}

impl FrontendState {
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    async fn wait_until_ready(&self, timeout: Duration) -> bool {
        if self.is_ready() {
            return true;
        }

        let notified = self.notify.notified();
        if self.is_ready() {
            return true;
        }

        tokio::time::timeout(timeout, notified).await.is_ok() || self.is_ready()
    }
}

/// Resolve a pending bridge request (called from Tauri command).
pub fn resolve_request(pending: &PendingRequests, id: &str, result: Value) {
    let Ok(mut map) = pending.lock() else {
        eprintln!("CRITICAL: pending requests mutex poisoned");
        return;
    };
    if let Some(tx) = map.remove(id) {
        let _ = tx.send(result);
    }
}

/// Start the Unix socket JSON-RPC server.
pub async fn run(
    socket_path: String,
    app_handle: tauri::AppHandle,
    pty_manager: Arc<Mutex<PtyManager>>,
    pending: PendingRequests,
    frontend: Arc<FrontendState>,
) {
    // Remove stale socket file
    if Path::new(&socket_path).exists() {
        if let Err(e) = std::fs::remove_file(&socket_path) {
            eprintln!("Warning: could not remove stale socket at {socket_path}: {e}");
        }
    }

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind socket at {socket_path}: {e}");
            return;
        }
    };

    // Restrict socket to owner only (mode 0600) — security invariant
    if let Err(e) = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("CRITICAL: Failed to set socket permissions, removing socket: {e}");
        let _ = std::fs::remove_file(&socket_path);
        return;
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app_handle.clone();
                let ptys = pty_manager.clone();
                let pend = pending.clone();
                let front = frontend.clone();
                tokio::spawn(async move {
                    handle_connection(stream, app, ptys, pend, front).await;
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
    frontend: Arc<FrontendState>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);

    loop {
        let line = match read_limited_line(&mut buf_reader, MAX_REQUEST_SIZE).await {
            None => break, // EOF
            Some(Err(_)) => {
                let resp = json!({"id": null, "ok": false, "error": {"code": "request_too_large", "message": "Request exceeds 1 MiB"}});
                let _ = write_response(&mut writer, &resp).await;
                break;
            }
            Some(Ok(line)) => line,
        };

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

        let result = dispatch(method, params, &app, &pty_manager, &pending, &frontend).await;

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

/// Read a newline-delimited line, enforcing a maximum byte size before allocation.
/// Uses fill_buf/consume to avoid buffering unbounded data.
/// Returns None on EOF, Some(Err) if the line exceeds max_size, Some(Ok) otherwise.
async fn read_limited_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_size: usize,
) -> Option<Result<String, ()>> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let available = reader.fill_buf().await.ok()?;
        if available.is_empty() {
            return if buf.is_empty() {
                None
            } else {
                Some(String::from_utf8(buf).map_err(|_| ()))
            };
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            break;
        }
        let len = available.len();
        if buf.len() + len > max_size {
            return Some(Err(()));
        }
        buf.extend_from_slice(available);
        reader.consume(len);
    }
    if buf.len() > max_size {
        return Some(Err(()));
    }
    // Strip trailing \r for Windows-style line endings
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Some(String::from_utf8(buf).map_err(|_| ()))
}

async fn dispatch(
    method: &str,
    params: Value,
    app: &tauri::AppHandle,
    pty_manager: &Arc<Mutex<PtyManager>>,
    pending: &PendingRequests,
    frontend: &Arc<FrontendState>,
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
            wait_for_frontend_ready(frontend).await?;
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let prompt = params
                .get("prompt")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let cwd = request_cwd(&params)?;
            let layout = crate::config::load_config()
                .ok()
                .map(|c| c.general.worktree_layout)
                .filter(|layout| !layout.is_empty())
                .unwrap_or_else(|| "nested".to_string());

            let info = crate::worktree::create(&cwd, name, &layout).map_err(|e| e.to_string())?;

            let workspace = match bridge_to_frontend(
                app,
                pending,
                frontend,
                "workspace.create",
                json!({
                    "name": &info.branch,
                    "workingDir": &info.path,
                    "gitBranch": &info.branch,
                    "worktreeDir": &info.path,
                    "worktreeName": &info.worktree_name,
                    "prompt": &prompt,
                }),
            )
            .await
            {
                Ok(workspace) => workspace,
                Err(err) => {
                    let rollback = crate::worktree::remove(&cwd, &info.worktree_name, true);
                    let rollback_note = match rollback {
                        Ok(()) => "Rolled back the backend worktree state".to_string(),
                        Err(rollback_err) => format!("Rollback failed: {rollback_err}"),
                    };
                    return Err(format!(
                        "Failed to synchronize the frontend after creating worktree '{}': {err}. {rollback_note}",
                        info.worktree_name
                    ));
                }
            };

            if let (Some(prompt), Some(pty_id)) = (
                prompt.as_deref(),
                workspace.get("pty_id").and_then(|v| v.as_u64()),
            ) {
                write_surface_text(pty_manager, pty_id as u32, prompt)?;
            }

            // Intentional: setup hook failure is advisory and should not block worktree creation
            if let Ok(verified) = crate::verify_repo_path(&info.path) {
                let _ = crate::worktree::run_hook(&verified, "setup");
            }

            Ok(json!({
                "id": workspace.get("id").cloned().unwrap_or(Value::Null),
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
                "worktree_name": info.worktree_name,
            }))
        }

        "worktree.remove" => {
            wait_for_frontend_ready(frontend).await?;
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let cwd = request_cwd(&params)?;
            let plan = crate::worktree::prepare_remove(&cwd, name, true)
                .map_err(|e| e.to_string())?;
            let resolved_worktree_name = plan.worktree_name().to_string();

            // Intentional: teardown hook failure is advisory and should not block removal
            if let Ok(verified) = crate::verify_repo_path(plan.worktree_path()) {
                let _ = crate::worktree::run_hook(&verified, "teardown");
            }

            crate::worktree::execute_remove(&cwd, &plan).map_err(|e| e.to_string())?;
            match bridge_to_frontend(
                app,
                pending,
                frontend,
                "workspace.close",
                json!({ "worktreeName": resolved_worktree_name }),
            )
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    return Err(format!(
                        "Removed '{name}', but failed to synchronize the frontend: {err}"
                    ));
                }
            }

            Ok(json!(format!("Removed '{name}'")))
        }

        "worktree.merge" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing name")?;
            let cwd = request_cwd(&params)?;
            crate::worktree::merge(&cwd, name).map_err(|e| e.to_string())?;
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
        | "surface.read_screen"
        | "notification.create"
        | "notification.list"
        | "notification.clear"
        | "metadata.set_status"
        | "metadata.list_status"
        | "metadata.clear_status"
        | "metadata.set_progress"
        | "metadata.clear_progress"
        | "metadata.log" => bridge_to_frontend(app, pending, frontend, method, params).await,

        _ => Err(format!("Unknown method: {method}")),
    }
}

fn request_cwd(params: &Value) -> Result<String, String> {
    match params.get("cwd").and_then(|value| value.as_str()) {
        Some(cwd) if !cwd.trim().is_empty() => Ok(cwd.to_string()),
        _ => crate::cwd_string(),
    }
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
    frontend: &Arc<FrontendState>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    use tauri::Emitter;

    wait_for_frontend_ready(frontend).await?;

    let req_id = format!("sr-{}", NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = oneshot::channel();

    {
        let mut map = pending.lock().map_err(|e| format!("Lock: {e}"))?;
        if map.len() >= 100 {
            return Err("Too many pending bridge requests".to_string());
        }
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

async fn wait_for_frontend_ready(frontend: &Arc<FrontendState>) -> Result<(), String> {
    if frontend.wait_until_ready(FRONTEND_READY_TIMEOUT).await {
        Ok(())
    } else {
        Err("Frontend is not ready to handle socket requests".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader;

    // -- read_limited_line tests --

    #[tokio::test]
    async fn test_read_limited_line_normal() {
        let data = b"hello world\n";
        let mut reader = BufReader::new(Cursor::new(data.to_vec()));
        let result = read_limited_line(&mut reader, 1024).await;
        assert_eq!(result, Some(Ok("hello world".to_string())));
    }

    #[tokio::test]
    async fn test_read_limited_line_oversized() {
        let data = b"this line is too long\n";
        let mut reader = BufReader::new(Cursor::new(data.to_vec()));
        let result = read_limited_line(&mut reader, 5).await;
        assert_eq!(result, Some(Err(())));
    }

    #[tokio::test]
    async fn test_read_limited_line_eof_empty() {
        let data = b"";
        let mut reader = BufReader::new(Cursor::new(data.to_vec()));
        let result = read_limited_line(&mut reader, 1024).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_read_limited_line_eof_partial() {
        let data = b"partial";
        let mut reader = BufReader::new(Cursor::new(data.to_vec()));
        let result = read_limited_line(&mut reader, 1024).await;
        assert_eq!(result, Some(Ok("partial".to_string())));
    }

    #[tokio::test]
    async fn test_read_limited_line_crlf() {
        let data = b"windows line\r\n";
        let mut reader = BufReader::new(Cursor::new(data.to_vec()));
        let result = read_limited_line(&mut reader, 1024).await;
        assert_eq!(result, Some(Ok("windows line".to_string())));
    }

    #[tokio::test]
    async fn test_read_limited_line_invalid_utf8() {
        let data = vec![0xFF, 0xFE, b'\n'];
        let mut reader = BufReader::new(Cursor::new(data));
        let result = read_limited_line(&mut reader, 1024).await;
        assert_eq!(result, Some(Err(())));
    }

    #[tokio::test]
    async fn test_read_limited_line_empty_line() {
        let data = b"\n";
        let mut reader = BufReader::new(Cursor::new(data.to_vec()));
        let result = read_limited_line(&mut reader, 1024).await;
        assert_eq!(result, Some(Ok(String::new())));
    }

    // -- resolve_request tests --

    #[test]
    fn test_resolve_request_normal() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert("r1".to_string(), tx);

        resolve_request(&pending, "r1", json!({"ok": true}));

        let result = rx.try_recv().unwrap();
        assert_eq!(result, json!({"ok": true}));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn test_resolve_request_unknown_id() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        // Should not panic
        resolve_request(&pending, "nonexistent", json!(null));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn test_resolve_request_poisoned_mutex() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        // Poison the mutex by panicking inside a lock
        let pending_clone = pending.clone();
        let _ = std::thread::spawn(move || {
            let _guard = pending_clone.lock().unwrap();
            panic!("intentional poison");
        })
        .join();
        // Mutex is now poisoned — should not panic, just log
        resolve_request(&pending, "r1", json!(null));
    }
}
