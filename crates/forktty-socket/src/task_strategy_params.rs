use crate::DispatchError;
use crate::{optional_bool_param, optional_non_blank_string_param, required_string};
use forktty_core::protocol_limits;
use serde_json::Value;

pub(crate) const MAX_TASK_STRATEGY_GOAL_BYTES: usize =
    protocol_limits::SOCKET_TASK_STRATEGY_GOAL_MAX_BYTES;

pub(crate) struct TaskStrategyPlanParams {
    pub(crate) goal: String,
    pub(crate) explicit_strategy: Option<String>,
    pub(crate) router_profile: Option<String>,
    pub(crate) task_class_hint: Option<String>,
    pub(crate) last_known_good: Option<Value>,
    pub(crate) harness_signals: Option<Value>,
    pub(crate) repo_dirty: Option<bool>,
    pub(crate) user_requested_parallelism: bool,
    pub(crate) user_requested_review: bool,
    pub(crate) likely_user_visible_change: Option<bool>,
}

pub(crate) fn task_strategy_plan_params(
    params: &Value,
) -> Result<TaskStrategyPlanParams, DispatchError> {
    let goal = task_strategy_goal_param(params)?;
    let explicit_strategy =
        optional_non_blank_string_param(params, "strategy")?.map(str::to_string);
    let router_profile =
        optional_non_blank_string_param(params, "router_profile")?.map(str::to_string);
    let task_kind = optional_non_blank_string_param(params, "task_kind")?;
    let task_class_hint_param = optional_non_blank_string_param(params, "task_class_hint")?;
    let task_class_hint = match (task_kind, task_class_hint_param) {
        (Some(task_kind), Some(task_class_hint)) if task_kind != task_class_hint => {
            return Err(DispatchError::InvalidParam(
                "task_kind and task_class_hint must match when both are provided".to_string(),
            ));
        }
        (Some(value), _) | (_, Some(value)) => Some(value.to_string()),
        (None, None) => None,
    };
    let last_known_good = match params.get("last_known_good") {
        None | Some(Value::Null) => None,
        Some(value @ Value::Object(_)) => Some(value.clone()),
        Some(_) => {
            return Err(DispatchError::InvalidParam(
                "last_known_good must be an object".to_string(),
            ));
        }
    };
    let harness_signals = match params.get("harness_signals") {
        None | Some(Value::Null) => None,
        Some(value @ Value::Object(_)) => Some(value.clone()),
        Some(_) => {
            return Err(DispatchError::InvalidParam(
                "harness_signals must be an object".to_string(),
            ));
        }
    };
    let repo_dirty = optional_bool_param(params, "repo_dirty")?;
    let user_requested_parallelism =
        optional_bool_param(params, "user_requested_parallelism")?.unwrap_or(false);
    let user_requested_review =
        optional_bool_param(params, "user_requested_review")?.unwrap_or(false);
    let likely_user_visible_change = optional_bool_param(params, "likely_user_visible_change")?;

    Ok(TaskStrategyPlanParams {
        goal,
        explicit_strategy,
        router_profile,
        task_class_hint,
        last_known_good,
        harness_signals,
        repo_dirty,
        user_requested_parallelism,
        user_requested_review,
        likely_user_visible_change,
    })
}

pub(crate) fn task_strategy_goal_param(params: &Value) -> Result<String, DispatchError> {
    let raw_goal = required_string(params, "goal")?;
    if raw_goal
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    {
        return Err(DispatchError::InvalidParam(
            "Invalid parameter goal: must not contain terminal control characters".to_string(),
        ));
    }
    let goal = raw_goal.trim();
    if goal.len() > MAX_TASK_STRATEGY_GOAL_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field: "goal",
            limit: MAX_TASK_STRATEGY_GOAL_BYTES,
            actual: goal.len(),
        });
    }
    Ok(goal.to_string())
}
