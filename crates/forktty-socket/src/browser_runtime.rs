use forktty_core::{
    BookmarkStore, BrowserCmdError, BrowserCommand, BrowserOp, CmdResult, HistoryStore,
    ProfileError, ProfileId, SurfaceKind,
};
use serde_json::{json, Value};
use std::time::Duration;

use crate::{
    browser_params::{
        BrowserBookmarkAddRequest, BrowserBookmarkRemoveRequest, BrowserClickRequest,
        BrowserFillRequest, BrowserHistoryListRequest, BrowserHistorySearchRequest,
        BrowserNavigateRequest, BrowserOpenRequest, BrowserProfileCreateRequest,
        BrowserProfileDeleteRequest, BrowserProfileRequest, BrowserSurfaceRequest,
    },
    browser_profile, DispatchError, SocketAppState,
};

const BROWSER_CMD_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn open(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserOpenRequest::decode(params)?;
    let _surface_set_guard = state.surface_set_guard().await;
    let surface = {
        let _profile_store_guard = state
            .profile_store_lock
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let profile = match params.get("profile") {
            Some(value) => {
                let profile_name = value.as_str().ok_or_else(|| {
                    DispatchError::InvalidParam(
                        "Invalid parameter profile: expected string".to_string(),
                    )
                })?;
                browser_profile::profiles_store()?
                    .resolve(profile_name)
                    .ok_or(DispatchError::NotFound("profile".to_string()))?
            }
            None => ProfileId::default(),
        };
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .open_browser(&request.workspace_id, &request.url, profile, request.axis)
            .ok_or(DispatchError::NotFound("workspace".to_string()))?
    };
    Ok(json!(surface))
}

pub(crate) fn navigate(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserNavigateRequest::decode(params)?;
    let updated = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.set_surface_url(&request.surface_id, &request.url)
    };
    if updated {
        Ok(json!({"navigated": true}))
    } else {
        Err(DispatchError::NotFound("surface".to_string()))
    }
}

pub(crate) async fn snapshot(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserSurfaceRequest::decode(params)?;
    dispatch_cmd(state, request.surface_id, BrowserOp::Snapshot).await
}

pub(crate) async fn click(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserClickRequest::decode(params)?;
    dispatch_cmd(
        state,
        request.surface_id,
        BrowserOp::Click {
            reference: request.reference,
        },
    )
    .await
}

pub(crate) async fn fill(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserFillRequest::decode(params)?;
    dispatch_cmd(
        state,
        request.surface_id,
        BrowserOp::Fill {
            reference: request.reference,
            value: request.value,
        },
    )
    .await
}

pub(crate) async fn back(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserSurfaceRequest::decode(params)?;
    dispatch_cmd(state, request.surface_id, BrowserOp::Back).await
}

pub(crate) async fn forward(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserSurfaceRequest::decode(params)?;
    dispatch_cmd(state, request.surface_id, BrowserOp::Forward).await
}

pub(crate) async fn reload(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserSurfaceRequest::decode(params)?;
    dispatch_cmd(state, request.surface_id, BrowserOp::Reload).await
}

pub(crate) fn profile_list(
    state: &SocketAppState,
    _params: &Value,
) -> Result<Value, DispatchError> {
    let _profile_store_guard = state
        .profile_store_lock
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let store = browser_profile::profiles_store()?;
    let out: Vec<_> = store
        .list()
        .iter()
        .map(|p| {
            json!({
                "id": p.id.to_string(),
                "display_name": p.display_name,
                "is_default": p.is_default,
            })
        })
        .collect();
    Ok(json!(out))
}

pub(crate) fn profile_create(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserProfileCreateRequest::decode(params)?;
    let _profile_store_guard = state
        .profile_store_lock
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let mut store = browser_profile::profiles_store()?;
    let meta = store
        .create(&request.display_name)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!({ "id": meta.id.to_string(), "display_name": meta.display_name }))
}

pub(crate) fn profile_delete(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserProfileDeleteRequest::decode(params)?;
    let _profile_store_guard = state
        .profile_store_lock
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let in_use = model.list_surfaces(None).iter().any(|s| {
            matches!(
                &s.kind,
                SurfaceKind::Browser { profile, .. } if *profile == request.id
            )
        });
        if in_use {
            return Err(DispatchError::Conflict(
                "profile in use by an open browser pane".to_string(),
            ));
        }
    }
    let mut store = browser_profile::profiles_store()?;
    // On-disk data dir cleanup is still deferred to the GUI profile manager.
    store.delete(&request.id).map_err(|e| match e {
        ProfileError::NotFound => DispatchError::NotFound("profile".to_string()),
        ProfileError::CannotDeleteDefault => {
            DispatchError::Conflict("the default profile cannot be deleted".to_string())
        }
        other => DispatchError::from(other.to_string()),
    })?;
    Ok(json!({ "deleted": true }))
}

pub(crate) fn history_list(
    _state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserHistoryListRequest::decode(params)?;
    let store = HistoryStore::for_profile(request.profile)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    let rows = store
        .list(request.limit)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!(rows))
}

pub(crate) fn history_search(
    _state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserHistorySearchRequest::decode(params)?;
    let store = HistoryStore::for_profile(request.profile)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    let rows = store
        .search(&request.query, request.limit)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!(rows))
}

pub(crate) fn history_clear(
    _state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserProfileRequest::decode(params)?;
    let store = HistoryStore::for_profile(request.profile)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    store
        .clear()
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!({ "cleared": true }))
}

pub(crate) fn bookmark_add(
    _state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserBookmarkAddRequest::decode(params)?;
    let mut store = BookmarkStore::for_profile(request.profile)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    store
        .add(&request.url, &request.title)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!({ "added": true }))
}

pub(crate) fn bookmark_list(
    _state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserProfileRequest::decode(params)?;
    let store = BookmarkStore::for_profile(request.profile)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!(store.list()))
}

pub(crate) fn bookmark_remove(
    _state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserBookmarkRemoveRequest::decode(params)?;
    let mut store = BookmarkStore::for_profile(request.profile)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    let removed = store
        .remove(&request.url)
        .map_err(|e| DispatchError::from(e.to_string()))?;
    Ok(json!({ "removed": removed }))
}

/// Validate the surface is a browser, send `op` to the GTK side, and await the
/// reply within [`BROWSER_CMD_TIMEOUT`]. Maps [`CmdResult`] to a JSON value or a
/// [`DispatchError`].
pub(crate) async fn dispatch_cmd(
    state: &SocketAppState,
    surface_id: String,
    op: BrowserOp,
) -> Result<Value, DispatchError> {
    {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        match model.surface(&surface_id) {
            None => return Err(DispatchError::NotFound("surface".to_string())),
            Some(surface) => {
                if !matches!(surface.kind, SurfaceKind::Browser { .. }) {
                    return Err(DispatchError::NotFound("browser surface".to_string()));
                }
            }
        }
    }
    let Some(sender) = state.browser_cmd.clone() else {
        return Err("browser automation unavailable".into());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    sender
        .send(BrowserCommand {
            surface_id,
            op,
            reply: reply_tx,
        })
        .await
        .map_err(|_| DispatchError::from("browser automation unavailable"))?;
    let result = tokio::time::timeout(BROWSER_CMD_TIMEOUT, reply_rx)
        .await
        .map_err(|_| DispatchError::from("browser command timed out"))?
        .map_err(|_| DispatchError::Other("browser reply dropped".to_string()))?;
    match result {
        CmdResult::Ok => Ok(json!({"ok": true})),
        CmdResult::Json(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|e| DispatchError::Other(format!("invalid browser result json: {e}"))),
        CmdResult::Err(err) => Err(error_to_dispatch(err)),
    }
}

fn error_to_dispatch(err: BrowserCmdError) -> DispatchError {
    match err {
        BrowserCmdError::SurfaceGone => DispatchError::NotFound("surface".to_string()),
        BrowserCmdError::NotABrowser => DispatchError::NotFound("browser surface".to_string()),
        BrowserCmdError::NoWebView => DispatchError::NotFound("web view".to_string()),
        BrowserCmdError::RefNotFound => DispatchError::NotFound("element ref".to_string()),
        BrowserCmdError::ElementNotInteractable => {
            DispatchError::InvalidParam("element is not interactable".to_string())
        }
        BrowserCmdError::TooLarge => DispatchError::PayloadTooLarge {
            field: "result",
            limit: forktty_core::MAX_BROWSER_RESULT_BYTES,
            actual: forktty_core::MAX_BROWSER_RESULT_BYTES + 1,
        },
        BrowserCmdError::JsError(msg) => DispatchError::Other(msg),
        BrowserCmdError::Internal(msg) => DispatchError::Other(msg),
    }
}
