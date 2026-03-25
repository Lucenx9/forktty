use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

fn default_socket_path() -> String {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        format!("{runtime_dir}/forktty.sock")
    } else {
        "/tmp/forktty.sock".to_string()
    }
}

#[derive(Parser)]
#[command(
    name = "forktty-cli",
    about = "ForkTTY CLI — control the terminal from scripts"
)]
struct Cli {
    /// Socket path (default: $XDG_RUNTIME_DIR/forktty.sock or /tmp/forktty.sock)
    #[arg(long, env = "FORKTTY_SOCKET_PATH", default_value_t = default_socket_path())]
    socket: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check if ForkTTY is running
    Ping,

    /// List workspaces
    Ls,

    /// Create a new workspace (optionally with worktree)
    New {
        /// Worktree/workspace name
        name: Option<String>,
        /// Send initial prompt text after creation
        #[arg(short, long)]
        prompt: Option<String>,
    },

    /// Focus a workspace by name
    Select {
        /// Workspace name
        name: String,
    },

    /// Send text to a surface by PTY ID
    Send {
        /// PTY ID
        pty_id: u32,
        /// Text to send
        text: String,
    },

    /// Create a notification
    Notify {
        /// Notification title
        #[arg(long)]
        title: String,
        /// Notification body
        #[arg(long, default_value = "")]
        body: String,
    },

    /// Split the focused pane
    Split {
        /// Direction: "right" or "down"
        #[arg(default_value = "right")]
        direction: String,
    },

    /// Merge a worktree branch into main
    Merge {
        /// Branch/worktree name
        name: String,
    },

    /// Remove a workspace and its worktree
    Rm {
        /// Workspace name
        name: String,
    },

    /// List notifications
    Notifications,

    /// Clear all notifications
    ClearNotifications,

    /// Read the terminal screen content
    ReadScreen {
        /// Surface ID (reads focused surface if omitted)
        #[arg(long)]
        surface_id: Option<String>,
    },

    /// Workspace metadata commands (status pills, progress bars, logs)
    Metadata {
        #[command(subcommand)]
        command: MetadataCommands,
    },
}

#[derive(Subcommand, Debug)]
enum MetadataCommands {
    /// Set a status pill on a workspace
    SetStatus {
        /// Status key (used for upsert)
        key: String,
        /// Display label
        label: String,
        /// Display value
        value: String,
        /// Optional color (green, yellow, red, blue, muted, or raw CSS)
        #[arg(long)]
        color: Option<String>,
        /// Workspace name (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// List status pills on a workspace
    ListStatus {
        /// Workspace name (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Clear status pills on a workspace
    ClearStatus {
        /// Clear only this key (clears all if omitted)
        #[arg(long)]
        key: Option<String>,
        /// Workspace name (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Set a progress bar on a workspace
    SetProgress {
        /// Progress key (used for upsert)
        key: String,
        /// Display label
        label: String,
        /// Current value
        value: f64,
        /// Total value (defaults to 100)
        #[arg(long)]
        total: Option<f64>,
        /// Workspace name (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Clear progress bars on a workspace
    ClearProgress {
        /// Clear only this key (clears all if omitted)
        #[arg(long)]
        key: Option<String>,
        /// Workspace name (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Append a log entry to a workspace
    Log {
        /// Log message
        message: String,
        /// Log level: info, warn, or error
        #[arg(long, default_value = "info")]
        level: String,
        /// Workspace name (defaults to active)
        #[arg(long)]
        workspace: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    let (method, params) = build_request(&cli.command);

    match send_request(&cli.socket, method, params) {
        Ok(response) => {
            if let Some(true) = response.get("ok").and_then(|v| v.as_bool()) {
                let result = response.get("result").unwrap_or(&Value::Null);
                if result.is_array() || result.is_object() {
                    println!("{}", serde_json::to_string_pretty(result).unwrap());
                } else if let Some(s) = result.as_str() {
                    println!("{s}");
                } else if result != &Value::Null {
                    println!("{result}");
                }
            } else {
                let error = response
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            eprintln!("Is ForkTTY running? Check socket at: {}", cli.socket);
            std::process::exit(1);
        }
    }
}

fn build_request(command: &Commands) -> (&'static str, Value) {
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().to_string());
    match command {
        Commands::Ping => ("system.ping", json!({})),
        Commands::Ls => ("workspace.list", json!({})),
        Commands::New { name, prompt } => match name {
            Some(name) => (
                "worktree.create",
                json!({ "name": name, "prompt": prompt, "cwd": cwd }),
            ),
            None => ("workspace.create", json!({ "prompt": prompt })),
        },
        Commands::Select { name } => ("workspace.select", json!({ "name": name })),
        Commands::Send { pty_id, text } => (
            "surface.send_text",
            json!({ "pty_id": pty_id, "text": text }),
        ),
        Commands::Notify { title, body } => (
            "notification.create",
            json!({ "title": title, "body": body }),
        ),
        Commands::Split { direction } => ("surface.split", json!({ "direction": direction })),
        Commands::Merge { name } => ("worktree.merge", json!({ "name": name, "cwd": cwd })),
        Commands::Rm { name } => ("worktree.remove", json!({ "name": name, "cwd": cwd })),
        Commands::Notifications => ("notification.list", json!({})),
        Commands::ClearNotifications => ("notification.clear", json!({})),
        Commands::ReadScreen { surface_id } => {
            ("surface.read_screen", json!({ "surface_id": surface_id }))
        }
        Commands::Metadata { command } => build_metadata_request(command),
    }
}

fn build_metadata_request(command: &MetadataCommands) -> (&'static str, Value) {
    match command {
        MetadataCommands::SetStatus {
            key,
            label,
            value,
            color,
            workspace,
        } => (
            "metadata.set_status",
            json!({
                "key": key,
                "label": label,
                "value": value,
                "color": color,
                "workspace_name": workspace,
            }),
        ),
        MetadataCommands::ListStatus { workspace } => (
            "metadata.list_status",
            json!({ "workspace_name": workspace }),
        ),
        MetadataCommands::ClearStatus { key, workspace } => (
            "metadata.clear_status",
            json!({ "key": key, "workspace_name": workspace }),
        ),
        MetadataCommands::SetProgress {
            key,
            label,
            value,
            total,
            workspace,
        } => (
            "metadata.set_progress",
            json!({
                "key": key,
                "label": label,
                "value": value,
                "total": total,
                "workspace_name": workspace,
            }),
        ),
        MetadataCommands::ClearProgress { key, workspace } => (
            "metadata.clear_progress",
            json!({ "key": key, "workspace_name": workspace }),
        ),
        MetadataCommands::Log {
            message,
            level,
            workspace,
        } => (
            "metadata.log",
            json!({
                "message": message,
                "level": level,
                "workspace_name": workspace,
            }),
        ),
    }
}

fn send_request(socket_path: &str, method: &str, params: Value) -> Result<Value, String> {
    let mut stream =
        UnixStream::connect(socket_path).map_err(|e| format!("Cannot connect: {e}"))?;

    let request = json!({
        "id": format!("cli-{}", std::process::id()),
        "method": method,
        "params": params,
    });

    let mut msg = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    msg.push(b'\n');
    stream.write_all(&msg).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut line = String::new();
    BufReader::new((&stream).take(1_048_576))
        .read_line(&mut line)
        .map_err(|e: std::io::Error| e.to_string())?;

    serde_json::from_str(&line).map_err(|e| format!("Invalid response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_name_uses_worktree_create() {
        let (method, params) = build_request(&Commands::New {
            name: Some("feature-x".to_string()),
            prompt: Some("hello".to_string()),
        });

        assert_eq!(method, "worktree.create");
        assert_eq!(params["name"], "feature-x");
        assert_eq!(params["prompt"], "hello");
        assert!(params["cwd"].is_string());
    }

    #[test]
    fn new_without_name_uses_workspace_create() {
        let (method, params) = build_request(&Commands::New {
            name: None,
            prompt: Some("hello".to_string()),
        });

        assert_eq!(method, "workspace.create");
        assert!(params.get("name").is_none());
        assert_eq!(params["prompt"], "hello");
    }

    #[test]
    fn rm_uses_worktree_remove() {
        let (method, params) = build_request(&Commands::Rm {
            name: "feature-x".to_string(),
        });

        assert_eq!(method, "worktree.remove");
        assert_eq!(params["name"], "feature-x");
        assert!(params["cwd"].is_string());
    }

    #[test]
    fn metadata_set_status_builds_correct_request() {
        let cmd = MetadataCommands::SetStatus {
            key: "build".to_string(),
            label: "Build".to_string(),
            value: "passing".to_string(),
            color: Some("green".to_string()),
            workspace: Some("my-ws".to_string()),
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.set_status");
        assert_eq!(params["key"], "build");
        assert_eq!(params["label"], "Build");
        assert_eq!(params["value"], "passing");
        assert_eq!(params["color"], "green");
        assert_eq!(params["workspace_name"], "my-ws");
    }

    #[test]
    fn metadata_set_status_without_optional_fields() {
        let cmd = MetadataCommands::SetStatus {
            key: "ci".to_string(),
            label: "CI".to_string(),
            value: "running".to_string(),
            color: None,
            workspace: None,
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.set_status");
        assert_eq!(params["key"], "ci");
        assert!(params["color"].is_null());
        assert!(params["workspace_name"].is_null());
    }

    #[test]
    fn metadata_list_status_builds_correct_request() {
        let cmd = MetadataCommands::ListStatus {
            workspace: Some("ws1".to_string()),
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.list_status");
        assert_eq!(params["workspace_name"], "ws1");
    }

    #[test]
    fn metadata_clear_status_with_key() {
        let cmd = MetadataCommands::ClearStatus {
            key: Some("build".to_string()),
            workspace: None,
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.clear_status");
        assert_eq!(params["key"], "build");
        assert!(params["workspace_name"].is_null());
    }

    #[test]
    fn metadata_set_progress_builds_correct_request() {
        let cmd = MetadataCommands::SetProgress {
            key: "download".to_string(),
            label: "Downloading".to_string(),
            value: 42.0,
            total: Some(100.0),
            workspace: None,
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.set_progress");
        assert_eq!(params["key"], "download");
        assert_eq!(params["label"], "Downloading");
        assert_eq!(params["value"], 42.0);
        assert_eq!(params["total"], 100.0);
    }

    #[test]
    fn metadata_clear_progress_builds_correct_request() {
        let cmd = MetadataCommands::ClearProgress {
            key: None,
            workspace: Some("ws2".to_string()),
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.clear_progress");
        assert!(params["key"].is_null());
        assert_eq!(params["workspace_name"], "ws2");
    }

    #[test]
    fn metadata_log_builds_correct_request() {
        let cmd = MetadataCommands::Log {
            message: "Build succeeded".to_string(),
            level: "info".to_string(),
            workspace: None,
        };
        let (method, params) = build_metadata_request(&cmd);
        assert_eq!(method, "metadata.log");
        assert_eq!(params["message"], "Build succeeded");
        assert_eq!(params["level"], "info");
    }

    #[test]
    fn metadata_via_build_request() {
        let cmd = Commands::Metadata {
            command: MetadataCommands::Log {
                message: "test".to_string(),
                level: "warn".to_string(),
                workspace: None,
            },
        };
        let (method, params) = build_request(&cmd);
        assert_eq!(method, "metadata.log");
        assert_eq!(params["message"], "test");
        assert_eq!(params["level"], "warn");
    }
}
