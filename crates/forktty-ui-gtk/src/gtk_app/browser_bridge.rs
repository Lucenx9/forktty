use super::*;

#[cfg(feature = "browser")]
pub(super) fn handle_browser_command(
    controller: &Rc<RefCell<TerminalController>>,
    cmd: forktty_core::BrowserCommand,
) {
    use crate::browser_pane::{click_js, fill_js};
    use forktty_core::{BrowserCmdError, BrowserOp, CmdResult};

    let pane = controller.borrow().browser_pane(&cmd.surface_id);
    let Some(pane) = pane else {
        let _ = cmd.reply.send(CmdResult::Err(BrowserCmdError::NoWebView));
        return;
    };
    let reply = cmd.reply;
    match cmd.op {
        BrowserOp::Snapshot => {
            pane.run_js("window.__forktty.snapshot()", move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        BrowserOp::Click { reference } => {
            pane.run_js(&click_js(&reference), move |r| {
                let _ = reply.send(into_ok_cmd_result(r));
            });
        }
        BrowserOp::Fill { reference, value } => {
            pane.run_js(&fill_js(&reference, &value), move |r| {
                let _ = reply.send(into_ok_cmd_result(r));
            });
        }
        BrowserOp::Eval { script } => {
            pane.run_js(&script, move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        // Nav ops are fire-and-forget: Ok means "navigation initiated", not
        // "page loaded". Callers issue a follow-up snapshot to see the result.
        BrowserOp::Back => {
            pane.go_back();
            let _ = reply.send(CmdResult::Ok);
        }
        BrowserOp::Forward => {
            pane.go_forward();
            let _ = reply.send(CmdResult::Ok);
        }
        BrowserOp::Reload => {
            pane.reload();
            let _ = reply.send(CmdResult::Ok);
        }
    }
}

#[cfg(feature = "browser")]
pub(super) fn into_cmd_result(
    r: Result<String, forktty_core::BrowserCmdError>,
) -> forktty_core::CmdResult {
    match r {
        Ok(json) => forktty_core::CmdResult::Json(json),
        Err(e) => forktty_core::CmdResult::Err(e),
    }
}

#[cfg(feature = "browser")]
pub(super) fn into_ok_cmd_result(
    r: Result<String, forktty_core::BrowserCmdError>,
) -> forktty_core::CmdResult {
    match r {
        Ok(_) => forktty_core::CmdResult::Ok,
        Err(e) => forktty_core::CmdResult::Err(e),
    }
}
