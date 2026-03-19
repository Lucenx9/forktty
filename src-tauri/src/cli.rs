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
        /// Workspace name
        name: String,
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
}

fn main() {
    let cli = Cli::parse();

    let (method, params) = match &cli.command {
        Commands::Ping => ("system.ping", json!({})),
        Commands::Ls => ("workspace.list", json!({})),
        Commands::New { name, prompt } => (
            "workspace.create",
            json!({ "name": name, "prompt": prompt }),
        ),
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
        Commands::Merge { name } => ("worktree.merge", json!({ "name": name })),
        Commands::Rm { name } => ("workspace.close", json!({ "name": name })),
        Commands::Notifications => ("notification.list", json!({})),
        Commands::ClearNotifications => ("notification.clear", json!({})),
    };

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
