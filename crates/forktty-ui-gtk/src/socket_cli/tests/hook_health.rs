//! Hook doctor CLI boundary and non-UTF-8 path regression tests.

use super::*;

const HOOK_DOCTOR_CHILD_ROOT: &str = "FORKTTY_TEST_HOOK_DOCTOR_CHILD_ROOT";
const HOOK_DOCTOR_NON_UTF8_CHILD: &str = "FORKTTY_TEST_HOOK_DOCTOR_NON_UTF8_CHILD";
const HOOK_DOCTOR_JSON_BEGIN: &str = "FORKTTY_HOOK_DOCTOR_JSON_BEGIN";
const HOOK_DOCTOR_JSON_END: &str = "FORKTTY_HOOK_DOCTOR_JSON_END";
const HOOK_DOCTOR_CONTROLLED_ERROR: &str = "FORKTTY_HOOK_DOCTOR_CONTROLLED_ERROR:";

#[test]
fn hook_doctor_output_child() {
    with_env(&[], || {
        let Some(root) = std::env::var_os(HOOK_DOCTOR_CHILD_ROOT) else {
            return;
        };
        let socket_path = PathBuf::from(root).join("runtime/forktty.sock");
        let json_context = CliContext {
            json: true,
            socket_path: socket_path.clone(),
            socket_explicit: true,
            verbose: false,
        };
        write_stdout_line(HOOK_DOCTOR_JSON_BEGIN).unwrap();
        let error = handle_hooks_doctor(&json_context, strings(&["antigravity"])).unwrap_err();
        assert_eq!(error.exit, 1);
        write_stdout_line(HOOK_DOCTOR_JSON_END).unwrap();

        let terminal_context = CliContext {
            json: false,
            socket_path,
            socket_explicit: true,
            verbose: false,
        };
        let error = handle_hooks_doctor(&terminal_context, strings(&["antigravity"])).unwrap_err();
        assert_eq!(error.exit, 1);
    });
}

#[test]
fn hook_doctor_reports_typed_installation_check_at_cli_boundary() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&runtime).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "socket_cli::tests::hook_health::hook_doctor_output_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOOK_DOCTOR_CHILD_ROOT, root.path())
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env_remove("APPIMAGE")
        .env_remove("APPDIR")
        .env_remove("FORKTTY_APPIMAGE")
        .env_remove("FORKTTY_APPIMAGE_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "hook doctor child failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let report_json = stdout
        .split_once(HOOK_DOCTOR_JSON_BEGIN)
        .expect("JSON begin marker")
        .1
        .split_once(HOOK_DOCTOR_JSON_END)
        .expect("JSON end marker")
        .0
        .trim();
    let report: Value = serde_json::from_str(report_json).unwrap();
    let config_path = home.join(".gemini/config/hooks.json");
    assert_eq!(report["version"], json!(1));
    assert_eq!(report["ok"], json!(false));
    assert_eq!(
        report["installationCheck"],
        json!({
            "status": "problems",
            "ok": false,
            "issues": [
                {
                    "code": "config_missing",
                    "path": config_path,
                    "message": "managed hook config is missing",
                },
                {
                    "code": "launcher_missing",
                    "path": config_path,
                    "message": "no managed launcher could be recovered from the installed assets",
                },
            ],
        })
    );

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("managed installation: problems (config_missing, launcher_missing)"));
}

#[test]
fn hook_doctor_non_utf8_home_child() {
    with_env(&[], || {
        if std::env::var_os(HOOK_DOCTOR_NON_UTF8_CHILD).is_none() {
            return;
        }
        let root = PathBuf::from(std::env::var_os(HOOK_DOCTOR_CHILD_ROOT).unwrap());
        let context = CliContext {
            json: true,
            socket_path: root.join("runtime/forktty.sock"),
            socket_explicit: true,
            verbose: false,
        };

        let error = handle_hooks_doctor(&context, strings(&["antigravity"])).unwrap_err();
        assert_eq!(error.exit, 1);
        eprintln!("{HOOK_DOCTOR_CONTROLLED_ERROR}{}", error.message);
    });
}

#[test]
fn hook_doctor_rejects_non_utf8_issue_paths_without_panicking() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let mut home = root.path().join("home").into_os_string().into_vec();
    home.extend_from_slice(b"-\xff");
    let home = PathBuf::from(std::ffi::OsString::from_vec(home));
    fs::create_dir_all(&home).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "socket_cli::tests::hook_health::hook_doctor_non_utf8_home_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOOK_DOCTOR_CHILD_ROOT, root.path())
        .env(HOOK_DOCTOR_NON_UTF8_CHILD, "1")
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env_remove("APPIMAGE")
        .env_remove("APPDIR")
        .env_remove("FORKTTY_APPIMAGE")
        .env_remove("FORKTTY_APPIMAGE_DIR")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "hook doctor must return a controlled error, not panic: stdout={} stderr={stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(stderr.contains(HOOK_DOCTOR_CONTROLLED_ERROR));
    assert!(stderr.contains("cannot encode installation check"));
    assert!(stderr.contains("installation issue path is not valid UTF-8"));
    assert!(!stderr.contains("panicked at"));
    assert!(!home.as_os_str().as_bytes().is_ascii());
}
