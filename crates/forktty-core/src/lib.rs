pub mod agents;
mod backup;
pub mod browser_cmd;
pub mod browser_history;
pub mod command_safety;
pub mod config;
pub mod events;
pub mod model;
pub mod notification;
pub mod ports;
pub mod pr;
pub mod profile;
pub mod project_actions;
pub mod protocol;
pub mod protocol_limits;
pub mod pty_persistence;
pub mod session;
pub mod update;
pub mod worktree;

pub use agents::{
    agent_resume_command, agent_resume_command_with_cwd,
    agent_resume_command_with_cwd_and_permission_mode, codex_session_cwd,
    codex_session_cwd_from_home, normalize_agent_status, AgentKind, AgentResumeCommand,
    AgentResumeError, AgentStatus,
};
pub use browser_cmd::{
    BrowserCmdError, BrowserCommand, BrowserOp, CmdResult, MAX_BROWSER_RESULT_BYTES,
};
pub use browser_history::{Bookmark, BookmarkStore, HistoryError, HistoryStore, Visit};
pub use command_safety::{validate_worktree_name, WorktreeNameError};
pub use config::{AppConfig, AppearanceConfig, GeneralConfig, NotificationConfig, UpdateConfig};
pub use model::{
    has_uri_scheme, normalize_browser_url, resolve_worktree_identity_snapshots,
    validated_browser_url, AgentSession, AgentSessionLifecycle, ClearedAgentMetadata,
    ClearedAgentSession, HookPromptKind, HookPromptMetadata, HookPromptResolution, LogEntry,
    LogLevel, MovePosition, NotificationCreation, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, ResolvedWorktreeIdentity, SplitAxis, StatusEntry, StatusHookMetadata, Surface,
    SurfaceId, SurfaceKind, TerminalNotificationMetadata, Workspace, WorkspaceId, WorkspaceModel,
    WorkspaceSelector, WorktreeIdentitySnapshot, WorktreeRemovalIdentity,
    WorktreeWorkspaceResolution, MAX_BROWSER_URL_BYTES, MAX_PERSISTED_SCROLLBACK_BYTES,
};
pub use notification::{
    close_desktop_notification, dispatch_notification, NotificationDispatchError,
};
pub use profile::{ProfileError, ProfileId, ProfileMeta, ProfileStore};
pub use project_actions::{
    action_cwd, discover_project_root, find_action, load_project_actions, ProjectAction,
    ProjectActionError,
};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use update::{select_newest_update, AssetKind, AvailableUpdate, ReleaseAsset, TargetArch};
