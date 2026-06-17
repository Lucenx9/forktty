use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::command_safety::is_shell_trampoline;

const PROJECT_ACTIONS_FILE: &str = "forktty.json";
const MAX_PROJECT_ACTION_FILE_BYTES: u64 = 256 * 1024;
const MAX_PROJECT_ACTIONS: usize = 64;
const MAX_ACTION_ID_BYTES: usize = 80;
const MAX_ACTION_LABEL_BYTES: usize = 120;
const MAX_ACTION_DESCRIPTION_BYTES: usize = 512;
const MAX_ACTION_ARGS: usize = 64;
const MAX_ACTION_ARG_BYTES: usize = 4096;
const MAX_ACTION_ARGV_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
pub enum ProjectActionError {
    #[error("Project action IO error: {0}")]
    Io(String),
    #[error("Project action JSON error: {0}")]
    Json(String),
    #[error("Invalid project action: {0}")]
    Invalid(String),
    #[error("Project action not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectAction {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectActionsFile {
    #[serde(default)]
    actions: Vec<RawProjectAction>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectAction {
    id: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    argv: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
}

pub fn discover_project_root(cwd: impl AsRef<Path>) -> Result<PathBuf, ProjectActionError> {
    let repo = git2::Repository::discover(cwd.as_ref())
        .map_err(|_| ProjectActionError::Invalid("cwd must be inside a git repository".into()))?;
    let Some(workdir) = repo.workdir() else {
        return Err(ProjectActionError::Invalid(
            "project actions require a non-bare git worktree".into(),
        ));
    };
    std::fs::canonicalize(workdir).map_err(|err| ProjectActionError::Io(err.to_string()))
}

pub fn load_project_actions(
    project_root: impl AsRef<Path>,
) -> Result<Vec<ProjectAction>, ProjectActionError> {
    let project_root = project_root.as_ref();
    let path = project_root.join(PROJECT_ACTIONS_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(ProjectActionError::Io(err.to_string())),
    };
    if !metadata.is_file() {
        return Err(ProjectActionError::Invalid(format!(
            "{PROJECT_ACTIONS_FILE} must be a regular file"
        )));
    }
    if metadata.len() > MAX_PROJECT_ACTION_FILE_BYTES {
        return Err(ProjectActionError::Invalid(format!(
            "{PROJECT_ACTIONS_FILE} is too large"
        )));
    }
    let bytes = std::fs::read(&path).map_err(|err| ProjectActionError::Io(err.to_string()))?;
    let raw = serde_json::from_slice::<ProjectActionsFile>(&bytes)
        .map_err(|err| ProjectActionError::Json(err.to_string()))?;
    if raw.actions.len() > MAX_PROJECT_ACTIONS {
        return Err(ProjectActionError::Invalid(format!(
            "actions is limited to {MAX_PROJECT_ACTIONS} entries"
        )));
    }
    let mut ids = BTreeSet::new();
    raw.actions
        .into_iter()
        .map(validate_action)
        .map(|action| {
            let action = action?;
            if !ids.insert(action.id.clone()) {
                return Err(ProjectActionError::Invalid(format!(
                    "duplicate action id: {}",
                    action.id
                )));
            }
            Ok(action)
        })
        .collect()
}

pub fn action_cwd(
    project_root: impl AsRef<Path>,
    action: &ProjectAction,
) -> Result<PathBuf, ProjectActionError> {
    let root = std::fs::canonicalize(project_root.as_ref())
        .map_err(|err| ProjectActionError::Io(err.to_string()))?;
    let cwd = root.join(&action.cwd);
    let cwd = std::fs::canonicalize(&cwd).map_err(|err| {
        ProjectActionError::Invalid(format!("action cwd '{}' is invalid: {err}", action.cwd))
    })?;
    if !cwd.starts_with(&root) {
        return Err(ProjectActionError::Invalid(format!(
            "action cwd '{}' escapes the project root",
            action.cwd
        )));
    }
    if !cwd.is_dir() {
        return Err(ProjectActionError::Invalid(format!(
            "action cwd '{}' is not a directory",
            action.cwd
        )));
    }
    Ok(cwd)
}

pub fn find_action(
    actions: &[ProjectAction],
    id: &str,
) -> Result<ProjectAction, ProjectActionError> {
    actions
        .iter()
        .find(|action| action.id == id)
        .cloned()
        .ok_or_else(|| ProjectActionError::NotFound(id.to_string()))
}

fn validate_action(raw: RawProjectAction) -> Result<ProjectAction, ProjectActionError> {
    let id = validate_id(raw.id)?;
    let label = validate_text("label", raw.label, MAX_ACTION_LABEL_BYTES)?;
    let description = raw
        .description
        .map(|description| {
            validate_optional_text("description", description, MAX_ACTION_DESCRIPTION_BYTES)
        })
        .transpose()?;
    let cwd = validate_cwd(raw.cwd.unwrap_or_else(|| ".".to_string()))?;
    let argv = validate_argv(raw.argv)?;
    Ok(ProjectAction {
        id,
        label,
        description,
        argv,
        cwd,
    })
}

fn validate_id(value: String) -> Result<String, ProjectActionError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ProjectActionError::Invalid("id must not be empty".into()));
    }
    if value.len() > MAX_ACTION_ID_BYTES {
        return Err(ProjectActionError::Invalid(format!(
            "id is limited to {MAX_ACTION_ID_BYTES} bytes"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(ProjectActionError::Invalid(
            "id may contain only ASCII letters, digits, '.', '-', '_', or ':'".into(),
        ));
    }
    Ok(value.to_string())
}

fn validate_text(
    field: &str,
    value: String,
    max_bytes: usize,
) -> Result<String, ProjectActionError> {
    let value = value.trim();
    validate_optional_text(field, value.to_string(), max_bytes).and_then(|value| {
        if value.is_empty() {
            Err(ProjectActionError::Invalid(format!(
                "{field} must not be empty"
            )))
        } else {
            Ok(value)
        }
    })
}

fn validate_optional_text(
    field: &str,
    value: String,
    max_bytes: usize,
) -> Result<String, ProjectActionError> {
    if value.len() > max_bytes {
        return Err(ProjectActionError::Invalid(format!(
            "{field} is limited to {max_bytes} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(ProjectActionError::Invalid(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(value)
}

fn validate_cwd(value: String) -> Result<String, ProjectActionError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ProjectActionError::Invalid("cwd must not be empty".into()));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(ProjectActionError::Invalid(
            "cwd must be relative to the project root".into(),
        ));
    }
    for component in path.components() {
        match component {
            Component::CurDir | Component::Normal(_) => {}
            Component::ParentDir => {
                return Err(ProjectActionError::Invalid(
                    "cwd must not contain parent-directory components".into(),
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ProjectActionError::Invalid(
                    "cwd must be relative to the project root".into(),
                ));
            }
        }
    }
    if value.chars().any(char::is_control) {
        return Err(ProjectActionError::Invalid(
            "cwd must not contain control characters".into(),
        ));
    }
    Ok(value.to_string())
}

fn validate_argv(argv: Vec<String>) -> Result<Vec<String>, ProjectActionError> {
    if argv.is_empty() {
        return Err(ProjectActionError::Invalid("argv must not be empty".into()));
    }
    if argv.len() > MAX_ACTION_ARGS {
        return Err(ProjectActionError::Invalid(format!(
            "argv is limited to {MAX_ACTION_ARGS} entries"
        )));
    }
    let total_bytes = argv.iter().map(String::len).sum::<usize>();
    if total_bytes > MAX_ACTION_ARGV_BYTES {
        return Err(ProjectActionError::Invalid(format!(
            "argv is limited to {MAX_ACTION_ARGV_BYTES} bytes"
        )));
    }
    for (index, arg) in argv.iter().enumerate() {
        if arg.is_empty() {
            return Err(ProjectActionError::Invalid(format!(
                "argv[{index}] must not be empty"
            )));
        }
        if arg.len() > MAX_ACTION_ARG_BYTES {
            return Err(ProjectActionError::Invalid(format!(
                "argv[{index}] is limited to {MAX_ACTION_ARG_BYTES} bytes"
            )));
        }
        if arg.chars().any(char::is_control) {
            return Err(ProjectActionError::Invalid(format!(
                "argv[{index}] must not contain control characters"
            )));
        }
    }
    if argv[0].starts_with('-') {
        return Err(ProjectActionError::Invalid(
            "argv[0] must not start with '-'".into(),
        ));
    }
    if is_shell_trampoline(&argv[0], &argv[1..]) {
        return Err(ProjectActionError::Invalid(
            "argv must not invoke a shell command string".into(),
        ));
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_actions(dir: &Path, json: &str) {
        std::fs::write(dir.join(PROJECT_ACTIONS_FILE), json).unwrap();
    }

    #[test]
    fn loads_valid_project_actions() {
        let dir = tempfile::tempdir().unwrap();
        write_actions(
            dir.path(),
            r#"{"actions":[{"id":"test","label":"Run tests","argv":["cargo","test"],"cwd":"."}]}"#,
        );

        let actions = load_project_actions(dir.path()).unwrap();

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "test");
        assert_eq!(actions[0].argv, ["cargo", "test"]);
    }

    #[test]
    fn rejects_shell_trampoline_actions() {
        let dir = tempfile::tempdir().unwrap();
        write_actions(
            dir.path(),
            r#"{"actions":[{"id":"bad","label":"Bad","argv":["bash","-lc","rm -rf ."]}]}"#,
        );

        let err = load_project_actions(dir.path()).unwrap_err().to_string();

        assert!(err.contains("shell command string"));
    }

    #[test]
    fn rejects_duplicate_action_ids() {
        let dir = tempfile::tempdir().unwrap();
        write_actions(
            dir.path(),
            r#"{"actions":[
                {"id":"test","label":"One","argv":["cargo","test"]},
                {"id":"test","label":"Two","argv":["cargo","check"]}
            ]}"#,
        );

        let err = load_project_actions(dir.path()).unwrap_err().to_string();

        assert!(err.contains("duplicate action id"));
    }

    #[test]
    fn action_cwd_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        let action = ProjectAction {
            id: "x".into(),
            label: "X".into(),
            description: None,
            argv: vec!["cargo".into(), "test".into()],
            cwd: "escape".into(),
        };

        let err = action_cwd(dir.path(), &action).unwrap_err().to_string();

        assert!(err.contains("escapes the project root"));
    }
}
