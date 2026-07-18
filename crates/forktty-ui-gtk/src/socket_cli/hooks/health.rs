//! Complete managed-hook installation validation for `forktty hooks doctor`.

use super::install::{
    build_hook_setup_plan, build_hook_setup_plan_with_profile, is_effectively_executable_file,
    read_regular_managed_script,
};
use super::{read_claude_installed_profile, AgentSpec, HookSetupProfile};
use serde::{Serialize, Serializer};
use serde_json::{json, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallationStatus {
    Ok,
    Problems,
}

impl InstallationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Problems => "problems",
        }
    }
}

impl Serialize for InstallationStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallationIssueCode {
    ConfigNotRegular,
    ConfigMissing,
    ConfigUnreadable,
    LauncherMissing,
    LauncherUnusable,
    ManagedAssetsInvalid,
    ConfigPathMismatch,
    ManagedAssetsIncompleteOrModified,
    ScriptMissing,
    ScriptUnreadable,
    ScriptNotRegular,
    ScriptNotExecutable,
    ScriptModified,
}

impl InstallationIssueCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::ConfigNotRegular => "config_not_regular",
            Self::ConfigMissing => "config_missing",
            Self::ConfigUnreadable => "config_unreadable",
            Self::LauncherMissing => "launcher_missing",
            Self::LauncherUnusable => "launcher_unusable",
            Self::ManagedAssetsInvalid => "managed_assets_invalid",
            Self::ConfigPathMismatch => "config_path_mismatch",
            Self::ManagedAssetsIncompleteOrModified => "managed_assets_incomplete_or_modified",
            Self::ScriptMissing => "script_missing",
            Self::ScriptUnreadable => "script_unreadable",
            Self::ScriptNotRegular => "script_not_regular",
            Self::ScriptNotExecutable => "script_not_executable",
            Self::ScriptModified => "script_modified",
        }
    }
}

impl Serialize for InstallationIssueCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct InstallationIssue {
    code: InstallationIssueCode,
    #[serde(serialize_with = "serialize_issue_path")]
    path: PathBuf,
    message: String,
}

// Doctor-v1 paths are JSON strings: preserve valid UTF-8 exactly and reject
// non-UTF-8 paths rather than changing their bytes through lossy conversion.
fn serialize_issue_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let path = path
        .to_str()
        .ok_or_else(|| serde::ser::Error::custom("installation issue path is not valid UTF-8"))?;
    serializer.serialize_str(path)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InstallationCheck {
    issues: Vec<InstallationIssue>,
}

#[derive(Serialize)]
struct InstallationCheckWire<'a> {
    status: InstallationStatus,
    ok: bool,
    issues: &'a [InstallationIssue],
}

impl Serialize for InstallationCheck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        InstallationCheckWire {
            status: self.status(),
            ok: self.is_healthy(),
            issues: &self.issues,
        }
        .serialize(serializer)
    }
}

impl InstallationCheck {
    fn from_issues(issues: Vec<InstallationIssue>) -> Self {
        Self { issues }
    }

    fn status(&self) -> InstallationStatus {
        if self.is_healthy() {
            InstallationStatus::Ok
        } else {
            InstallationStatus::Problems
        }
    }

    fn is_healthy(&self) -> bool {
        self.issues.is_empty()
    }
}

fn issue(
    code: InstallationIssueCode,
    path: &Path,
    message: impl Into<String>,
) -> InstallationIssue {
    InstallationIssue {
        code,
        path: path.to_path_buf(),
        message: message.into(),
    }
}

pub(super) fn describe_installation_check(
    spec: &'static AgentSpec,
    config_path: &Path,
    installed_launcher: Option<&Path>,
) -> InstallationCheck {
    let mut issues = Vec::new();
    match fs::metadata(config_path) {
        Ok(metadata) if !metadata.file_type().is_file() => issues.push(issue(
            InstallationIssueCode::ConfigNotRegular,
            config_path,
            "managed hook config is not a regular file",
        )),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => issues.push(issue(
            InstallationIssueCode::ConfigMissing,
            config_path,
            "managed hook config is missing",
        )),
        Err(error) => issues.push(issue(
            InstallationIssueCode::ConfigUnreadable,
            config_path,
            format!("managed hook config cannot be inspected: {error}"),
        )),
    }

    let Some(installed_launcher) = installed_launcher else {
        issues.push(issue(
            InstallationIssueCode::LauncherMissing,
            config_path,
            "no managed launcher could be recovered from the installed assets",
        ));
        return installation_result(issues);
    };
    if !is_effectively_executable_file(installed_launcher) {
        issues.push(issue(
            InstallationIssueCode::LauncherUnusable,
            installed_launcher,
            "the launcher recorded by the managed assets is not executable",
        ));
    }

    let plan = if spec.key == "claude"
        && matches!(
            read_claude_installed_profile(config_path),
            Ok(Some(HookSetupProfile::Full))
        ) {
        build_hook_setup_plan_with_profile(spec, installed_launcher, HookSetupProfile::Full)
    } else {
        build_hook_setup_plan(spec, installed_launcher)
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => {
            issues.push(issue(
                InstallationIssueCode::ManagedAssetsInvalid,
                config_path,
                error.message,
            ));
            return installation_result(issues);
        }
    };

    if plan.config_path != config_path {
        issues.push(issue(
            InstallationIssueCode::ConfigPathMismatch,
            config_path,
            format!(
                "managed hook plan resolved a different config path: {}",
                plan.config_path.display()
            ),
        ));
    }
    if plan.changed
        && !issues
            .iter()
            .any(|entry| entry.code == InstallationIssueCode::ConfigMissing)
    {
        issues.push(issue(
            InstallationIssueCode::ManagedAssetsIncompleteOrModified,
            config_path,
            "managed hook assets differ from the generated installation plan",
        ));
    }

    for (script_path, expected_content) in plan.scripts {
        let metadata = match fs::symlink_metadata(&script_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                issues.push(issue(
                    InstallationIssueCode::ScriptMissing,
                    &script_path,
                    "managed hook script is missing",
                ));
                continue;
            }
            Err(error) => {
                issues.push(issue(
                    InstallationIssueCode::ScriptUnreadable,
                    &script_path,
                    format!("managed hook script cannot be inspected: {error}"),
                ));
                continue;
            }
        };
        if !metadata.file_type().is_file() {
            issues.push(issue(
                InstallationIssueCode::ScriptNotRegular,
                &script_path,
                "managed hook script is not a regular file",
            ));
            continue;
        }
        if !is_effectively_executable_file(&script_path) {
            issues.push(issue(
                InstallationIssueCode::ScriptNotExecutable,
                &script_path,
                "managed hook script is not executable",
            ));
        }
        match read_regular_managed_script(&script_path) {
            Some(actual_content) if actual_content == expected_content => {}
            Some(_) => issues.push(issue(
                InstallationIssueCode::ScriptModified,
                &script_path,
                "managed hook script differs from generated content",
            )),
            None => issues.push(issue(
                InstallationIssueCode::ScriptUnreadable,
                &script_path,
                "managed hook script cannot be read safely",
            )),
        }
    }

    installation_result(issues)
}

fn installation_result(issues: Vec<InstallationIssue>) -> InstallationCheck {
    InstallationCheck::from_issues(issues)
}

pub(super) fn hook_doctor_report_is_healthy(
    report: &Value,
    installation_check: &InstallationCheck,
) -> bool {
    report["socket"]["inspect"]["exists"] == json!(true)
        && report["socket"]["inspect"]["readable"] == json!(true)
        && report["socket"]["inspect"]["writable"] == json!(true)
        && report["hookConfig"]["exists"] == json!(true)
        && report["launcherCheck"]["status"] == json!("ok")
        && installation_check.is_healthy()
}

pub(super) fn format_installation_check(check: &InstallationCheck) -> String {
    if check.is_healthy() {
        return "managed installation: ok".to_string();
    }
    let issues = check
        .issues
        .iter()
        .map(|issue| issue.code.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("managed installation: problems ({issues})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket_cli::hooks::install::{agent_spec, HookSetupPlan};
    use crate::test_env::with_env;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::Duration;

    fn executable_launcher(root: &Path) -> PathBuf {
        let launcher = root.join("forktty");
        fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        launcher
    }

    fn install_setup_plan(plan: &HookSetupPlan) {
        fs::create_dir_all(plan.config_path.parent().unwrap()).unwrap();
        fs::write(&plan.config_path, &plan.content).unwrap();
        for (path, content) in &plan.scripts {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn issue_codes(check: &InstallationCheck) -> Vec<InstallationIssueCode> {
        check.issues.iter().map(|issue| issue.code).collect()
    }

    #[test]
    fn installation_check_is_typed_and_preserves_doctor_v1_json_shape() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let spec = agent_spec("antigravity").unwrap();
            let config_path = (spec.config_path)();

            let check = describe_installation_check(spec, &config_path, None);

            assert_eq!(check.status(), InstallationStatus::Problems);
            assert!(!check.is_healthy());
            assert_eq!(
                check
                    .issues
                    .iter()
                    .map(|issue| issue.code)
                    .collect::<Vec<_>>(),
                vec![
                    InstallationIssueCode::ConfigMissing,
                    InstallationIssueCode::LauncherMissing,
                ]
            );
            assert_eq!(
                format_installation_check(&check),
                "managed installation: problems (config_missing, launcher_missing)"
            );
            assert_eq!(
                serde_json::to_value(&check).unwrap(),
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
        });
    }

    #[test]
    fn installation_check_rejects_non_utf8_issue_path_during_serialization() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/forktty-\xff".to_vec()));
        let check = InstallationCheck::from_issues(vec![issue(
            InstallationIssueCode::ConfigMissing,
            &path,
            "managed hook config is missing",
        )]);

        let error = serde_json::to_value(&check).unwrap_err();

        assert_eq!(
            error.to_string(),
            "installation issue path is not valid UTF-8"
        );
    }

    #[test]
    fn installation_issue_codes_preserve_doctor_v1_strings() {
        let cases = [
            (
                InstallationIssueCode::ConfigNotRegular,
                "config_not_regular",
            ),
            (InstallationIssueCode::ConfigMissing, "config_missing"),
            (InstallationIssueCode::ConfigUnreadable, "config_unreadable"),
            (InstallationIssueCode::LauncherMissing, "launcher_missing"),
            (InstallationIssueCode::LauncherUnusable, "launcher_unusable"),
            (
                InstallationIssueCode::ManagedAssetsInvalid,
                "managed_assets_invalid",
            ),
            (
                InstallationIssueCode::ConfigPathMismatch,
                "config_path_mismatch",
            ),
            (
                InstallationIssueCode::ManagedAssetsIncompleteOrModified,
                "managed_assets_incomplete_or_modified",
            ),
            (InstallationIssueCode::ScriptMissing, "script_missing"),
            (InstallationIssueCode::ScriptUnreadable, "script_unreadable"),
            (
                InstallationIssueCode::ScriptNotRegular,
                "script_not_regular",
            ),
            (
                InstallationIssueCode::ScriptNotExecutable,
                "script_not_executable",
            ),
            (InstallationIssueCode::ScriptModified, "script_modified"),
        ];

        for (code, expected) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(expected));
        }
    }

    #[test]
    fn installation_check_antigravity_rejects_wrapper_only_installation() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            for (path, content) in &plan.scripts {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, content).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ConfigMissing));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_malformed_config() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::write(&plan.config_path, "{not-json").unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ManagedAssetsInvalid));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_missing_managed_group() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::write(&plan.config_path, "{\"foreign\": {}}\n").unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check)
                .contains(&InstallationIssueCode::ManagedAssetsIncompleteOrModified));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_partial_scripts() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::remove_file(&plan.scripts[0].0).unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ScriptMissing));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_non_executable_script() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::set_permissions(&plan.scripts[0].0, fs::Permissions::from_mode(0o644)).unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ScriptNotExecutable));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_effectively_non_executable_script() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::set_permissions(&plan.scripts[0].0, fs::Permissions::from_mode(0o410)).unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ScriptNotExecutable));
        });
    }

    #[test]
    fn installation_check_rejects_effectively_non_executable_launcher() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::set_permissions(&launcher, fs::Permissions::from_mode(0o410)).unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::LauncherUnusable));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_fifo_wrapper_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            let fifo = plan.scripts[0].0.clone();
            fs::remove_file(&fifo).unwrap();
            let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: `c_path` is a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

            let config_path = plan.config_path.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            let probe = thread::spawn(move || {
                sender
                    .send(describe_installation_check(
                        spec,
                        &config_path,
                        Some(&launcher),
                    ))
                    .ok();
            });
            let check = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("installation check must not block on a FIFO wrapper");
            probe.join().unwrap();

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ScriptNotRegular));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_non_regular_script() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::remove_file(&plan.scripts[0].0).unwrap();
            std::os::unix::fs::symlink(&launcher, &plan.scripts[0].0).unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ScriptNotRegular));
        });
    }

    #[test]
    fn installation_check_antigravity_rejects_modified_script() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);
            fs::write(&plan.scripts[0].0, "#!/bin/sh\nexit 7\n").unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(!check.is_healthy());
            assert!(issue_codes(&check).contains(&InstallationIssueCode::ScriptModified));
        });
    }

    #[test]
    fn installation_check_antigravity_accepts_complete_managed_installation() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let launcher = executable_launcher(root.path());
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            install_setup_plan(&plan);

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert_eq!(check.status(), InstallationStatus::Ok);
            assert!(check.is_healthy());
            assert!(check.issues.is_empty());
            assert_eq!(
                serde_json::to_value(&check).unwrap(),
                json!({"status": "ok", "ok": true, "issues": []})
            );
        });
    }

    #[test]
    fn installation_check_rejects_incomplete_claude_codex_and_opencode_assets() {
        let root = tempfile::tempdir().unwrap();
        let launcher = executable_launcher(root.path());
        let codex_home = root.path().join("codex").display().to_string();
        let claude_home = root.path().join("claude").display().to_string();
        let opencode_home = root.path().join("opencode").display().to_string();
        let home = root.path().join("home").display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home)),
                ("CLAUDE_CONFIG_DIR", Some(&claude_home)),
                ("OPENCODE_CONFIG_DIR", Some(&opencode_home)),
                ("HOME", Some(&home)),
            ],
            || {
                for key in ["claude", "codex", "opencode"] {
                    let spec = agent_spec(key).unwrap();
                    let plan = build_hook_setup_plan(spec, &launcher).unwrap();
                    install_setup_plan(&plan);
                    fs::write(
                        &plan.config_path,
                        if key == "opencode" {
                            "// partial\n"
                        } else {
                            "{}\n"
                        },
                    )
                    .unwrap();

                    let check =
                        describe_installation_check(spec, &plan.config_path, Some(&launcher));

                    assert!(!check.is_healthy(), "{key}");
                    let expected_issue = if key == "opencode" {
                        InstallationIssueCode::ManagedAssetsInvalid
                    } else {
                        InstallationIssueCode::ManagedAssetsIncompleteOrModified
                    };
                    assert!(issue_codes(&check).contains(&expected_issue), "{key}");
                }
            },
        );
    }

    #[test]
    fn installation_check_accepts_supported_symlinked_json_config() {
        let root = tempfile::tempdir().unwrap();
        let launcher = executable_launcher(root.path());
        let claude_home = root.path().join("claude");
        let claude_home_s = claude_home.display().to_string();
        with_env(&[("CLAUDE_CONFIG_DIR", Some(&claude_home_s))], || {
            let spec = agent_spec("claude").unwrap();
            let plan = build_hook_setup_plan(spec, &launcher).unwrap();
            let target = root.path().join("real-settings.json");
            fs::create_dir_all(plan.config_path.parent().unwrap()).unwrap();
            fs::write(&target, &plan.content).unwrap();
            std::os::unix::fs::symlink(&target, &plan.config_path).unwrap();

            let check = describe_installation_check(spec, &plan.config_path, Some(&launcher));

            assert!(check.is_healthy());
        });
    }

    #[test]
    fn installation_check_is_required_for_doctor_health() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let home_s = home.display().to_string();
        with_env(&[("HOME", Some(&home_s))], || {
            let spec = agent_spec("antigravity").unwrap();
            let config_path = (spec.config_path)();
            let installation_check = describe_installation_check(spec, &config_path, None);
            let report = json!({
                "socket": {"inspect": {"exists": true, "readable": true, "writable": true}},
                "hookConfig": {"exists": true},
                "launcherCheck": {"status": "ok"},
                "installationCheck": {"status": "ok", "ok": true, "issues": []},
            });

            assert!(!hook_doctor_report_is_healthy(&report, &installation_check));
        });
    }
}
