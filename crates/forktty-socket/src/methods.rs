#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MethodExposure {
    Public,
    ConnectionLevel,
    #[cfg(feature = "browser")]
    InternalOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MethodSpec {
    pub(crate) name: &'static str,
    pub(crate) exposure: MethodExposure,
}

impl MethodSpec {
    const fn public(name: &'static str) -> Self {
        Self {
            name,
            exposure: MethodExposure::Public,
        }
    }

    const fn connection_level(name: &'static str) -> Self {
        Self {
            name,
            exposure: MethodExposure::ConnectionLevel,
        }
    }

    #[cfg(feature = "browser")]
    const fn internal_only(name: &'static str) -> Self {
        Self {
            name,
            exposure: MethodExposure::InternalOnly,
        }
    }
}

#[cfg(feature = "browser")]
const BROWSER_PUBLIC_METHOD_SPECS: &[MethodSpec] = &[
    MethodSpec::public("browser.back"),
    MethodSpec::public("browser.bookmark.add"),
    MethodSpec::public("browser.bookmark.list"),
    MethodSpec::public("browser.bookmark.remove"),
    MethodSpec::public("browser.click"),
    MethodSpec::public("browser.fill"),
    MethodSpec::public("browser.forward"),
    MethodSpec::public("browser.history.clear"),
    MethodSpec::public("browser.history.list"),
    MethodSpec::public("browser.history.search"),
    MethodSpec::public("browser.navigate"),
    MethodSpec::public("browser.open"),
    MethodSpec::public("browser.profile.create"),
    MethodSpec::public("browser.profile.delete"),
    MethodSpec::public("browser.profile.list"),
    MethodSpec::public("browser.reload"),
    MethodSpec::public("browser.snapshot"),
];

const CORE_METHOD_SPECS: &[MethodSpec] = &[
    MethodSpec::public("context.snapshot"),
    MethodSpec::connection_level("events.subscribe"),
    MethodSpec::public("feed.approval.respond"),
    MethodSpec::public("feed.list"),
    MethodSpec::public("agent.health"),
    MethodSpec::public("agent.hibernate"),
    MethodSpec::public("agent.list"),
    MethodSpec::public("agent.reclaim.plan"),
    MethodSpec::public("agent.reclaim"),
    MethodSpec::public("agent.resume"),
    MethodSpec::public("metadata.clear_logs"),
    MethodSpec::public("metadata.clear_progress"),
    MethodSpec::public("metadata.clear_status"),
    MethodSpec::public("metadata.list_logs"),
    MethodSpec::public("metadata.list_progress"),
    MethodSpec::public("metadata.list_status"),
    MethodSpec::public("metadata.log"),
    MethodSpec::public("metadata.set_progress"),
    MethodSpec::public("metadata.set_status"),
    MethodSpec::public("notification.clear"),
    MethodSpec::public("notification.create"),
    MethodSpec::public("notification.list"),
    MethodSpec::public("orchestration.cleanup"),
    MethodSpec::public("pane.new_tab"),
    MethodSpec::public("pane.select_tab"),
    MethodSpec::public("project.action.list"),
    MethodSpec::public("project.action.run"),
    MethodSpec::public("remote.list"),
    MethodSpec::public("remote.status"),
    MethodSpec::public("surface.close"),
    MethodSpec::public("surface.focus"),
    MethodSpec::public("surface.capture_tail"),
    MethodSpec::public("surface.list"),
    MethodSpec::public("surface.read_text"),
    MethodSpec::public("surface.send_text"),
    MethodSpec::public("surface.split"),
    MethodSpec::public("status.summary"),
    MethodSpec::public("system.capabilities"),
    MethodSpec::public("system.identify"),
    MethodSpec::public("system.ping"),
    MethodSpec::public("system.top"),
    MethodSpec::public("task.strategy.apply"),
    MethodSpec::public("task.strategy.plan"),
    MethodSpec::public("team.events"),
    MethodSpec::public("team.finish"),
    MethodSpec::public("team.get"),
    MethodSpec::public("team.inbox"),
    MethodSpec::public("team.list"),
    MethodSpec::public("team.message.dispatch"),
    MethodSpec::public("team.message.ack"),
    MethodSpec::public("team.message.send"),
    MethodSpec::public("team.summary"),
    MethodSpec::public("team.task.upsert"),
    MethodSpec::public("team.upsert"),
    MethodSpec::public("team.worker.heartbeat"),
    MethodSpec::public("team.worker.health"),
    MethodSpec::public("team.worker.launch"),
    MethodSpec::public("team.worker.nudge"),
    MethodSpec::public("team.worker.shutdown"),
    MethodSpec::public("team.worker.upsert"),
    MethodSpec::public("topology.tree"),
    MethodSpec::public("workspace.close"),
    MethodSpec::public("workspace.create"),
    MethodSpec::public("workspace.create_ssh"),
    MethodSpec::public("workspace.list"),
    MethodSpec::public("workspace.select"),
    MethodSpec::public("workflow.evidence.add"),
    MethodSpec::public("workflow.get"),
    MethodSpec::public("workflow.list"),
    MethodSpec::public("workflow.loop.set"),
    MethodSpec::public("workflow.plan.set"),
    MethodSpec::public("workflow.replay"),
    MethodSpec::public("workflow.upsert"),
    MethodSpec::public("worktree.attach"),
    MethodSpec::public("worktree.create"),
    MethodSpec::public("worktree.list"),
    MethodSpec::public("worktree.merge"),
    MethodSpec::public("worktree.remove"),
    MethodSpec::public("worktree.status"),
];

#[cfg(feature = "browser")]
const BROWSER_INTERNAL_METHOD_SPECS: &[MethodSpec] = &[
    MethodSpec::internal_only("browser.import.discover"),
    MethodSpec::internal_only("browser.import.preview"),
    MethodSpec::internal_only("browser.import.run"),
];

pub(crate) fn capability_method_names() -> Vec<&'static str> {
    let mut names = Vec::new();
    #[cfg(feature = "browser")]
    names.extend(BROWSER_PUBLIC_METHOD_SPECS.iter().map(|spec| spec.name));
    names.extend(
        CORE_METHOD_SPECS
            .iter()
            .filter(|spec| {
                matches!(
                    spec.exposure,
                    MethodExposure::Public | MethodExposure::ConnectionLevel
                )
            })
            .map(|spec| spec.name),
    );
    names
}

#[cfg(test)]
pub(crate) fn method_specs() -> Vec<&'static MethodSpec> {
    let mut specs = Vec::new();
    #[cfg(feature = "browser")]
    specs.extend(BROWSER_PUBLIC_METHOD_SPECS.iter());
    specs.extend(CORE_METHOD_SPECS.iter());
    #[cfg(feature = "browser")]
    specs.extend(BROWSER_INTERNAL_METHOD_SPECS.iter());
    specs
}

#[cfg(any(test, feature = "browser"))]
pub(crate) fn exposure(method: &str) -> Option<MethodExposure> {
    #[cfg(feature = "browser")]
    if let Some(spec) = BROWSER_PUBLIC_METHOD_SPECS
        .iter()
        .find(|spec| spec.name == method)
    {
        return Some(spec.exposure);
    }
    if let Some(spec) = CORE_METHOD_SPECS.iter().find(|spec| spec.name == method) {
        return Some(spec.exposure);
    }
    #[cfg(feature = "browser")]
    if let Some(spec) = BROWSER_INTERNAL_METHOD_SPECS
        .iter()
        .find(|spec| spec.name == method)
    {
        return Some(spec.exposure);
    }
    None
}

pub(crate) fn allowed_from_socket(method: &str) -> bool {
    #[cfg(not(feature = "browser"))]
    {
        !method.starts_with("browser.import.")
    }
    #[cfg(feature = "browser")]
    !matches!(exposure(method), Some(MethodExposure::InternalOnly))
}
