//! Task strategy routing for visible multi-harness agent work.
//!
//! This module is pure domain logic. It does not inspect the filesystem, launch
//! agents, create worktrees, or mutate workflow/team state.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    TinyAnswer,
    RepoInspection,
    FocusedBugfix,
    FeatureImplementation,
    ReviewOnly,
    ParallelResearch,
    ParallelExperiment,
    VerifyFixLoop,
    LongRunningTeamRun,
}

/// Parse a caller-normalized task kind hint.
///
/// This deliberately accepts a small set of stable category aliases instead of
/// trying to infer natural-language intent across every possible user language.
pub fn task_class_from_hint(value: &str) -> Option<TaskClass> {
    let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "tiny" | "answer" | "tiny_answer" => Some(TaskClass::TinyAnswer),
        "inspect" | "inspection" | "repo" | "repo_inspection" => Some(TaskClass::RepoInspection),
        "fix" | "bugfix" | "bug_fix" | "focused_bugfix" => Some(TaskClass::FocusedBugfix),
        "feature" | "implement" | "implementation" | "feature_implementation" => {
            Some(TaskClass::FeatureImplementation)
        }
        "review" | "read_only" | "readonly" | "review_only" => Some(TaskClass::ReviewOnly),
        "parallel" | "research" | "parallel_research" => Some(TaskClass::ParallelResearch),
        "experiment" | "parallel_experiment" => Some(TaskClass::ParallelExperiment),
        "verify" | "verification" | "verify_fix_loop" => Some(TaskClass::VerifyFixLoop),
        "team" | "team_run" | "long_running" | "long_running_team_run" => {
            Some(TaskClass::LongRunningTeamRun)
        }
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStrategy {
    Solo,
    SoloTracked,
    SoloWithVerifyLoop,
    ImplementerPlusReviewer,
    ParallelResearch,
    ParallelExperiment,
    TeamPipeline,
    ReviewOnly,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRouterProfile {
    #[default]
    Balanced,
    Fast,
    Conservative,
    Parallel,
    ReviewHeavy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessRole {
    Implementer,
    Reviewer,
    Researcher,
    Verifier,
    Synthesizer,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessHealth {
    Ready,
    Missing,
    Unauthenticated,
    Disabled,
    Error,
}

/// Structured cooldown cause. Different causes get different soft penalties:
/// an auth failure will not heal on its own, while a timeout usually will.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessCooldownKind {
    Quota,
    Auth,
    Crash,
    Timeout,
}

/// Parse a caller-provided cooldown kind. Accepts the canonical snake_case
/// category names only; unknown categories are the caller's error.
pub fn harness_cooldown_kind_from_str(value: &str) -> Option<HarnessCooldownKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "quota" => Some(HarnessCooldownKind::Quota),
        "auth" => Some(HarnessCooldownKind::Auth),
        "crash" => Some(HarnessCooldownKind::Crash),
        "timeout" => Some(HarnessCooldownKind::Timeout),
        _ => None,
    }
}

fn harness_cooldown_kind_id(kind: &HarnessCooldownKind) -> &'static str {
    match kind {
        HarnessCooldownKind::Quota => "quota",
        HarnessCooldownKind::Auth => "auth",
        HarnessCooldownKind::Crash => "crash",
        HarnessCooldownKind::Timeout => "timeout",
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessRoutingSignals {
    pub cooldown: bool,
    #[serde(default)]
    pub cooldown_kind: Option<HarnessCooldownKind>,
    pub cooldown_reason: Option<String>,
    pub locked_out: bool,
    pub lockout_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessCapability {
    pub id: String,
    pub installed: bool,
    pub authenticated: bool,
    pub supports_prompt_launch: bool,
    pub supports_resume: bool,
    pub supports_hooks: bool,
    pub supports_mcp: bool,
    pub supports_plan_mode: bool,
    pub supports_worktree_cwd: bool,
    pub max_parallel_sessions: Option<u32>,
    pub health: HarnessHealth,
    #[serde(default)]
    pub routing_signals: HarnessRoutingSignals,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessRegistry {
    pub harnesses: Vec<HarnessCapability>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyLayers {
    pub workflow: bool,
    pub team: bool,
    pub loop_metadata: bool,
    pub worktree: bool,
    pub feed: bool,
    pub mcp: bool,
    pub hooks: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStrategyApproval {
    StartRun,
    CreateWorktree,
    LaunchParallelWorkers,
    IncreaseRisk,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessAssignment {
    pub role: HarnessRole,
    pub harness_id: String,
    pub reason: String,
    #[serde(default)]
    pub score: i32,
    #[serde(default)]
    pub factors: Vec<TaskStrategyScoreFactor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyScoreFactor {
    pub name: String,
    pub points: i32,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyCandidateScore {
    pub strategy: TaskStrategy,
    pub score: i32,
    pub factors: Vec<TaskStrategyScoreFactor>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyLastKnownGood {
    pub strategy: Option<TaskStrategy>,
    pub harness_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyInput {
    pub goal: String,
    pub explicit_mode: Option<TaskStrategy>,
    pub router_profile: Option<TaskRouterProfile>,
    #[serde(default)]
    pub task_class_hint: Option<TaskClass>,
    #[serde(default)]
    pub last_known_good: Option<TaskStrategyLastKnownGood>,
    pub repo_dirty: bool,
    pub user_requested_parallelism: bool,
    pub user_requested_review: bool,
    pub likely_user_visible_change: bool,
    pub harness_registry: HarnessRegistry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyPlan {
    pub task_class: TaskClass,
    pub strategy: TaskStrategy,
    #[serde(default)]
    pub router_profile: TaskRouterProfile,
    pub layers: TaskStrategyLayers,
    pub assignments: Vec<HarnessAssignment>,
    pub approvals: Vec<TaskStrategyApproval>,
    #[serde(default)]
    pub candidate_scores: Vec<TaskStrategyCandidateScore>,
    pub reasons: Vec<String>,
    pub safety_notes: Vec<String>,
}

pub fn plan_task_strategy(input: TaskStrategyInput) -> Result<TaskStrategyPlan, String> {
    let goal = input.goal.trim();
    if goal.is_empty() {
        return Err("goal must not be empty".to_string());
    }

    let task_class = input
        .task_class_hint
        .clone()
        .unwrap_or_else(|| classify_task(goal, &input));
    let router_profile = input
        .router_profile
        .clone()
        .unwrap_or_else(|| infer_router_profile(goal, &task_class, &input));
    let candidate_scores = score_candidate_strategies(&task_class, &router_profile, &input);
    let top_scored_strategy = candidate_scores
        .first()
        .map(|candidate| candidate.strategy.clone())
        .expect("all_task_strategies() always yields at least one scored candidate");
    let strategy = input
        .explicit_mode
        .clone()
        .unwrap_or_else(|| top_scored_strategy.clone());
    let layers = effective_layers_for_strategy(&strategy, &input);

    let assignments = assignments_for_strategy(
        &strategy,
        &input.harness_registry,
        &layers,
        input.last_known_good.as_ref(),
    );
    if assignments.is_empty() {
        return Err("no ready harness can satisfy the selected strategy".to_string());
    }

    let mut approvals = vec![TaskStrategyApproval::StartRun];
    if layers.worktree {
        approvals.push(TaskStrategyApproval::CreateWorktree);
    }
    if requires_parallel_workers_approval(assignments.len()) {
        approvals.push(TaskStrategyApproval::LaunchParallelWorkers);
    }
    approvals.sort_by_key(approval_order);
    approvals.dedup();

    let mut reasons = vec![format!("classified task as {task_class:?}")];
    if layers.worktree && input.repo_dirty && input.likely_user_visible_change {
        reasons.push("dirty repo plus editing task requires worktree isolation".to_string());
    }
    if input.user_requested_parallelism {
        reasons.push("user requested parallelism or comparison".to_string());
    }
    if input.user_requested_review {
        reasons.push("user requested review coverage".to_string());
    }
    if input.task_class_hint.is_some() {
        reasons.push("caller-provided task kind guided classification".to_string());
    }
    if let Some(profile) = input.router_profile.as_ref() {
        reasons.push(format!("explicit router profile {profile:?}"));
    } else if router_profile != TaskRouterProfile::Balanced {
        reasons.push(format!("inferred router profile {router_profile:?}"));
    }
    if let Some(explicit_mode) = input.explicit_mode.as_ref() {
        if *explicit_mode == top_scored_strategy {
            reasons.push(format!(
                "explicit strategy override matches top-scored strategy {:?}",
                explicit_mode
            ));
        } else {
            reasons.push(format!(
                "explicit strategy override {:?} selected over top-scored strategy {:?}",
                explicit_mode, top_scored_strategy
            ));
        }
    } else {
        reasons.push(format!(
            "selected top-scored strategy {:?}",
            top_scored_strategy
        ));
    }

    Ok(TaskStrategyPlan {
        task_class,
        strategy,
        router_profile,
        layers,
        assignments,
        approvals,
        candidate_scores,
        reasons,
        safety_notes: vec![
            "planning is read-only".to_string(),
            "applying this strategy must keep all agent work visible in ForkTTY panes".to_string(),
            "push, merge, destructive commands, and out-of-scope edits require a later approval"
                .to_string(),
        ],
    })
}

fn approval_order(approval: &TaskStrategyApproval) -> u8 {
    match approval {
        TaskStrategyApproval::StartRun => 0,
        TaskStrategyApproval::CreateWorktree => 1,
        TaskStrategyApproval::LaunchParallelWorkers => 2,
        TaskStrategyApproval::IncreaseRisk => 3,
    }
}

fn requires_parallel_workers_approval(assignment_count: usize) -> bool {
    assignment_count > 1
}

fn score_candidate_strategies(
    task_class: &TaskClass,
    router_profile: &TaskRouterProfile,
    input: &TaskStrategyInput,
) -> Vec<TaskStrategyCandidateScore> {
    let mut candidates = all_task_strategies()
        .into_iter()
        .map(|strategy| score_strategy_candidate(strategy, task_class, router_profile, input))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| strategy_order(&left.strategy).cmp(&strategy_order(&right.strategy)))
    });
    candidates
}

fn all_task_strategies() -> Vec<TaskStrategy> {
    vec![
        TaskStrategy::Solo,
        TaskStrategy::SoloTracked,
        TaskStrategy::SoloWithVerifyLoop,
        TaskStrategy::ImplementerPlusReviewer,
        TaskStrategy::ParallelResearch,
        TaskStrategy::ParallelExperiment,
        TaskStrategy::TeamPipeline,
        TaskStrategy::ReviewOnly,
    ]
}

fn score_strategy_candidate(
    strategy: TaskStrategy,
    task_class: &TaskClass,
    router_profile: &TaskRouterProfile,
    input: &TaskStrategyInput,
) -> TaskStrategyCandidateScore {
    let mut factors = Vec::new();
    push_score_factor(
        &mut factors,
        "task_class_fit",
        task_class_fit_points(&strategy, task_class),
        format!("{strategy:?} fit for {task_class:?}"),
    );
    push_score_factor(
        &mut factors,
        "router_profile_fit",
        router_profile_fit_points(&strategy, router_profile, task_class),
        format!("{strategy:?} fit for router profile {router_profile:?}"),
    );
    if let Some(last_known_good) = input.last_known_good.as_ref() {
        if let Some(last_strategy) = last_known_good.strategy.as_ref() {
            let points = if *last_strategy == strategy { 12 } else { 0 };
            push_score_factor(
                &mut factors,
                "last_known_good_strategy",
                points,
                if points > 0 {
                    last_known_good_reason(
                        last_known_good,
                        "last successful strategy matched this candidate",
                    )
                } else {
                    format!("last successful strategy was {last_strategy:?}")
                },
            );
        }
    }

    if input.user_requested_parallelism {
        let points = if matches!(
            strategy,
            TaskStrategy::ParallelResearch
                | TaskStrategy::ParallelExperiment
                | TaskStrategy::TeamPipeline
        ) {
            40
        } else {
            -10
        };
        push_score_factor(
            &mut factors,
            "parallelism_request",
            points,
            "user asked for comparison or parallel lanes".to_string(),
        );
    } else if matches!(
        strategy,
        TaskStrategy::ParallelResearch
            | TaskStrategy::ParallelExperiment
            | TaskStrategy::TeamPipeline
    ) {
        push_score_factor(
            &mut factors,
            "parallelism_cost",
            -20,
            "parallel workers add coordination cost when not requested".to_string(),
        );
    }

    if input.user_requested_review {
        let points = if matches!(
            strategy,
            TaskStrategy::ImplementerPlusReviewer | TaskStrategy::ReviewOnly
        ) {
            35
        } else {
            0
        };
        push_score_factor(
            &mut factors,
            "review_request",
            points,
            "user asked for separate review coverage".to_string(),
        );
    } else if strategy == TaskStrategy::ImplementerPlusReviewer {
        push_score_factor(
            &mut factors,
            "review_coordination_cost",
            -15,
            "review worker adds overhead when review was not requested".to_string(),
        );
    }

    let layers = effective_layers_for_strategy(&strategy, input);
    if layers.loop_metadata
        && matches!(
            task_class,
            TaskClass::FocusedBugfix | TaskClass::FeatureImplementation | TaskClass::VerifyFixLoop
        )
    {
        push_score_factor(
            &mut factors,
            "verification_loop_fit",
            10,
            "editing and bugfix tasks benefit from a bounded verify loop".to_string(),
        );
    }
    if layers.team && !(input.user_requested_parallelism || input.user_requested_review) {
        push_score_factor(
            &mut factors,
            "team_coordination_cost",
            -15,
            "team state is reserved for review, parallelism, or long-running lanes".to_string(),
        );
    }
    if input.repo_dirty && input.likely_user_visible_change {
        push_score_factor(
            &mut factors,
            "dirty_editing_context",
            5,
            "dirty repo and editing task require extra isolation if selected".to_string(),
        );
    }

    let assignment_count = assignments_for_strategy(
        &strategy,
        &input.harness_registry,
        &layers,
        input.last_known_good.as_ref(),
    )
    .len();
    if assignment_count == 0 {
        push_score_factor(
            &mut factors,
            "harness_readiness",
            -1_000,
            "no ready harness can satisfy this strategy".to_string(),
        );
    } else {
        push_score_factor(
            &mut factors,
            "harness_readiness",
            20,
            format!("{assignment_count} visible harness role(s) can be assigned"),
        );
    }

    let score = factors.iter().map(|factor| factor.points).sum();
    TaskStrategyCandidateScore {
        strategy,
        score,
        factors,
    }
}

/// Profile weights may intentionally outrank `task_class_fit_points` (for
/// example `Parallel` grants up to 110 while class fit tops out at 95): an
/// explicit or inferred profile is a stronger routing signal than the
/// keyword-derived task class. The corpus test
/// `goal_routing_corpus_pins_classifier_and_weights` pins the combined effect
/// of these weights; extend it when retuning any constant here.
fn router_profile_fit_points(
    strategy: &TaskStrategy,
    router_profile: &TaskRouterProfile,
    task_class: &TaskClass,
) -> i32 {
    match router_profile {
        TaskRouterProfile::Balanced => 0,
        TaskRouterProfile::Fast => match strategy {
            TaskStrategy::Solo | TaskStrategy::SoloTracked => 15,
            TaskStrategy::SoloWithVerifyLoop => 10,
            TaskStrategy::ReviewOnly => 5,
            TaskStrategy::ImplementerPlusReviewer => -25,
            TaskStrategy::ParallelResearch
            | TaskStrategy::ParallelExperiment
            | TaskStrategy::TeamPipeline => -35,
        },
        TaskRouterProfile::Conservative => match strategy {
            TaskStrategy::ImplementerPlusReviewer => 70,
            TaskStrategy::SoloWithVerifyLoop => 20,
            TaskStrategy::ReviewOnly => 25,
            TaskStrategy::TeamPipeline if *task_class == TaskClass::LongRunningTeamRun => 40,
            TaskStrategy::TeamPipeline => 10,
            TaskStrategy::Solo | TaskStrategy::SoloTracked => -5,
            TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment => -10,
        },
        TaskRouterProfile::Parallel => match strategy {
            TaskStrategy::ParallelResearch => 110,
            TaskStrategy::ParallelExperiment => 95,
            TaskStrategy::TeamPipeline => 75,
            TaskStrategy::ImplementerPlusReviewer => 10,
            _ => -25,
        },
        TaskRouterProfile::ReviewHeavy => match strategy {
            TaskStrategy::ImplementerPlusReviewer => 75,
            TaskStrategy::ReviewOnly => 60,
            TaskStrategy::SoloWithVerifyLoop => 5,
            TaskStrategy::TeamPipeline => 20,
            _ => -10,
        },
    }
}

fn push_score_factor(
    factors: &mut Vec<TaskStrategyScoreFactor>,
    name: &str,
    points: i32,
    reason: String,
) {
    factors.push(TaskStrategyScoreFactor {
        name: name.to_string(),
        points,
        reason,
    });
}

fn task_class_fit_points(strategy: &TaskStrategy, task_class: &TaskClass) -> i32 {
    match task_class {
        TaskClass::TinyAnswer => match strategy {
            TaskStrategy::Solo => 90,
            TaskStrategy::SoloTracked => 55,
            _ => 10,
        },
        TaskClass::RepoInspection => match strategy {
            TaskStrategy::SoloTracked => 85,
            TaskStrategy::Solo => 60,
            TaskStrategy::ReviewOnly => 45,
            TaskStrategy::SoloWithVerifyLoop => 35,
            _ => 10,
        },
        TaskClass::FocusedBugfix | TaskClass::VerifyFixLoop => match strategy {
            TaskStrategy::SoloWithVerifyLoop => 90,
            TaskStrategy::SoloTracked => 50,
            TaskStrategy::ImplementerPlusReviewer => 50,
            TaskStrategy::ReviewOnly => 35,
            _ => 15,
        },
        TaskClass::FeatureImplementation => match strategy {
            TaskStrategy::SoloWithVerifyLoop => 80,
            TaskStrategy::ImplementerPlusReviewer => 70,
            TaskStrategy::SoloTracked => 45,
            TaskStrategy::ReviewOnly => 35,
            _ => 20,
        },
        TaskClass::ReviewOnly => match strategy {
            TaskStrategy::ReviewOnly => 95,
            TaskStrategy::SoloTracked => 45,
            TaskStrategy::ImplementerPlusReviewer => 40,
            _ => 15,
        },
        TaskClass::ParallelResearch => match strategy {
            TaskStrategy::ParallelResearch => 95,
            TaskStrategy::TeamPipeline => 60,
            TaskStrategy::ParallelExperiment => 55,
            TaskStrategy::SoloTracked => 30,
            _ => 10,
        },
        TaskClass::ParallelExperiment => match strategy {
            TaskStrategy::ParallelExperiment => 95,
            TaskStrategy::ParallelResearch => 65,
            TaskStrategy::TeamPipeline => 60,
            _ => 10,
        },
        TaskClass::LongRunningTeamRun => match strategy {
            TaskStrategy::TeamPipeline => 95,
            TaskStrategy::ImplementerPlusReviewer => 65,
            TaskStrategy::ParallelResearch => 50,
            _ => 10,
        },
    }
}

fn strategy_order(strategy: &TaskStrategy) -> u8 {
    match strategy {
        TaskStrategy::Solo => 0,
        TaskStrategy::SoloTracked => 1,
        TaskStrategy::SoloWithVerifyLoop => 2,
        TaskStrategy::ImplementerPlusReviewer => 3,
        TaskStrategy::ParallelResearch => 4,
        TaskStrategy::ParallelExperiment => 5,
        TaskStrategy::TeamPipeline => 6,
        TaskStrategy::ReviewOnly => 7,
    }
}

fn classify_task(goal: &str, input: &TaskStrategyInput) -> TaskClass {
    let lower = goal.to_lowercase();
    if input.user_requested_parallelism || contains_parallel_research_intent(&lower) {
        // Parallel lanes over an editing goal are competing experiment
        // attempts, not read-only research.
        if contains_implementation_intent(&lower) || contains_bugfix_intent(&lower) {
            return TaskClass::ParallelExperiment;
        }
        return TaskClass::ParallelResearch;
    }
    if is_review_primary_goal(goal) {
        return TaskClass::ReviewOnly;
    }
    if contains_implementation_intent(&lower) {
        return TaskClass::FeatureImplementation;
    }
    if contains_bugfix_intent(&lower) {
        return TaskClass::FocusedBugfix;
    }
    if contains_comparison_research_intent(&lower) {
        return TaskClass::ParallelResearch;
    }
    if contains_token(
        &lower,
        &["test", "tests", "testing", "verify", "verifies", "verified"],
    ) || contains_token_prefix(&lower, &["verif"])
    {
        return TaskClass::FocusedBugfix;
    }
    if contains_token(&lower, &["review", "reviews"]) {
        return TaskClass::ReviewOnly;
    }
    TaskClass::RepoInspection
}

fn contains_bugfix_intent(lower: &str) -> bool {
    // Exact tokens plus explicit inflection prefixes: a bare "fix"/"bug" prefix
    // would also match unrelated words such as "fixtures".
    contains_token(lower, &["fix", "bug", "bugs"])
        || contains_token_prefix(lower, &["fixe", "fixi", "bugfix"])
}

fn contains_parallel_research_intent(lower: &str) -> bool {
    contains_token_prefix(lower, &["parallel"])
}

fn contains_comparison_research_intent(lower: &str) -> bool {
    contains_token(lower, &["compare", "alternatives", "approaches"])
        || contains_token_prefix(lower, &["alternative"])
}

fn contains_implementation_intent(lower: &str) -> bool {
    contains_token(
        lower,
        &[
            "add", "adds", "added", "adding", "build", "builds", "built", "building",
        ],
    ) || contains_token_prefix(lower, &["implement"])
}

/// Returns true when the goal reads as a review-first request (the first
/// meaningful token is a review verb), as opposed to an editing goal that
/// merely mentions a review.
fn is_review_primary_goal(goal: &str) -> bool {
    let lower = goal.to_lowercase();
    let skip = [
        "the",
        "a",
        "an",
        "please",
        "quickly",
        "carefully",
        "thoroughly",
    ];
    let mut tokens = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty() && !skip.contains(token));
    let first = tokens.next();
    if first == Some("read") && tokens.next() == Some("only") {
        return true;
    }
    matches!(first, Some("review" | "reviews"))
}

fn infer_router_profile(
    goal: &str,
    task_class: &TaskClass,
    input: &TaskStrategyInput,
) -> TaskRouterProfile {
    if input.user_requested_parallelism
        || matches!(
            task_class,
            TaskClass::ParallelResearch | TaskClass::ParallelExperiment
        )
    {
        return TaskRouterProfile::Parallel;
    }
    if input.user_requested_review || matches!(task_class, TaskClass::ReviewOnly) {
        return TaskRouterProfile::ReviewHeavy;
    }

    let lower = goal.to_lowercase();
    if contains_token_prefix(
        &lower,
        &["quick", "fast", "asap", "urgent", "small", "simple"],
    ) {
        return TaskRouterProfile::Fast;
    }
    if contains_token_prefix(
        &lower,
        &[
            "safe",
            "safely",
            "careful",
            "conservative",
            "risk",
            "thorough",
        ],
    ) {
        return TaskRouterProfile::Conservative;
    }

    TaskRouterProfile::Balanced
}

fn contains_token_prefix(value: &str, prefixes: &[&str]) -> bool {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| prefixes.iter().any(|prefix| token.starts_with(prefix)))
}

fn contains_token(value: &str, tokens: &[&str]) -> bool {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| tokens.contains(&token))
}

pub fn phase_one_classifier_reachable_classes() -> &'static [TaskClass] {
    &[
        TaskClass::RepoInspection,
        TaskClass::FocusedBugfix,
        TaskClass::FeatureImplementation,
        TaskClass::ReviewOnly,
        TaskClass::ParallelResearch,
        TaskClass::ParallelExperiment,
    ]
}

fn layers_for_strategy(strategy: &TaskStrategy) -> TaskStrategyLayers {
    match strategy {
        TaskStrategy::Solo => TaskStrategyLayers {
            mcp: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::SoloTracked => TaskStrategyLayers {
            workflow: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::SoloWithVerifyLoop => TaskStrategyLayers {
            workflow: true,
            loop_metadata: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::ImplementerPlusReviewer => TaskStrategyLayers {
            workflow: true,
            team: true,
            loop_metadata: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::ParallelResearch
        | TaskStrategy::ParallelExperiment
        | TaskStrategy::TeamPipeline => TaskStrategyLayers {
            workflow: true,
            team: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::ReviewOnly => TaskStrategyLayers {
            workflow: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
    }
}

fn effective_layers_for_strategy(
    strategy: &TaskStrategy,
    input: &TaskStrategyInput,
) -> TaskStrategyLayers {
    let mut layers = layers_for_strategy(strategy);
    if !matches!(strategy, TaskStrategy::ReviewOnly)
        && input.repo_dirty
        && input.likely_user_visible_change
    {
        layers.worktree = true;
    }
    layers
}

fn assignments_for_strategy(
    strategy: &TaskStrategy,
    registry: &HarnessRegistry,
    layers: &TaskStrategyLayers,
    last_known_good: Option<&TaskStrategyLastKnownGood>,
) -> Vec<HarnessAssignment> {
    let ready = registry
        .harnesses
        .iter()
        .enumerate()
        .filter(|harness| harness_available_for_routing(harness.1))
        .collect::<Vec<_>>();
    if ready.is_empty() {
        return self_assignments_for_strategy(strategy);
    }
    let primary_role = match strategy {
        TaskStrategy::ReviewOnly => HarnessRole::Reviewer,
        TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment => {
            HarnessRole::Researcher
        }
        _ => HarnessRole::Implementer,
    };
    let Some(primary) =
        select_harness_assignment(primary_role, &ready, layers, last_known_good, &[])
    else {
        return Vec::new();
    };

    let mut assignments = vec![primary.clone()];

    if matches!(
        strategy,
        TaskStrategy::ImplementerPlusReviewer | TaskStrategy::TeamPipeline
    ) {
        let excluded = [primary.harness_id.clone()];
        let reviewer = select_harness_assignment_with_capacity(
            HarnessRole::Reviewer,
            &ready,
            layers,
            last_known_good,
            &excluded,
            &assignments,
        )
        .or_else(|| {
            select_harness_assignment_with_capacity(
                HarnessRole::Reviewer,
                &ready,
                layers,
                last_known_good,
                &[],
                &assignments,
            )
        });
        if let Some(mut reviewer) = reviewer {
            if reviewer.harness_id == primary.harness_id {
                reviewer.reason =
                    "highest-scored ready harness for reviewer; same harness reused because no separate ready harness was available".to_string();
            }
            assignments.push(reviewer);
        } else {
            return Vec::new();
        }
    }

    if matches!(
        strategy,
        TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment
    ) {
        let used = assignments
            .iter()
            .map(|assignment| assignment.harness_id.clone())
            .collect::<Vec<_>>();
        let mut additional_researchers =
            scored_harness_assignments(HarnessRole::Researcher, &ready, layers, last_known_good)
                .into_iter()
                .filter(|assignment| !used.contains(&assignment.harness_id))
                .filter(|assignment| {
                    harness_has_remaining_assignment_capacity(
                        &ready,
                        &assignment.harness_id,
                        &assignments,
                    )
                })
                .take(2)
                .collect::<Vec<_>>();
        for assignment in &mut additional_researchers {
            assignment.reason =
                "next highest-scored ready harness for parallel context isolation".to_string();
        }
        if assignments.len() + additional_researchers.len() < 2 {
            let mut existing_assignments = assignments.clone();
            existing_assignments.extend(additional_researchers.iter().cloned());
            let scored_researchers = scored_harness_assignments(
                HarnessRole::Researcher,
                &ready,
                layers,
                last_known_good,
            );
            for assignment in scored_researchers {
                if additional_researchers.len() >= 2 {
                    break;
                }
                if !harness_has_remaining_assignment_capacity(
                    &ready,
                    &assignment.harness_id,
                    &existing_assignments,
                ) {
                    continue;
                }
                let mut assignment = assignment;
                assignment.reason =
                    "highest-scored ready harness reused for parallel research because declared session capacity allows another lane"
                        .to_string();
                existing_assignments.push(assignment.clone());
                additional_researchers.push(assignment);
                if existing_assignments.len() >= 2 {
                    break;
                }
            }
        }
        assignments.extend(additional_researchers);
        if assignments.len() < 2 {
            return Vec::new();
        }
    }

    assignments
}

fn self_assignments_for_strategy(strategy: &TaskStrategy) -> Vec<HarnessAssignment> {
    let role = match strategy {
        TaskStrategy::Solo | TaskStrategy::SoloTracked | TaskStrategy::SoloWithVerifyLoop => {
            HarnessRole::Implementer
        }
        TaskStrategy::ReviewOnly => HarnessRole::Reviewer,
        TaskStrategy::ImplementerPlusReviewer
        | TaskStrategy::ParallelResearch
        | TaskStrategy::ParallelExperiment
        | TaskStrategy::TeamPipeline => return Vec::new(),
    };
    vec![HarnessAssignment {
        role,
        harness_id: "self".to_string(),
        reason:
            "no ready external harness was available; route stays with the current orchestrator"
                .to_string(),
        score: 1,
        factors: vec![TaskStrategyScoreFactor {
            name: "self_fallback".to_string(),
            points: 1,
            reason: "solo/read-only strategy can be handled by the current orchestrator"
                .to_string(),
        }],
    }]
}

/// Rank the ready harnesses in `registry` for one role, best first, using the
/// same scorer as plan-time assignment. Callers use this after a launch
/// failure to suggest the next-best harness without re-planning; it never
/// launches anything.
pub fn ranked_harnesses_for_role(
    role: &HarnessRole,
    registry: &HarnessRegistry,
    layers: &TaskStrategyLayers,
) -> Vec<HarnessAssignment> {
    let ready = registry
        .harnesses
        .iter()
        .enumerate()
        .filter(|(_, harness)| harness_available_for_routing(harness))
        .collect::<Vec<_>>();
    scored_harness_assignments(role.clone(), &ready, layers, None)
}

fn select_harness_assignment(
    role: HarnessRole,
    ready: &[(usize, &HarnessCapability)],
    layers: &TaskStrategyLayers,
    last_known_good: Option<&TaskStrategyLastKnownGood>,
    excluded_harness_ids: &[String],
) -> Option<HarnessAssignment> {
    scored_harness_assignments(role, ready, layers, last_known_good)
        .into_iter()
        .find(|assignment| !excluded_harness_ids.contains(&assignment.harness_id))
}

fn select_harness_assignment_with_capacity(
    role: HarnessRole,
    ready: &[(usize, &HarnessCapability)],
    layers: &TaskStrategyLayers,
    last_known_good: Option<&TaskStrategyLastKnownGood>,
    excluded_harness_ids: &[String],
    existing_assignments: &[HarnessAssignment],
) -> Option<HarnessAssignment> {
    scored_harness_assignments(role, ready, layers, last_known_good)
        .into_iter()
        .find(|assignment| {
            !excluded_harness_ids.contains(&assignment.harness_id)
                && harness_has_remaining_assignment_capacity(
                    ready,
                    &assignment.harness_id,
                    existing_assignments,
                )
        })
}

fn harness_has_remaining_assignment_capacity(
    ready: &[(usize, &HarnessCapability)],
    harness_id: &str,
    existing_assignments: &[HarnessAssignment],
) -> bool {
    let capacity = ready
        .iter()
        .find(|(_, harness)| harness.id == harness_id)
        .and_then(|(_, harness)| harness.max_parallel_sessions)
        .unwrap_or(1);
    let used = existing_assignments
        .iter()
        .filter(|assignment| assignment.harness_id == harness_id)
        .count() as u32;
    used < capacity
}

fn harness_available_for_routing(harness: &HarnessCapability) -> bool {
    harness.installed
        && harness.authenticated
        && harness.supports_prompt_launch
        && harness.health == HarnessHealth::Ready
        && !harness.routing_signals.locked_out
}

fn scored_harness_assignments(
    role: HarnessRole,
    ready: &[(usize, &HarnessCapability)],
    layers: &TaskStrategyLayers,
    last_known_good: Option<&TaskStrategyLastKnownGood>,
) -> Vec<HarnessAssignment> {
    let mut assignments = ready
        .iter()
        .map(|(order, harness)| {
            let (score, factors) = score_harness_for_role(&role, harness, layers, last_known_good);
            (
                *order,
                HarnessAssignment {
                    role: role.clone(),
                    harness_id: harness.id.clone(),
                    reason: format!(
                        "highest-scored ready harness for {}",
                        harness_role_id(&role)
                    ),
                    score,
                    factors,
                },
            )
        })
        .collect::<Vec<_>>();
    assignments.sort_by(|left, right| {
        right
            .1
            .score
            .cmp(&left.1.score)
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.harness_id.cmp(&right.1.harness_id))
    });
    assignments
        .into_iter()
        .map(|(_, assignment)| assignment)
        .collect()
}

fn score_harness_for_role(
    role: &HarnessRole,
    harness: &HarnessCapability,
    layers: &TaskStrategyLayers,
    last_known_good: Option<&TaskStrategyLastKnownGood>,
) -> (i32, Vec<TaskStrategyScoreFactor>) {
    let mut factors = Vec::new();
    push_score_factor(
        &mut factors,
        "harness_readiness",
        50,
        "harness is installed, authenticated, ready, and supports prompt launch".to_string(),
    );
    push_score_factor(
        &mut factors,
        "task_mode_lockout",
        0,
        "harness has no active task or mode lockout".to_string(),
    );
    let cooldown_points = if harness.routing_signals.cooldown {
        // Auth failures do not heal without user action, crashes are worse
        // than capacity pressure, and timeouts are usually transient.
        match harness.routing_signals.cooldown_kind.as_ref() {
            Some(HarnessCooldownKind::Auth) => -60,
            Some(HarnessCooldownKind::Crash) => -45,
            Some(HarnessCooldownKind::Quota) | None => -35,
            Some(HarnessCooldownKind::Timeout) => -25,
        }
    } else {
        0
    };
    push_score_factor(
        &mut factors,
        "session_cooldown",
        cooldown_points,
        if harness.routing_signals.cooldown {
            let kind_label = harness
                .routing_signals
                .cooldown_kind
                .as_ref()
                .map(harness_cooldown_kind_id)
                .unwrap_or("session");
            harness
                .routing_signals
                .cooldown_reason
                .as_deref()
                .map(|reason| format!("harness is on {kind_label} cooldown: {reason}"))
                .unwrap_or_else(|| {
                    format!("harness is on {kind_label} cooldown after a recent runtime signal")
                })
        } else {
            "harness has no active session cooldown".to_string()
        },
    );
    if let Some(last_known_good) = last_known_good {
        if let Some(harness_id) = last_known_good.harness_id.as_deref() {
            let points = if harness_id == harness.id { 12 } else { 0 };
            push_score_factor(
                &mut factors,
                "last_known_good_harness",
                points,
                if points > 0 {
                    last_known_good_reason(
                        last_known_good,
                        "last successful harness matched this candidate",
                    )
                } else {
                    format!("last successful harness was {harness_id}")
                },
            );
        }
    }

    match role {
        HarnessRole::Reviewer => push_score_factor(
            &mut factors,
            "role_capability_fit",
            if harness.supports_plan_mode { 25 } else { 0 },
            if harness.supports_plan_mode {
                "review role benefits from plan-mode support".to_string()
            } else {
                "review role has no plan-mode support signal".to_string()
            },
        ),
        HarnessRole::Synthesizer => push_score_factor(
            &mut factors,
            "role_capability_fit",
            if harness.supports_plan_mode { 15 } else { 0 },
            if harness.supports_plan_mode {
                "synthesizer role benefits from plan-mode support".to_string()
            } else {
                "synthesizer role has no plan-mode support signal".to_string()
            },
        ),
        HarnessRole::Researcher => push_score_factor(
            &mut factors,
            "role_capability_fit",
            if harness.max_parallel_sessions.unwrap_or(1) > 1 {
                10
            } else {
                0
            },
            "research role benefits from available parallel session capacity".to_string(),
        ),
        HarnessRole::Implementer | HarnessRole::Verifier => push_score_factor(
            &mut factors,
            "role_capability_fit",
            0,
            "role has no specialized harness capability preference yet".to_string(),
        ),
    }

    if layers.worktree {
        push_score_factor(
            &mut factors,
            "worktree_cwd_fit",
            if harness.supports_worktree_cwd {
                30
            } else {
                -25
            },
            if harness.supports_worktree_cwd {
                "selected strategy requires worktree isolation and harness supports cwd launch"
                    .to_string()
            } else {
                "selected strategy requires worktree isolation but harness lacks cwd launch support"
                    .to_string()
            },
        );
    }

    push_score_factor(
        &mut factors,
        "mcp_support",
        if layers.mcp && harness.supports_mcp {
            5
        } else {
            0
        },
        "MCP support improves controlled task routing and inspection".to_string(),
    );
    push_score_factor(
        &mut factors,
        "hook_support",
        if layers.hooks && harness.supports_hooks {
            5
        } else {
            0
        },
        "hook support improves visible lifecycle/status evidence".to_string(),
    );
    push_score_factor(
        &mut factors,
        "resume_support",
        if harness.supports_resume { 5 } else { 0 },
        "resume support improves recoverability after interruption".to_string(),
    );

    let score = factors.iter().map(|factor| factor.points).sum();
    (score, factors)
}

fn last_known_good_reason(last_known_good: &TaskStrategyLastKnownGood, fallback: &str) -> String {
    last_known_good
        .reason
        .as_deref()
        .filter(|reason| !reason.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn harness_role_id(role: &HarnessRole) -> &'static str {
    match role {
        HarnessRole::Implementer => "implementer",
        HarnessRole::Reviewer => "reviewer",
        HarnessRole::Researcher => "researcher",
        HarnessRole::Verifier => "verifier",
        HarnessRole::Synthesizer => "synthesizer",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> HarnessRegistry {
        HarnessRegistry {
            harnesses: vec![
                HarnessCapability {
                    id: "codex".to_string(),
                    installed: true,
                    authenticated: true,
                    supports_prompt_launch: true,
                    supports_resume: true,
                    supports_hooks: true,
                    supports_mcp: true,
                    supports_plan_mode: false,
                    supports_worktree_cwd: true,
                    max_parallel_sessions: Some(4),
                    health: HarnessHealth::Ready,
                    routing_signals: HarnessRoutingSignals::default(),
                },
                HarnessCapability {
                    id: "claude".to_string(),
                    installed: true,
                    authenticated: true,
                    supports_prompt_launch: true,
                    supports_resume: true,
                    supports_hooks: true,
                    supports_mcp: true,
                    supports_plan_mode: true,
                    supports_worktree_cwd: true,
                    max_parallel_sessions: Some(4),
                    health: HarnessHealth::Ready,
                    routing_signals: HarnessRoutingSignals::default(),
                },
            ],
        }
    }

    fn harness(
        id: &str,
        supports_plan_mode: bool,
        supports_worktree_cwd: bool,
    ) -> HarnessCapability {
        HarnessCapability {
            id: id.to_string(),
            installed: true,
            authenticated: true,
            supports_prompt_launch: true,
            supports_resume: true,
            supports_hooks: true,
            supports_mcp: true,
            supports_plan_mode,
            supports_worktree_cwd,
            max_parallel_sessions: Some(4),
            health: HarnessHealth::Ready,
            routing_signals: HarnessRoutingSignals::default(),
        }
    }

    fn harness_with_signals(id: &str, signals: HarnessRoutingSignals) -> HarnessCapability {
        HarnessCapability {
            routing_signals: signals,
            ..harness(id, false, true)
        }
    }

    fn plan_for_goal(goal: &str, likely_user_visible_change: bool) -> TaskStrategyPlan {
        plan_task_strategy(TaskStrategyInput {
            goal: goal.to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change,
            harness_registry: caps(),
        })
        .unwrap()
    }

    /// Table-driven corpus pinning goal → (class, profile, strategy) as one
    /// unit. Any retuning of classifier keywords or score constants must keep
    /// this table passing (or update it deliberately in the same change).
    #[test]
    fn goal_routing_corpus_pins_classifier_and_weights() {
        struct Case {
            goal: &'static str,
            editing: bool,
            class: TaskClass,
            profile: TaskRouterProfile,
            strategy: TaskStrategy,
        }
        let cases = [
            Case {
                goal: "Fix the flaky socket test and rerun it",
                editing: true,
                class: TaskClass::FocusedBugfix,
                profile: TaskRouterProfile::Balanced,
                strategy: TaskStrategy::SoloWithVerifyLoop,
            },
            Case {
                goal: "Quickly fix the startup bug",
                editing: true,
                class: TaskClass::FocusedBugfix,
                profile: TaskRouterProfile::Fast,
                strategy: TaskStrategy::SoloWithVerifyLoop,
            },
            Case {
                goal: "Carefully fix the config quarantine logic",
                editing: true,
                class: TaskClass::FocusedBugfix,
                profile: TaskRouterProfile::Conservative,
                strategy: TaskStrategy::SoloWithVerifyLoop,
            },
            Case {
                goal: "Implement retry logic for the socket client",
                editing: true,
                class: TaskClass::FeatureImplementation,
                profile: TaskRouterProfile::Balanced,
                strategy: TaskStrategy::SoloWithVerifyLoop,
            },
            Case {
                goal: "Add a configuration flag to the CLI",
                editing: true,
                class: TaskClass::FeatureImplementation,
                profile: TaskRouterProfile::Balanced,
                strategy: TaskStrategy::SoloWithVerifyLoop,
            },
            Case {
                goal: "Review the worktree merge changes",
                editing: false,
                class: TaskClass::ReviewOnly,
                profile: TaskRouterProfile::ReviewHeavy,
                strategy: TaskStrategy::ReviewOnly,
            },
            Case {
                goal: "Please review the worktree layer changes",
                editing: false,
                class: TaskClass::ReviewOnly,
                profile: TaskRouterProfile::ReviewHeavy,
                strategy: TaskStrategy::ReviewOnly,
            },
            Case {
                goal: "Compare caching approaches for the renderer",
                editing: false,
                class: TaskClass::ParallelResearch,
                profile: TaskRouterProfile::Parallel,
                strategy: TaskStrategy::ParallelResearch,
            },
            // Regression: "fixtures" must not prefix-match "fix" into a bugfix.
            Case {
                goal: "Summarize the fixtures directory layout",
                editing: false,
                class: TaskClass::RepoInspection,
                profile: TaskRouterProfile::Balanced,
                strategy: TaskStrategy::SoloTracked,
            },
        ];

        for case in cases {
            let plan = plan_for_goal(case.goal, case.editing);
            assert_eq!(plan.task_class, case.class, "class for {:?}", case.goal);
            assert_eq!(
                plan.router_profile, case.profile,
                "profile for {:?}",
                case.goal
            );
            assert_eq!(plan.strategy, case.strategy, "strategy for {:?}", case.goal);
            assert_eq!(
                plan.candidate_scores[0].strategy, plan.strategy,
                "top candidate for {:?}",
                case.goal
            );
        }
    }

    #[test]
    fn task_class_hint_routes_non_english_goals_without_language_keywords() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Bitte diese Änderung umsetzen".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: Some(TaskClass::FeatureImplementation),
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry::default(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::FeatureImplementation);
        assert_eq!(plan.strategy, TaskStrategy::SoloWithVerifyLoop);
        assert!(plan
            .reasons
            .iter()
            .any(|reason| reason.contains("caller-provided task kind")));
    }

    #[test]
    fn task_class_hint_parser_accepts_stable_category_aliases() {
        for (hint, task_class) in [
            ("bugfix", TaskClass::FocusedBugfix),
            ("bug-fix", TaskClass::FocusedBugfix),
            ("feature", TaskClass::FeatureImplementation),
            ("review", TaskClass::ReviewOnly),
            ("research", TaskClass::ParallelResearch),
            ("parallel", TaskClass::ParallelResearch),
            ("verify", TaskClass::VerifyFixLoop),
            ("team", TaskClass::LongRunningTeamRun),
        ] {
            assert_eq!(task_class_from_hint(hint), Some(task_class), "{hint}");
        }
        assert_eq!(task_class_from_hint("correggi"), None);
    }

    #[test]
    fn focused_bugfix_routes_to_verify_loop_without_team() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the browser feature test failure and run the relevant tests".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::FocusedBugfix);
        assert_eq!(plan.strategy, TaskStrategy::SoloWithVerifyLoop);
        assert!(plan.layers.workflow);
        assert!(plan.layers.loop_metadata);
        assert!(!plan.layers.team);
        assert_eq!(plan.assignments[0].role, HarnessRole::Implementer);
        assert_eq!(plan.approvals, vec![TaskStrategyApproval::StartRun]);
    }

    #[test]
    fn strategy_plan_exposes_ranked_candidate_scores() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the browser feature test failure and run the relevant tests".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert!(!plan.candidate_scores.is_empty());
        assert_eq!(plan.candidate_scores[0].strategy, plan.strategy);
        assert_eq!(
            plan.candidate_scores[0].strategy,
            TaskStrategy::SoloWithVerifyLoop
        );
        assert!(plan.candidate_scores.windows(2).all(|pair| {
            pair[0].score > pair[1].score
                || pair[0].score == pair[1].score
                    && strategy_order(&pair[0].strategy) <= strategy_order(&pair[1].strategy)
        }));
        assert!(plan.candidate_scores[0]
            .factors
            .iter()
            .any(|factor| factor.name == "task_class_fit" && factor.points > 0));
        assert!(plan
            .reasons
            .iter()
            .any(|reason| reason.contains("selected top-scored strategy")));
    }

    #[test]
    fn requested_review_changes_top_scored_strategy() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: true,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.strategy, TaskStrategy::ImplementerPlusReviewer);
        assert_eq!(
            plan.candidate_scores[0].strategy,
            TaskStrategy::ImplementerPlusReviewer
        );
        assert!(plan.candidate_scores[0]
            .factors
            .iter()
            .any(|factor| factor.name == "review_request" && factor.points > 0));
        assert!(!plan.layers.worktree);
        assert!(!plan
            .approvals
            .contains(&TaskStrategyApproval::CreateWorktree));
    }

    #[test]
    fn fixing_review_feedback_routes_as_bugfix_not_review_only() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Address review feedback by fixing task strategy apply bug and run tests"
                .to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::FocusedBugfix);
        assert_eq!(plan.strategy, TaskStrategy::SoloWithVerifyLoop);
    }

    #[test]
    fn review_primary_goal_with_fix_terms_stays_review_only() {
        let review_bug_fix = plan_task_strategy(TaskStrategyInput {
            goal: "Review the bug fix in the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();
        let fix_with_approaches = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the bug using one of the approaches discussed".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(review_bug_fix.task_class, TaskClass::ReviewOnly);
        assert_eq!(review_bug_fix.strategy, TaskStrategy::ReviewOnly);
        assert_eq!(fix_with_approaches.task_class, TaskClass::FocusedBugfix);
        assert_eq!(
            fix_with_approaches.strategy,
            TaskStrategy::SoloWithVerifyLoop
        );
    }

    #[test]
    fn review_primary_goal_with_implementation_terms_stays_review_only() {
        for goal in [
            "Review the change that adds retry logic",
            "Review the PR that implements the settings dialog",
            "Review the current findings and independently verify the evidence",
        ] {
            let plan = plan_task_strategy(TaskStrategyInput {
                goal: goal.to_string(),
                explicit_mode: None,
                router_profile: None,
                task_class_hint: None,
                last_known_good: None,
                repo_dirty: false,
                user_requested_parallelism: false,
                user_requested_review: false,
                likely_user_visible_change: false,
                harness_registry: caps(),
            })
            .unwrap();

            assert_eq!(plan.task_class, TaskClass::ReviewOnly, "{goal}");
            assert_eq!(plan.strategy, TaskStrategy::ReviewOnly, "{goal}");
        }
    }

    #[test]
    fn review_only_does_not_force_dirty_repo_worktree_isolation() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Review the bug fix in the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: true,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::ReviewOnly);
        assert_eq!(plan.strategy, TaskStrategy::ReviewOnly);
        assert!(!plan.layers.worktree);
        assert!(!plan
            .approvals
            .contains(&TaskStrategyApproval::CreateWorktree));
    }

    #[test]
    fn reviewing_test_coverage_stays_review_only() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Review task strategy test coverage".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::ReviewOnly);
        assert_eq!(plan.strategy, TaskStrategy::ReviewOnly);
    }

    #[test]
    fn review_heavy_profile_prefers_reviewer_strategy_without_review_hint() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            router_profile: Some(TaskRouterProfile::ReviewHeavy),
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.router_profile, TaskRouterProfile::ReviewHeavy);
        assert_eq!(plan.strategy, TaskStrategy::ImplementerPlusReviewer);
        assert!(plan
            .assignments
            .iter()
            .any(|assignment| assignment.role == HarnessRole::Reviewer));
        assert!(plan.candidate_scores[0]
            .factors
            .iter()
            .any(|factor| factor.name == "router_profile_fit" && factor.points > 0));
    }

    #[test]
    fn parallel_profile_prefers_parallel_research_without_parallel_hint() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Research the task router architecture".to_string(),
            explicit_mode: None,
            router_profile: Some(TaskRouterProfile::Parallel),
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.router_profile, TaskRouterProfile::Parallel);
        assert_eq!(plan.strategy, TaskStrategy::ParallelResearch);
        assert!(plan.layers.team);
        assert!(plan
            .approvals
            .contains(&TaskStrategyApproval::LaunchParallelWorkers));
    }

    #[test]
    fn fast_profile_is_inferred_from_goal_text() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Quickly inspect the task router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.router_profile, TaskRouterProfile::Fast);
        assert!(plan
            .reasons
            .iter()
            .any(|reason| reason.contains("inferred router profile Fast")));
    }

    #[test]
    fn requested_parallel_research_uses_researchers_without_eager_synthesizer() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Compare three approaches for a multi-harness router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: true,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::ParallelResearch);
        assert_eq!(plan.strategy, TaskStrategy::ParallelResearch);
        assert!(plan.layers.workflow);
        assert!(plan.layers.team);
        assert!(!plan.layers.worktree);
        let researchers = plan
            .assignments
            .iter()
            .filter(|assignment| assignment.role == HarnessRole::Researcher)
            .count();
        assert!(researchers >= 2);
        assert!(!plan
            .assignments
            .iter()
            .any(|assignment| assignment.role == HarnessRole::Synthesizer));
    }

    #[test]
    fn requested_parallelism_with_editing_intent_routes_to_parallel_experiment() {
        let requested = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the task router bug trying two approaches".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: true,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();
        let goal_worded = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the cache layer in parallel variants".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(requested.task_class, TaskClass::ParallelExperiment);
        assert_eq!(requested.strategy, TaskStrategy::ParallelExperiment);
        assert_eq!(requested.router_profile, TaskRouterProfile::Parallel);
        assert_eq!(goal_worded.task_class, TaskClass::ParallelExperiment);
        assert_eq!(goal_worded.strategy, TaskStrategy::ParallelExperiment);
    }

    #[test]
    fn parallel_review_hint_routes_to_parallel_research() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fai una review parallela nel repo".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: Some(TaskClass::ParallelResearch),
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::ParallelResearch);
        assert_eq!(plan.strategy, TaskStrategy::ParallelResearch);
        assert_eq!(plan.router_profile, TaskRouterProfile::Parallel);
        assert!(plan.layers.team);
    }

    #[test]
    fn requested_review_always_gets_reviewer_role() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: true,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry {
                harnesses: vec![HarnessCapability {
                    id: "codex".to_string(),
                    installed: true,
                    authenticated: true,
                    supports_prompt_launch: true,
                    supports_resume: true,
                    supports_hooks: true,
                    supports_mcp: true,
                    supports_plan_mode: false,
                    supports_worktree_cwd: true,
                    max_parallel_sessions: Some(4),
                    health: HarnessHealth::Ready,
                    routing_signals: HarnessRoutingSignals::default(),
                }],
            },
        })
        .unwrap();

        assert_eq!(plan.strategy, TaskStrategy::ImplementerPlusReviewer);
        assert!(plan
            .assignments
            .iter()
            .any(|assignment| assignment.role == HarnessRole::Implementer));
        assert!(plan
            .assignments
            .iter()
            .any(|assignment| assignment.role == HarnessRole::Reviewer));
        assert!(plan
            .approvals
            .contains(&TaskStrategyApproval::LaunchParallelWorkers));
    }

    #[test]
    fn reviewer_fallback_respects_single_session_capacity() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: true,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry {
                harnesses: vec![HarnessCapability {
                    max_parallel_sessions: Some(1),
                    ..harness("codex", false, true)
                }],
            },
        })
        .unwrap();

        assert_ne!(plan.strategy, TaskStrategy::ImplementerPlusReviewer);
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].harness_id, "codex");
    }

    #[test]
    fn parallel_research_respects_single_session_capacity() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Compare three approaches for a multi-harness router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: true,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry {
                harnesses: vec![HarnessCapability {
                    max_parallel_sessions: Some(1),
                    ..harness("codex", false, true)
                }],
            },
        })
        .unwrap();

        assert_ne!(plan.strategy, TaskStrategy::ParallelResearch);
        assert_eq!(plan.assignments.len(), 1);
        assert!(!plan
            .approvals
            .contains(&TaskStrategyApproval::LaunchParallelWorkers));
    }

    #[test]
    fn parallel_research_can_reuse_one_harness_when_capacity_allows() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Compare three approaches for a multi-harness router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: true,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry {
                harnesses: vec![HarnessCapability {
                    max_parallel_sessions: Some(3),
                    ..harness("codex", false, true)
                }],
            },
        })
        .unwrap();

        assert_eq!(plan.strategy, TaskStrategy::ParallelResearch);
        let researchers = plan
            .assignments
            .iter()
            .filter(|assignment| assignment.role == HarnessRole::Researcher)
            .collect::<Vec<_>>();
        assert!(researchers.len() >= 2);
        assert!(researchers
            .iter()
            .all(|assignment| assignment.harness_id == "codex"));
        assert!(plan
            .approvals
            .contains(&TaskStrategyApproval::LaunchParallelWorkers));
    }

    #[test]
    fn solo_strategy_falls_back_to_current_orchestrator_without_ready_harnesses() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Inspect the task router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry::default(),
        })
        .unwrap();

        assert_eq!(plan.strategy, TaskStrategy::SoloTracked);
        assert_eq!(plan.assignments[0].harness_id, "self");
        assert!(plan.assignments[0]
            .factors
            .iter()
            .any(|factor| factor.name == "self_fallback"));
    }

    #[test]
    fn classifier_uses_tokens_not_substrings() {
        let latest = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the latest feature".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();
        assert_eq!(latest.task_class, TaskClass::FeatureImplementation);

        let comparison_bug = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the comparison logic bug".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();
        assert_eq!(comparison_bug.task_class, TaskClass::FocusedBugfix);

        let preview = plan_task_strategy(TaskStrategyInput {
            goal: "Inspect preview rendering state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();
        assert_eq!(preview.task_class, TaskClass::RepoInspection);

        let read_only = plan_task_strategy(TaskStrategyInput {
            goal: "Read only review of the router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();
        assert_eq!(read_only.task_class, TaskClass::ReviewOnly);
    }

    #[test]
    fn reviewer_assignment_prefers_plan_mode_harness() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: true,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry {
                harnesses: vec![
                    harness("codex", false, true),
                    harness("opencode", false, true),
                    harness("claude", true, true),
                ],
            },
        })
        .unwrap();

        let reviewer = plan
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Reviewer)
            .unwrap();
        assert_eq!(reviewer.harness_id, "claude");

        let reviewer_json = serde_json::to_value(reviewer).unwrap();
        assert!(reviewer_json["score"].as_i64().unwrap_or_default() > 0);
        assert!(reviewer_json["factors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|factor| factor["name"] == "role_capability_fit"
                && factor["points"].as_i64() > Some(0)));
    }

    #[test]
    fn worktree_assignment_prefers_worktree_cwd_harness() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the task router bug and run tests".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: true,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry {
                harnesses: vec![
                    harness("codex", false, false),
                    harness("claude", true, true),
                ],
            },
        })
        .unwrap();

        assert_eq!(plan.strategy, TaskStrategy::SoloWithVerifyLoop);
        assert!(plan.layers.worktree);
        let implementer = plan
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Implementer)
            .unwrap();
        assert_eq!(implementer.harness_id, "claude");

        let implementer_json = serde_json::to_value(implementer).unwrap();
        assert!(implementer_json["score"].as_i64().unwrap_or_default() > 0);
        assert!(implementer_json["factors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|factor| factor["name"] == "worktree_cwd_fit"
                && factor["points"].as_i64() > Some(0)));
    }

    #[test]
    fn cooldown_penalty_prefers_available_harness_for_same_role() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Inspect the task router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry {
                harnesses: vec![
                    harness_with_signals(
                        "codex",
                        HarnessRoutingSignals {
                            cooldown: true,
                            cooldown_reason: Some("recent quota error".to_string()),
                            ..HarnessRoutingSignals::default()
                        },
                    ),
                    harness("claude", false, true),
                ],
            },
        })
        .unwrap();

        let implementer = plan
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Implementer)
            .unwrap();
        assert_eq!(implementer.harness_id, "claude");
        assert!(implementer
            .factors
            .iter()
            .any(|factor| factor.name == "session_cooldown" && factor.points == 0));
    }

    #[test]
    fn ranked_harnesses_for_role_orders_ready_harnesses_and_skips_locked_out() {
        let registry = HarnessRegistry {
            harnesses: vec![
                harness("codex", false, true),
                harness("claude", true, true),
                harness_with_signals(
                    "opencode",
                    HarnessRoutingSignals {
                        locked_out: true,
                        lockout_reason: Some("task mode disabled".to_string()),
                        ..HarnessRoutingSignals::default()
                    },
                ),
            ],
        };

        let ranked = ranked_harnesses_for_role(
            &HarnessRole::Reviewer,
            &registry,
            &TaskStrategyLayers::default(),
        );

        let ids = ranked
            .iter()
            .map(|assignment| assignment.harness_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["claude", "codex"]);
        assert!(ranked
            .iter()
            .all(|assignment| assignment.role == HarnessRole::Reviewer));
    }

    #[test]
    fn cooldown_kind_differentiates_session_cooldown_penalty() {
        for (kind, expected_points) in [
            (Some(HarnessCooldownKind::Auth), -60),
            (Some(HarnessCooldownKind::Crash), -45),
            (Some(HarnessCooldownKind::Quota), -35),
            (Some(HarnessCooldownKind::Timeout), -25),
            (None, -35),
        ] {
            let plan = plan_task_strategy(TaskStrategyInput {
                goal: "Inspect the task router state".to_string(),
                explicit_mode: None,
                router_profile: None,
                task_class_hint: None,
                last_known_good: None,
                repo_dirty: false,
                user_requested_parallelism: false,
                user_requested_review: false,
                likely_user_visible_change: false,
                harness_registry: HarnessRegistry {
                    harnesses: vec![harness_with_signals(
                        "codex",
                        HarnessRoutingSignals {
                            cooldown: true,
                            cooldown_kind: kind.clone(),
                            cooldown_reason: Some("recent runtime signal".to_string()),
                            ..HarnessRoutingSignals::default()
                        },
                    )],
                },
            })
            .unwrap();

            let factor = plan.assignments[0]
                .factors
                .iter()
                .find(|factor| factor.name == "session_cooldown")
                .unwrap();
            assert_eq!(factor.points, expected_points, "{kind:?}");
            if let Some(kind) = kind {
                let kind_id = match kind {
                    HarnessCooldownKind::Quota => "quota",
                    HarnessCooldownKind::Auth => "auth",
                    HarnessCooldownKind::Crash => "crash",
                    HarnessCooldownKind::Timeout => "timeout",
                };
                assert!(factor.reason.contains(kind_id), "{}", factor.reason);
            }
        }
    }

    #[test]
    fn cooldown_is_soft_when_no_other_harness_is_ready() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Inspect the task router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry {
                harnesses: vec![harness_with_signals(
                    "codex",
                    HarnessRoutingSignals {
                        cooldown: true,
                        cooldown_reason: Some("recent worker failure".to_string()),
                        ..HarnessRoutingSignals::default()
                    },
                )],
            },
        })
        .unwrap();

        let implementer = plan
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Implementer)
            .unwrap();
        assert_eq!(implementer.harness_id, "codex");
        assert!(implementer
            .factors
            .iter()
            .any(|factor| factor.name == "session_cooldown" && factor.points < 0));
    }

    #[test]
    fn task_mode_lockout_excludes_harness_from_assignments() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the task router bug and run tests".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry {
                harnesses: vec![
                    harness_with_signals(
                        "codex",
                        HarnessRoutingSignals {
                            locked_out: true,
                            lockout_reason: Some(
                                "task mode disabled after repeated failures".to_string(),
                            ),
                            ..HarnessRoutingSignals::default()
                        },
                    ),
                    harness("claude", false, true),
                ],
            },
        })
        .unwrap();

        assert!(plan
            .assignments
            .iter()
            .all(|assignment| assignment.harness_id != "codex"));
        let implementer = plan
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Implementer)
            .unwrap();
        assert_eq!(implementer.harness_id, "claude");
    }

    #[test]
    fn last_known_good_harness_biases_equivalent_assignment_without_overriding_cooldown() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Inspect the task router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: Some(TaskStrategyLastKnownGood {
                strategy: None,
                harness_id: Some("claude".to_string()),
                reason: Some("last successful router inspection in this repo".to_string()),
            }),
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry {
                harnesses: vec![
                    harness("codex", false, true),
                    harness("claude", false, true),
                ],
            },
        })
        .unwrap();

        let implementer = plan
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Implementer)
            .unwrap();
        assert_eq!(implementer.harness_id, "claude");
        assert!(implementer.factors.iter().any(|factor| {
            factor.name == "last_known_good_harness"
                && factor.points > 0
                && factor.reason.contains("last successful router inspection")
        }));

        let cooled_down = plan_task_strategy(TaskStrategyInput {
            goal: "Inspect the task router state".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: Some(TaskStrategyLastKnownGood {
                strategy: None,
                harness_id: Some("claude".to_string()),
                reason: Some("last successful router inspection in this repo".to_string()),
            }),
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: HarnessRegistry {
                harnesses: vec![
                    harness("codex", false, true),
                    harness_with_signals(
                        "claude",
                        HarnessRoutingSignals {
                            cooldown: true,
                            cooldown_reason: Some("recent quota error".to_string()),
                            ..HarnessRoutingSignals::default()
                        },
                    ),
                ],
            },
        })
        .unwrap();

        let implementer = cooled_down
            .assignments
            .iter()
            .find(|assignment| assignment.role == HarnessRole::Implementer)
            .unwrap();
        assert_eq!(implementer.harness_id, "codex");
    }

    #[test]
    fn last_known_good_strategy_is_explainable_but_not_a_hard_override() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the task router bug and run tests".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: Some(TaskStrategyLastKnownGood {
                strategy: Some(TaskStrategy::ParallelExperiment),
                harness_id: None,
                reason: Some("last successful experiment for this repo".to_string()),
            }),
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.strategy, TaskStrategy::SoloWithVerifyLoop);
        let lkg_candidate = plan
            .candidate_scores
            .iter()
            .find(|candidate| candidate.strategy == TaskStrategy::ParallelExperiment)
            .unwrap();
        assert!(lkg_candidate.factors.iter().any(|factor| {
            factor.name == "last_known_good_strategy"
                && factor.points > 0
                && factor.reason.contains("last successful experiment")
        }));
    }

    #[test]
    fn dirty_repo_requires_worktree_approval_for_editing_strategy() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: true,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert!(plan.layers.worktree);
        assert!(plan
            .approvals
            .contains(&TaskStrategyApproval::CreateWorktree));
        assert!(plan
            .reasons
            .iter()
            .any(|reason| reason.contains("dirty repo")));
    }

    #[test]
    fn implement_goal_with_review_token_is_not_review_only() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the code review workflow".to_string(),
            explicit_mode: None,
            router_profile: None,
            task_class_hint: None,
            last_known_good: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: HarnessRegistry::default(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::FeatureImplementation);
    }

    #[test]
    fn implementation_variants_with_review_terms_are_not_review_only() {
        for goal in [
            "Implementing the code review workflow",
            "Adding review workflow support",
            "Building the review dashboard",
            "Implement read-only mode for shared sessions",
        ] {
            let plan = plan_task_strategy(TaskStrategyInput {
                goal: goal.to_string(),
                explicit_mode: None,
                router_profile: None,
                task_class_hint: None,
                last_known_good: None,
                repo_dirty: false,
                user_requested_parallelism: false,
                user_requested_review: false,
                likely_user_visible_change: true,
                harness_registry: HarnessRegistry::default(),
            })
            .unwrap();

            assert_eq!(plan.task_class, TaskClass::FeatureImplementation, "{goal}");
        }
    }

    #[test]
    fn phase_one_classifier_reachable_classes_are_explicit() {
        let reachable = phase_one_classifier_reachable_classes();
        assert_eq!(
            reachable,
            &[
                TaskClass::RepoInspection,
                TaskClass::FocusedBugfix,
                TaskClass::FeatureImplementation,
                TaskClass::ReviewOnly,
                TaskClass::ParallelResearch,
                TaskClass::ParallelExperiment,
            ]
        );
    }
}
