use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Default, Debug)]
pub(crate) struct SocketCoordinator {
    pub(crate) team_message_dispatch: tokio::sync::Mutex<()>,
    pub(crate) team_worker_launch: tokio::sync::Mutex<()>,
    pub(crate) surface_set: tokio::sync::Mutex<()>,
    team_launch_owned_surfaces: Mutex<HashSet<TeamLaunchOwnedSurface>>,
    team_terminal_dispatched_messages: Mutex<HashSet<TeamTerminalDispatchedMessage>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TeamLaunchOwnedSurface {
    team_id: String,
    worker_id: String,
    surface_id: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TeamTerminalDispatchedMessage {
    store_path: PathBuf,
    team_id: String,
    message_id: String,
}

impl TeamTerminalDispatchedMessage {
    pub(crate) fn new(store_path: PathBuf, team_id: &str, message_id: &str) -> Self {
        Self {
            store_path,
            team_id: team_id.to_string(),
            message_id: message_id.to_string(),
        }
    }
}

impl SocketCoordinator {
    pub(crate) fn remember_team_launch_owned_surface(
        &self,
        team_id: &str,
        worker_id: &str,
        surface_id: &str,
    ) -> Result<(), String> {
        self.team_launch_owned_surfaces
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .insert(TeamLaunchOwnedSurface {
                team_id: team_id.to_string(),
                worker_id: worker_id.to_string(),
                surface_id: surface_id.to_string(),
            });
        Ok(())
    }

    pub(crate) fn is_current_team_launch_owned_surface(
        &self,
        team_id: &str,
        worker_id: &str,
        surface_id: &str,
    ) -> Result<bool, String> {
        Ok(self
            .team_launch_owned_surfaces
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .contains(&TeamLaunchOwnedSurface {
                team_id: team_id.to_string(),
                worker_id: worker_id.to_string(),
                surface_id: surface_id.to_string(),
            }))
    }

    pub(crate) fn forget_team_launch_owned_surface_for_surface(
        &self,
        surface_id: &str,
    ) -> Result<(), String> {
        self.team_launch_owned_surfaces
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .retain(|owned| owned.surface_id != surface_id);
        Ok(())
    }

    pub(crate) fn forget_team_launch_owned_surfaces_for_surfaces(
        &self,
        surface_ids: &HashSet<&str>,
    ) -> Result<(), String> {
        self.team_launch_owned_surfaces
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .retain(|owned| !surface_ids.contains(owned.surface_id.as_str()));
        Ok(())
    }

    pub(crate) fn ensure_team_message_not_terminal_dispatched(
        &self,
        message: &TeamTerminalDispatchedMessage,
    ) -> Result<(), String> {
        if self
            .team_terminal_dispatched_messages
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .contains(message)
        {
            return Err("message already dispatched to a terminal in this runtime".to_string());
        }
        Ok(())
    }

    pub(crate) fn remember_team_message_terminal_dispatched(
        &self,
        message: TeamTerminalDispatchedMessage,
    ) -> Result<(), String> {
        self.team_terminal_dispatched_messages
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .insert(message);
        Ok(())
    }

    pub(crate) fn forget_team_message_terminal_dispatched(
        &self,
        message: &TeamTerminalDispatchedMessage,
    ) -> Result<(), String> {
        self.team_terminal_dispatched_messages
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .remove(message);
        Ok(())
    }
}
