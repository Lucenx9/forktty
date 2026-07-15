use std::time::Duration;

/// Maximum size of one newline-delimited JSON-RPC socket request.
pub const SOCKET_REQUEST_MAX_BYTES: usize = 1_048_576;
/// Maximum size accepted by official socket clients for one JSON-RPC response.
pub const OFFICIAL_SOCKET_RESPONSE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// Timeout used by official socket CLI clients.
pub const OFFICIAL_SOCKET_TIMEOUT: Duration = Duration::from_secs(35);

/// Maximum text payload accepted by `surface.send_text`.
pub const SOCKET_SEND_TEXT_MAX_BYTES: usize = 262_144;
/// Maximum terminal text bytes returned by read/capture operations.
pub const SOCKET_TERMINAL_TEXT_MAX_BYTES: usize = 512 * 1024;
/// Default byte cap for each terminal tail in `context.snapshot`.
pub const DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES: usize = 16 * 1024;
/// Aggregate byte cap for terminal tails in `context.snapshot`.
pub const MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES: usize = 1024 * 1024;
/// Maximum text stored by metadata log/status/progress methods.
pub const SOCKET_METADATA_TEXT_MAX_BYTES: usize = 16_384;
/// Maximum text read from stdin or files by socket CLI helpers.
pub const CLI_STDIN_TEXT_MAX_BYTES: usize = SOCKET_REQUEST_MAX_BYTES;

/// Server timeout while waiting for a complete socket request line.
pub const SOCKET_REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Server timeout while writing one socket response.
pub const SOCKET_RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
