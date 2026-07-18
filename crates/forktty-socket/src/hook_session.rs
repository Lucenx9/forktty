use crate::{
    agent_kind_from_status_key, ensure_max_text_size, optional_hook_status_metadata,
    optional_non_blank_string_param, optional_surface_id_param, path_resolver,
    resolve_workspace_id_for_metadata, workspace_selector_params, DispatchError, SocketAppState,
};
use forktty_core::{AgentSessionLifecycle, HookPromptKind, HookPromptResolution};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct HookSessionTarget {
    workspace_id: String,
    surface_id: String,
}

struct HookSessionCwdSurface {
    target: HookSessionTarget,
    cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HookEventWatermark {
    clock: Option<String>,
    order: u128,
}

#[derive(Default, Debug)]
struct HookSessionIngress {
    target: Option<HookSessionTarget>,
    watermark: Option<HookEventWatermark>,
    takeover: Option<HookSessionTakeover>,
}

#[derive(Debug)]
struct HookSessionTakeover {
    from_session_id: String,
    event_name: Option<String>,
    clock: Option<String>,
    order: Option<u128>,
}

#[derive(Default, Debug)]
pub(super) struct HookTargetGates {
    entries: HashMap<HookSessionTarget, Weak<HookTargetGate>>,
}

impl HookTargetGates {
    fn gate_for(&mut self, target: &HookSessionTarget) -> Arc<HookTargetGate> {
        self.entries.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = self.entries.get(target).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(HookTargetGate::default());
        self.entries.insert(target.clone(), Arc::downgrade(&gate));
        gate
    }
}

#[derive(Default, Debug)]
pub(super) struct HookTargetGate {
    lock: Mutex<()>,
    #[cfg(test)]
    contention: HookTargetGateContention,
    #[cfg(test)]
    panic_next_completion_lock: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
#[derive(Default, Debug)]
struct HookTargetGateContention {
    waiters: Mutex<usize>,
    changed: std::sync::Condvar,
}

impl HookTargetGate {
    pub(super) fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, ()>> {
        #[cfg(test)]
        {
            match self.lock.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(std::sync::TryLockError::Poisoned(err)) => return Err(err),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            {
                let mut waiters = self
                    .contention
                    .waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *waiters = waiters.saturating_add(1);
                self.contention.changed.notify_all();
            }
            let result = self.lock.lock();
            let mut waiters = self
                .contention
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *waiters = waiters.saturating_sub(1);
            result
        }
        #[cfg(not(test))]
        self.lock.lock()
    }

    pub(super) fn clear_poison(&self) {
        self.lock.clear_poison();
    }

    #[cfg(test)]
    pub(super) fn panic_next_completion_lock(&self) {
        self.panic_next_completion_lock
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn panic_for_requested_completion_lock(&self) {
        if self
            .panic_next_completion_lock
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            let _guard = self
                .lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            panic!("injected panic while reacquiring a removal hook target gate");
        }
    }

    #[cfg(test)]
    pub(super) fn wait_until_contended(&self, timeout: std::time::Duration) -> bool {
        let waiters = self
            .contention
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (waiters, _) = self
            .contention
            .changed
            .wait_timeout_while(waiters, timeout, |waiters| *waiters == 0)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *waiters > 0
    }
}

pub(super) fn hook_target_gate_for_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<Arc<HookTargetGate>, DispatchError> {
    let workspace_id = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .surface(surface_id)
        .map(|surface| surface.workspace_id.clone())
        .ok_or(DispatchError::NotFound("surface".to_string()))?;
    let target = HookSessionTarget {
        workspace_id,
        surface_id: surface_id.to_string(),
    };
    let mut gates = state
        .hook_target_gates
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(gates.gate_for(&target))
}

pub(super) fn hook_target_gates_for_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<Vec<Arc<HookTargetGate>>, DispatchError> {
    let mut surface_ids = surface_ids.to_vec();
    surface_ids.sort();
    surface_ids.dedup();
    surface_ids
        .iter()
        .map(|surface_id| hook_target_gate_for_surface(state, surface_id))
        .collect()
}

pub(super) fn lock_hook_target_gates(
    gates: &[Arc<HookTargetGate>],
) -> Result<Vec<std::sync::MutexGuard<'_, ()>>, DispatchError> {
    let mut guards = Vec::with_capacity(gates.len());
    for gate in gates {
        guards.push(gate.lock().map_err(|_| "Lock poisoned".to_string())?);
    }
    Ok(guards)
}

pub(super) fn lock_hook_target_gates_for_completion(
    gates: &[Arc<HookTargetGate>],
) -> (Vec<std::sync::MutexGuard<'_, ()>>, bool) {
    let mut guards = Vec::with_capacity(gates.len());
    let mut recovered_poison = false;
    for gate in gates {
        #[cfg(test)]
        gate.panic_for_requested_completion_lock();
        match gate.lock() {
            Ok(guard) => guards.push(guard),
            Err(poisoned) => {
                recovered_poison = true;
                gate.clear_poison();
                guards.push(poisoned.into_inner());
            }
        }
    }
    (guards, recovered_poison)
}

#[derive(Default, Debug)]
pub(super) struct HookSessionTargets {
    entries: HashMap<String, Arc<Mutex<HookSessionIngress>>>,
    order: VecDeque<String>,
}

impl HookSessionTargets {
    fn ingress_for(&mut self, session_id: &str) -> Arc<Mutex<HookSessionIngress>> {
        if let Some(ingress) = self.entries.get(session_id).cloned() {
            self.order.retain(|existing| existing != session_id);
            self.order.push_back(session_id.to_string());
            return ingress;
        }

        self.evict_oldest_idle_entry();
        let ingress = Arc::new(Mutex::new(HookSessionIngress::default()));
        self.entries.insert(session_id.to_string(), ingress.clone());
        self.order.push_back(session_id.to_string());
        ingress
    }

    fn evict_oldest_idle_entry(&mut self) {
        if self.entries.len() < super::HOOK_SESSION_TARGET_CAPACITY {
            return;
        }
        let candidates = self.order.len();
        for _ in 0..candidates {
            let Some(oldest) = self.order.pop_front() else {
                return;
            };
            let idle = self
                .entries
                .get(&oldest)
                .is_some_and(|ingress| Arc::strong_count(ingress) == 1);
            if idle {
                self.entries.remove(&oldest);
                return;
            }
            self.order.push_back(oldest);
        }
    }

    pub(super) fn remove_surface(&mut self, surface_id: &str) {
        for ingress in self.entries.values() {
            let mut ingress = ingress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if ingress
                .target
                .as_ref()
                .is_some_and(|target| target.surface_id == surface_id)
            {
                ingress.target = None;
            }
        }
    }
}

pub(crate) fn optional_hook_session_cwd(params: &Value) -> Result<Option<PathBuf>, DispatchError> {
    let Some(raw) = optional_non_blank_string_param(params, "hook_session_cwd")? else {
        return Ok(None);
    };
    ensure_max_text_size("hook_session_cwd", raw)?;
    let path = Path::new(raw);
    if path.is_absolute() {
        match path_resolver::canonical_existing_dir(path, "hook_session_cwd") {
            Ok(canonical) => return Ok(Some(canonical)),
            Err(_) => return Ok(None),
        }
    }
    Ok(None)
}

pub(super) fn run_serialized_hook_ingress<F>(
    state: &SocketAppState,
    method: &str,
    params: &mut Value,
    mutation: F,
) -> Result<Option<Value>, DispatchError>
where
    F: FnOnce(&Value) -> Result<Value, DispatchError>,
{
    if !is_hook_targetable_method(method) {
        return Ok(None);
    }
    let Some(session_id) = optional_non_blank_string_param(params, "hook_session_id")? else {
        return Ok(None);
    };
    ensure_max_text_size("hook_session_id", session_id)?;
    let session_id = session_id.to_string();
    let hook = optional_hook_status_metadata(params)?;
    let event_name = hook.as_ref().map(|hook| hook.event.as_str());
    let cleanup_prompts_on_success =
        method == "metadata.clear_status" && event_name == Some("session-end");
    let evict_on_success = should_evict_hook_session_target_on_return(method, params, event_name)?;
    let ingress = state
        .hook_session_targets
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .ingress_for(&session_id);
    let mut ingress = ingress.lock().map_err(|_| "Lock poisoned".to_string())?;

    let previous_target = ingress.target.clone();
    let learned_target = resolve_hook_session_target(state, params, &mut ingress)?;
    let target = learned_target.clone().or_else(|| ingress.target.clone());
    let target_gate = if let Some(target) = target.as_ref() {
        Some(
            state
                .hook_target_gates
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?
                .gate_for(target),
        )
    } else {
        None
    };
    let _target_guard = if let Some(gate) = target_gate.as_ref() {
        Some(gate.lock().map_err(|_| "Lock poisoned".to_string())?)
    } else {
        None
    };
    let incoming_clock = hook.as_ref().and_then(|hook| hook.clock.as_deref());
    let incoming_order = hook.as_ref().and_then(|hook| hook.order);
    if hook_session_is_superseded(
        state,
        &session_id,
        event_name,
        incoming_clock,
        incoming_order,
        target.as_ref(),
        &mut ingress,
    )? {
        return Ok(Some(json!({"ignored": true})));
    }
    let decision =
        hook_ingress_decision(ingress.watermark.as_ref(), incoming_clock, incoming_order);
    let HookIngressDecision::Apply { next_watermark } = decision else {
        return Ok(Some(json!({"ignored": true})));
    };

    let provider = optional_non_blank_string_param(params, "hook_agent")?;
    if let Some(provider) = provider {
        ensure_max_text_size("hook_agent", provider)?;
    }
    let prompt_resolution = completed_hook_prompt_resolution(
        params,
        &session_id,
        hook.as_ref(),
        provider,
        learned_target.as_ref(),
    )?;
    let result = mutation(params)?;
    resolve_completed_hook_prompt(state, prompt_resolution)?;
    if cleanup_prompts_on_success {
        if let (Some(provider), Some(target)) = (provider, target.as_ref()) {
            evict_hook_prompt_correlations_for_session_target(
                state,
                provider,
                &session_id,
                target,
            )?;
        }
    }
    if let (Some(previous_target), Some(learned_target)) =
        (previous_target.as_ref(), learned_target.as_ref())
    {
        if previous_target != learned_target {
            if let Some(provider) = provider {
                evict_hook_prompt_correlations_for_session_target(
                    state,
                    provider,
                    &session_id,
                    previous_target,
                )?;
            }
        }
    }
    if let Some(target) = learned_target {
        ingress.target = Some(target);
    }
    if let Some(next_watermark) = next_watermark {
        ingress.watermark = Some(next_watermark);
    }
    if evict_on_success {
        ingress.target = None;
    }
    Ok(Some(result))
}

fn evict_hook_prompt_correlations_for_session_target(
    state: &SocketAppState,
    provider: &str,
    session_id: &str,
    previous_target: &HookSessionTarget,
) -> Result<(), DispatchError> {
    let desktop_notification_ids = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .evict_hook_prompt_correlations_for_session_target(
            provider,
            session_id,
            &previous_target.surface_id,
        );
    for notification_id in desktop_notification_ids {
        state.close_desktop_notification(&notification_id);
    }
    Ok(())
}

fn completed_hook_prompt_resolution(
    params: &Value,
    session_id: &str,
    hook: Option<&forktty_core::StatusHookMetadata>,
    provider: Option<&str>,
    target: Option<&HookSessionTarget>,
) -> Result<Option<HookPromptResolution>, DispatchError> {
    let Some(hook) = hook else {
        return Ok(None);
    };
    let kind = match hook.event.as_str() {
        "permission-replied" | "permission-result" | "permission-denied" | "post-tool-batch" => {
            HookPromptKind::Permission
        }
        "elicitation-result" => HookPromptKind::Elicitation,
        _ => return Ok(None),
    };
    let Some(provider) = provider else {
        return Ok(None);
    };
    if let Some(supplied_kind) = optional_non_blank_string_param(params, "hook_prompt_kind")? {
        let expected = match kind {
            HookPromptKind::Permission => "permission",
            HookPromptKind::Elicitation => "elicitation",
            HookPromptKind::Attention => "attention",
        };
        if supplied_kind != expected {
            return Err(DispatchError::InvalidParam(format!(
                "Invalid parameter hook_prompt_kind: expected {expected} for {}",
                hook.event
            )));
        }
    }
    let Some(event_order) = hook.order else {
        return Ok(None);
    };
    let correlation_id = optional_non_blank_string_param(params, "hook_correlation_id")?;
    if let Some(correlation_id) = correlation_id {
        ensure_max_text_size("hook_correlation_id", correlation_id)?;
    }
    let workspace_id = optional_non_blank_string_param(params, "workspace_id")?;
    let surface_id = optional_non_blank_string_param(params, "surface_id")?;
    Ok(Some(HookPromptResolution {
        provider: provider.to_string(),
        session_id: session_id.to_string(),
        kind,
        event_order,
        correlation_id: correlation_id.map(str::to_string),
        workspace_id: target
            .map(|target| target.workspace_id.clone())
            .or_else(|| workspace_id.map(str::to_string)),
        surface_id: target
            .map(|target| target.surface_id.clone())
            .or_else(|| surface_id.map(str::to_string)),
    }))
}

fn resolve_completed_hook_prompt(
    state: &SocketAppState,
    resolution: Option<HookPromptResolution>,
) -> Result<(), DispatchError> {
    let Some(resolution) = resolution else {
        return Ok(());
    };
    let resolved = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .resolve_hook_prompt(&resolution);
    if let Some(notification) = resolved {
        state.close_desktop_notification(&notification.id);
    }
    Ok(())
}

fn hook_session_is_superseded(
    state: &SocketAppState,
    session_id: &str,
    event_name: Option<&str>,
    incoming_clock: Option<&str>,
    incoming_order: Option<u128>,
    target: Option<&HookSessionTarget>,
    ingress: &mut HookSessionIngress,
) -> Result<bool, DispatchError> {
    let Some(target) = target else {
        return Ok(false);
    };
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let Some(current) = model
        .surface(&target.surface_id)
        .filter(|surface| surface.workspace_id == target.workspace_id)
        .and_then(|surface| surface.agent_session.as_ref())
    else {
        return Ok(false);
    };
    if current.lifecycle == AgentSessionLifecycle::Suspended {
        return Ok(true);
    }
    if current.session_id == session_id || current.lifecycle == AgentSessionLifecycle::Ended {
        ingress.takeover = None;
        return Ok(false);
    }

    let starts_takeover = ingress.takeover.is_none() && ingress.watermark.is_none();
    let continues_takeover = ingress.takeover.as_ref().is_some_and(|takeover| {
        takeover.from_session_id == current.session_id
            && takeover.event_name.as_deref() == event_name
            && takeover.clock.as_deref() == incoming_clock
            && takeover.order == incoming_order
    });
    if starts_takeover || continues_takeover {
        ingress.takeover = Some(HookSessionTakeover {
            from_session_id: current.session_id.clone(),
            event_name: event_name.map(str::to_string),
            clock: incoming_clock.map(str::to_string),
            order: incoming_order,
        });
        return Ok(false);
    }
    Ok(true)
}

#[derive(Debug, PartialEq, Eq)]
enum HookIngressDecision {
    Ignore,
    Apply {
        next_watermark: Option<HookEventWatermark>,
    },
}

fn hook_ingress_decision(
    watermark: Option<&HookEventWatermark>,
    incoming_clock: Option<&str>,
    incoming_order: Option<u128>,
) -> HookIngressDecision {
    let Some(incoming_order) = incoming_order else {
        return HookIngressDecision::Apply {
            next_watermark: None,
        };
    };
    let incoming_clock = incoming_clock.map(str::to_string);
    let Some(watermark) = watermark else {
        return HookIngressDecision::Apply {
            next_watermark: Some(HookEventWatermark {
                clock: incoming_clock,
                order: incoming_order,
            }),
        };
    };
    if watermark.clock != incoming_clock {
        return HookIngressDecision::Apply {
            next_watermark: Some(HookEventWatermark {
                clock: incoming_clock,
                order: incoming_order,
            }),
        };
    }
    if incoming_order < watermark.order {
        return HookIngressDecision::Ignore;
    }
    HookIngressDecision::Apply {
        next_watermark: (incoming_order > watermark.order).then_some(HookEventWatermark {
            clock: watermark.clock.clone(),
            order: incoming_order,
        }),
    }
}

fn resolve_hook_session_target(
    state: &SocketAppState,
    params: &mut Value,
    ingress: &mut HookSessionIngress,
) -> Result<Option<HookSessionTarget>, DispatchError> {
    let surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let workspace_selectors = workspace_selector_params(params)?;
    let has_explicit_workspace = !workspace_selectors.is_empty();
    if let Some(surface_id) = surface_id {
        let workspace_id = resolve_workspace_id_for_metadata(state, params)?;
        let target = HookSessionTarget {
            workspace_id,
            surface_id,
        };
        if !hook_session_target_is_live(state, &target)? {
            return Err(DispatchError::NotFound("surface".to_string()));
        }
        return Ok(Some(target));
    }

    if !has_explicit_workspace {
        if let Some(target) = ingress.target.clone() {
            if hook_session_target_is_live(state, &target)? {
                insert_hook_session_target(params, &target)?;
            } else {
                ingress.target = None;
                return Err(DispatchError::NotFound("surface".to_string()));
            }
        } else if let Some(target) = unique_hook_session_target_from_cwd(state, params)? {
            insert_hook_session_target(params, &target)?;
            return Ok(Some(target));
        }
    }

    Ok(None)
}

fn unique_hook_session_target_from_cwd(
    state: &SocketAppState,
    params: &Value,
) -> Result<Option<HookSessionTarget>, DispatchError> {
    unique_hook_session_target_from_cwd_with(state, params, |path| fs::canonicalize(path).ok())
}

fn unique_hook_session_target_from_cwd_with(
    state: &SocketAppState,
    params: &Value,
    mut canonicalize: impl FnMut(&Path) -> Option<PathBuf>,
) -> Result<Option<HookSessionTarget>, DispatchError> {
    let Some(cwd) = optional_hook_session_cwd(params)? else {
        return Ok(None);
    };
    let surfaces = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .list_surfaces(None)
            .into_iter()
            .map(|surface| HookSessionCwdSurface {
                target: HookSessionTarget {
                    workspace_id: surface.workspace_id,
                    surface_id: surface.id,
                },
                cwd: surface.cwd,
            })
            .collect::<Vec<_>>()
    };
    let exact_matches = surfaces
        .iter()
        .filter_map(|surface| {
            let surface_cwd = canonicalize(&surface.cwd)?;
            (surface_cwd == cwd).then(|| surface.target.clone())
        })
        .collect::<Vec<_>>();
    if !exact_matches.is_empty() {
        return unique_cwd_hook_session_target(exact_matches);
    }
    let ancestor_matches = surfaces
        .into_iter()
        .filter_map(|surface| {
            let surface_cwd = canonicalize(&surface.cwd)?;
            cwd.starts_with(&surface_cwd).then_some(surface.target)
        })
        .collect::<Vec<_>>();
    unique_cwd_hook_session_target(ancestor_matches)
}

fn unique_cwd_hook_session_target(
    matches: Vec<HookSessionTarget>,
) -> Result<Option<HookSessionTarget>, DispatchError> {
    match matches.as_slice() {
        [target] => Ok(Some(target.clone())),
        [] => Err(DispatchError::NotFound(
            "hook_session_cwd target".to_string(),
        )),
        _ => Err(DispatchError::Conflict(
            "hook_session_cwd matches multiple live surfaces".to_string(),
        )),
    }
}

fn should_evict_hook_session_target_on_return(
    method: &str,
    params: &Value,
    event_name: Option<&str>,
) -> Result<bool, DispatchError> {
    if event_name != Some("session-end") || method != "metadata.clear_status" {
        return Ok(false);
    }
    let key = optional_non_blank_string_param(params, "key")?;
    Ok(key.is_none_or(|key| agent_kind_from_status_key(key).is_none()))
}

fn is_hook_targetable_method(method: &str) -> bool {
    matches!(
        method,
        "metadata.set_status"
            | "metadata.clear_status"
            | "metadata.set_progress"
            | "metadata.log"
            | "notification.create"
    )
}

fn hook_session_target_is_live(
    state: &SocketAppState,
    target: &HookSessionTarget,
) -> Result<bool, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(model
        .surface(&target.surface_id)
        .is_some_and(|surface| surface.workspace_id == target.workspace_id))
}

fn insert_hook_session_target(
    params: &mut Value,
    target: &HookSessionTarget,
) -> Result<(), DispatchError> {
    let Some(params) = params.as_object_mut() else {
        return Err(DispatchError::InvalidParam(
            "Invalid hook target params: expected object".to_string(),
        ));
    };
    params.insert(
        "workspace_id".to_string(),
        Value::String(target.workspace_id.clone()),
    );
    params.insert(
        "surface_id".to_string(),
        Value::String(target.surface_id.clone()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_core::WorkspaceModel;
    use forktty_terminal::HeadlessTerminalBackend;

    #[test]
    fn cwd_target_filesystem_resolution_runs_after_model_access_is_released() {
        let cwd = tempfile::tempdir().unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        model.lock().unwrap().create_workspace("main", cwd.path());
        let state = SocketAppState::new(
            Arc::clone(&model),
            Arc::new(HeadlessTerminalBackend::new()),
            "/bin/sh",
            cwd.path().join("forktty.sock"),
        )
        .with_notification_dispatch(false);
        let params = json!({"hook_session_cwd": cwd.path()});

        let target = unique_hook_session_target_from_cwd_with(&state, &params, |path| {
            assert!(
                model.try_lock().is_ok(),
                "filesystem resolution must not retain the workspace model lock"
            );
            fs::canonicalize(path).ok()
        })
        .unwrap()
        .expect("the sole surface cwd should resolve uniquely");

        assert_eq!(target.workspace_id, "workspace-1");
        assert_eq!(target.surface_id, "surface-1");
    }
}
