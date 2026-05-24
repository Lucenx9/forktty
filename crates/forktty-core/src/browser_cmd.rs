//! Pure command/result types for the browser-pane scripting channel (SP2).
//!
//! These cross the socket thread -> GTK main thread boundary. They hold no
//! GTK/WebKit types so they can live in the pure core crate; `forktty-socket`
//! builds the commands and `forktty-ui-gtk` consumes them behind the `browser`
//! cargo feature.

use tokio::sync::oneshot;

/// Maximum byte length of a JSON result returned by a browser command.
pub const MAX_BROWSER_RESULT_BYTES: usize = 256 * 1024;

/// Maximum byte length of a JS-evaluate script.
pub const MAX_BROWSER_SCRIPT_BYTES: usize = 64 * 1024;

/// What a browser command asks the WebView to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserOp {
    /// Walk the accessibility tree, assign element refs, return a JSON tree.
    Snapshot,
    /// Click the element previously assigned `reference` by a snapshot.
    Click {
        reference: String,
    },
    /// Set the value of the element `reference` to `value`.
    Fill {
        reference: String,
        value: String,
    },
    /// Run arbitrary JavaScript, return its JSON-serialized result.
    Eval {
        script: String,
    },
    /// Navigate back / forward in session history.
    Back,
    Forward,
    /// Reload the current page.
    Reload,
}

/// Result of running a [`BrowserCommand`] against the WebView.
#[derive(Debug)]
pub enum CmdResult {
    /// Operation completed, no payload (nav / click / fill).
    Ok,
    /// Operation produced a JSON value (snapshot / JS-evaluate), already serialized.
    Json(String),
    /// Operation failed.
    Err(BrowserCmdError),
}

/// Why a browser command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCmdError {
    /// The addressed surface no longer exists.
    SurfaceGone,
    /// The addressed surface is not a browser surface.
    NotABrowser,
    /// No live WebView is realized for the surface.
    NoWebView,
    /// A snapshot ref was not found (stale after navigation).
    RefNotFound,
    /// JavaScript threw or evaluation failed.
    JsError(String),
    /// The result exceeded [`MAX_BROWSER_RESULT_BYTES`].
    TooLarge,
    /// An unexpected internal failure (e.g. the reply channel was dropped).
    Internal(String),
}

impl std::fmt::Display for BrowserCmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserCmdError::SurfaceGone => f.write_str("surface no longer exists"),
            BrowserCmdError::NotABrowser => f.write_str("surface is not a browser"),
            BrowserCmdError::NoWebView => f.write_str("no live web view for surface"),
            BrowserCmdError::RefNotFound => f.write_str("element ref not found"),
            BrowserCmdError::JsError(msg) => write!(f, "javascript error: {msg}"),
            BrowserCmdError::TooLarge => f.write_str("result too large"),
            BrowserCmdError::Internal(msg) => write!(f, "internal browser error: {msg}"),
        }
    }
}

/// A command sent from the socket thread to the GTK WebView, with a one-shot
/// reply channel the GTK side fulfils once the operation settles.
#[derive(Debug)]
pub struct BrowserCommand {
    pub surface_id: String,
    pub op: BrowserOp,
    pub reply: oneshot::Sender<CmdResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_op_equality() {
        assert_eq!(BrowserOp::Snapshot, BrowserOp::Snapshot);
        assert_eq!(
            BrowserOp::Click {
                reference: "e1".into()
            },
            BrowserOp::Click {
                reference: "e1".into()
            }
        );
        assert_ne!(
            BrowserOp::Click {
                reference: "e1".into()
            },
            BrowserOp::Click {
                reference: "e2".into()
            }
        );
    }

    #[test]
    fn error_display_strings_are_stable() {
        assert_eq!(
            BrowserCmdError::RefNotFound.to_string(),
            "element ref not found"
        );
        assert_eq!(
            BrowserCmdError::NotABrowser.to_string(),
            "surface is not a browser"
        );
        assert_eq!(
            BrowserCmdError::JsError("boom".into()).to_string(),
            "javascript error: boom"
        );
    }

    #[test]
    fn size_constants_are_sane() {
        assert!(MAX_BROWSER_SCRIPT_BYTES < MAX_BROWSER_RESULT_BYTES);
    }

    #[tokio::test]
    async fn command_reply_round_trips() {
        let (tx, rx) = oneshot::channel();
        let cmd = BrowserCommand {
            surface_id: "s1".into(),
            op: BrowserOp::Snapshot,
            reply: tx,
        };
        cmd.reply.send(CmdResult::Json("{}".into())).unwrap();
        match rx.await.unwrap() {
            CmdResult::Json(s) => assert_eq!(s, "{}"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
