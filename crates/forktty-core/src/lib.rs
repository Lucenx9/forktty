pub mod agents;
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
pub mod protocol;
pub mod session;
pub mod worktree;

pub use agents::{
    normalize_agent_status, AgentCapabilities, AgentConfigLocation, AgentKind, AgentProfile,
    AgentStatus,
};
pub use browser_cmd::{
    BrowserCmdError, BrowserCommand, BrowserOp, CmdResult, MAX_BROWSER_RESULT_BYTES,
    MAX_BROWSER_SCRIPT_BYTES,
};
pub use command_safety::{validate_worktree_name, WorktreeNameError};

pub use browser_history::{HistoryError, HistoryStore, Visit};
pub use config::{AppConfig, AppearanceConfig, GeneralConfig, NotificationConfig};
pub use model::{
    has_uri_scheme, LogEntry, LogLevel, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, SplitAxis, StatusEntry, StatusHookMetadata, Surface, SurfaceId, SurfaceKind,
    Workspace, WorkspaceId, WorkspaceModel, WorkspaceSelector,
};
pub use notification::{dispatch_notification, NotificationDispatchError};
pub use profile::{ProfileError, ProfileId, ProfileMeta, ProfileStore};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
