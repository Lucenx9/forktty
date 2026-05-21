pub mod command_safety;
pub mod config;
pub mod model;
pub mod notification;
pub mod protocol;
pub mod session;
pub mod worktree;

pub use command_safety::{validate_worktree_name, WorktreeNameError};

pub use config::{AppConfig, AppearanceConfig, GeneralConfig, NotificationConfig};
pub use model::{
    LogEntry, LogLevel, NotificationItem, NotificationKind, PaneNode, ProgressEntry, SplitAxis,
    StatusEntry, Surface, SurfaceId, Workspace, WorkspaceId, WorkspaceModel, WorkspaceSelector,
};
pub use notification::{dispatch_notification, NotificationDispatchError};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
