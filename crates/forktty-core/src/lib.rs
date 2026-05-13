pub mod config;
pub mod model;
pub mod notification;
pub mod protocol;
pub mod session;
pub mod worktree;

pub use config::{AppConfig, AppearanceConfig, GeneralConfig, NotificationConfig};
pub use model::{
    NotificationItem, NotificationKind, PaneNode, SplitAxis, StatusEntry, Surface, SurfaceId,
    Workspace, WorkspaceId, WorkspaceModel, WorkspaceSelector,
};
pub use notification::{dispatch_notification, NotificationDispatchError};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
