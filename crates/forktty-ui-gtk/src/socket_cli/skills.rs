//! Managed agent skill CLI setup, removal, status, and checksum helpers.

use super::hooks::home_dir;
use super::integration_files::{
    atomic_write_file, ensure_parent_dir, read_text_config, MAX_HOOK_CONFIG_SIZE_BYTES,
};
use super::{
    bool_option, inspect_path, next_file_nonce, parse_flags, print_json, reject_unknown_options,
    trimmed_env, write_stdout_line, CliContext, CliError, CliResult,
};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const AGENT_SKILL_NAME: &str = "forktty-agent-orchestration";
pub(super) const AGENT_SKILL_MARKER: &str = "<!-- forktty-managed-agent-skill -->";
// Source of truth: edit the files under .agents/skills/forktty-agent-orchestration/,
// then verify the embedded checksums with the final forktty binary.
pub(super) const AGENT_SKILL_MD: &str =
    include_str!("../../../../.agents/skills/forktty-agent-orchestration/SKILL.md");
pub(super) const AGENT_SKILL_OPENAI_YAML: &str =
    include_str!("../../../../.agents/skills/forktty-agent-orchestration/agents/openai.yaml");
const MAX_AGENT_SKILL_FILE_BYTES: u64 = MAX_HOOK_CONFIG_SIZE_BYTES;

#[derive(Clone, Copy)]
pub(super) struct SkillTargetSpec {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) skill_dir: fn() -> PathBuf,
}

pub(super) const SKILL_TARGETS: &[SkillTargetSpec] = &[
    SkillTargetSpec {
        key: "agents",
        label: "Agent Skills",
        skill_dir: agent_skills_dir,
    },
    SkillTargetSpec {
        key: "claude",
        label: "Claude",
        skill_dir: claude_skill_dir,
    },
];

const DEFAULT_SKILL_SETUP_TARGET_KEYS: &[&str] = &["agents", "claude"];
const MAX_AGENT_SKILL_BACKUPS: usize = 3;

pub(super) struct SkillSetupPlan {
    pub(super) spec: &'static SkillTargetSpec,
    pub(super) skill_dir: PathBuf,
    pub(super) changed: bool,
    pub(super) status: &'static str,
    pub(super) source_checksum: String,
    pub(super) installed_checksum: String,
    pub(super) repair_command: Option<String>,
    pub(super) files: Vec<(PathBuf, String)>,
}

struct SkillRemovePlan {
    spec: &'static SkillTargetSpec,
    skill_dir: PathBuf,
    changed: bool,
}

pub(super) fn handle_skills(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("setup") | Some("install") => handle_skills_setup(context, args[1..].to_vec()),
        Some("remove") | Some("uninstall") => handle_skills_remove(context, args[1..].to_vec()),
        Some("help") | Some("--help") | Some("-h") => write_stdout_line(
            "Usage: forktty skills setup [agents|codex|pi|claude] | forktty skills remove [agents|codex|pi|claude]\nDefault setup targets: agents, claude. codex and pi alias the interoperable agents target.",
        ),
        Some(other) => Err(CliError::new(format!("skills: unknown subcommand {other}"))),
        None => Err(CliError::new(
            "skills requires a subcommand: setup or remove",
        )),
    }
}

pub(super) fn handle_skills_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "skills setup")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "skills setup: --dry-run must be true or false",
        ));
    };
    let targets = if parsed.positionals.is_empty() {
        default_skill_setup_targets()
    } else {
        supported_skill_targets(&parsed.positionals)?
    };

    let mut plans = Vec::new();
    for target in targets {
        plans.push(build_skill_setup_plan(target)?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        let mut removed_backups = Vec::new();
        if plan.changed && !dry_run {
            backup_path = backup_skill_dir(&plan.skill_dir)?;
            for (path, content) in &plan.files {
                ensure_parent_dir(path)?;
                atomic_write_file(path, content.as_bytes())?;
            }
        }
        if !dry_run {
            removed_backups = prune_managed_skill_backups(&plan.skill_dir)?;
        }
        summaries.push(skill_setup_summary(
            &plan,
            dry_run,
            backup_path,
            removed_backups,
        ));
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let target = summary["label"]
            .as_str()
            .unwrap_or_else(|| summary["target"].as_str().unwrap_or("target"));
        let skill_dir = summary["skillDir"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would install"
        } else if changed {
            "installed"
        } else {
            "already installed"
        };
        write_stdout_line(&format!("{target}: {verb} ForkTTY skill at {skill_dir}"))?;
        if let Some(status) = summary["status"].as_str() {
            write_stdout_line(&format!("  status: {status}"))?;
        }
        if let Some(repair) = summary["repairCommand"].as_str() {
            write_stdout_line(&format!("  repair: {repair}"))?;
        }
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
        if let Some(removed) = summary["removedBackups"].as_array() {
            for backup in removed.iter().filter_map(Value::as_str) {
                write_stdout_line(&format!("  removed backup: {backup}"))?;
            }
        }
    }
    Ok(())
}

pub(super) fn handle_skills_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "skills remove")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "skills remove: --dry-run must be true or false",
        ));
    };
    let targets = supported_skill_targets(&parsed.positionals)?;

    let mut plans = Vec::new();
    for target in targets {
        plans.push(build_skill_remove_plan(target)?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        let mut removed_backups = Vec::new();
        if plan.changed && !dry_run {
            backup_path = backup_skill_dir(&plan.skill_dir)?;
        }
        if !dry_run {
            removed_backups = prune_managed_skill_backups(&plan.skill_dir)?;
        }
        summaries.push(json!({
            "target": plan.spec.key,
            "agent": plan.spec.key,
            "label": plan.spec.label,
            "skillDir": plan.skill_dir,
            "configPath": plan.skill_dir,
            "changed": plan.changed,
            "backupPath": backup_path,
            "removedBackups": removed_backups,
            "dryRun": dry_run,
        }));
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let target = summary["label"]
            .as_str()
            .unwrap_or_else(|| summary["target"].as_str().unwrap_or("target"));
        let skill_dir = summary["skillDir"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would remove"
        } else if changed {
            "removed"
        } else {
            "not installed"
        };
        write_stdout_line(&format!("{target}: {verb} ForkTTY skill at {skill_dir}"))?;
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
        if let Some(removed) = summary["removedBackups"].as_array() {
            for backup in removed.iter().filter_map(Value::as_str) {
                write_stdout_line(&format!("  removed backup: {backup}"))?;
            }
        }
    }
    Ok(())
}

pub(super) fn build_skill_setup_plan(spec: &'static SkillTargetSpec) -> CliResult<SkillSetupPlan> {
    let skill_dir = (spec.skill_dir)();
    let skill_path = skill_dir.join("SKILL.md");
    let metadata_path = skill_dir.join("agents").join("openai.yaml");
    let files = vec![
        (skill_path, AGENT_SKILL_MD.to_string()),
        (metadata_path, AGENT_SKILL_OPENAI_YAML.to_string()),
    ];
    let source_checksum = skill_content_checksum(AGENT_SKILL_MD, Some(AGENT_SKILL_OPENAI_YAML));
    let (status, installed_checksum, _) =
        skill_target_status(&skill_dir, &source_checksum, spec.key);
    if status == "unmanaged" {
        return Err(CliError::new(format!(
            "skills: refusing to overwrite unmanaged skill at {}",
            skill_dir.display()
        )));
    }
    if status == "invalid" && installed_checksum.is_none() {
        return Err(CliError::new(format!(
            "skills: refusing to overwrite invalid skill at {} because the ForkTTY-managed marker could not be verified",
            skill_dir.display()
        )));
    }
    let changed = status != "up_to_date";
    Ok(SkillSetupPlan {
        spec,
        skill_dir,
        changed,
        status,
        source_checksum,
        installed_checksum: installed_checksum.unwrap_or_default(),
        repair_command: changed.then(|| format!("forktty skills setup {}", spec.key)),
        files,
    })
}

pub(super) fn skill_setup_summary(
    plan: &SkillSetupPlan,
    dry_run: bool,
    backup_path: Option<PathBuf>,
    removed_backups: Vec<PathBuf>,
) -> Value {
    let applied = plan.changed && !dry_run;
    let installed_checksum = if applied {
        Value::String(plan.source_checksum.clone())
    } else if plan.installed_checksum.is_empty() {
        Value::Null
    } else {
        Value::String(plan.installed_checksum.clone())
    };
    json!({
        "target": plan.spec.key,
        "agent": plan.spec.key,
        "label": plan.spec.label,
        "skillDir": plan.skill_dir,
        "configPath": plan.skill_dir,
        "changed": plan.changed,
        "status": if applied { "up_to_date" } else { plan.status },
        "sourceChecksum": plan.source_checksum,
        "installedChecksum": installed_checksum,
        "repairCommand": if applied { None } else { plan.repair_command.clone() },
        "backupPath": backup_path,
        "removedBackups": removed_backups,
        "dryRun": dry_run,
    })
}

pub(super) fn inspect_skill_target(spec: &'static SkillTargetSpec) -> Value {
    let skill_dir = (spec.skill_dir)();
    let mut value = inspect_path(&skill_dir);
    let source_checksum = skill_content_checksum(AGENT_SKILL_MD, Some(AGENT_SKILL_OPENAI_YAML));
    let (status, installed_checksum, repair_command) =
        skill_target_status(&skill_dir, &source_checksum, spec.key);
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_string(), Value::String(status.to_string()));
        object.insert("sourceChecksum".to_string(), Value::String(source_checksum));
        object.insert(
            "installedChecksum".to_string(),
            installed_checksum.map(Value::String).unwrap_or(Value::Null),
        );
        object.insert(
            "repairCommand".to_string(),
            repair_command.map(Value::String).unwrap_or(Value::Null),
        );
    }
    value
}

fn skill_target_status(
    skill_dir: &Path,
    source_checksum: &str,
    target: &str,
) -> (&'static str, Option<String>, Option<String>) {
    let repair = || Some(format!("forktty skills setup {target}"));
    let unrepairable_invalid = || ("invalid", None, None);
    let meta = match fs::symlink_metadata(skill_dir) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return ("missing", None, repair());
        }
        Err(_) => return unrepairable_invalid(),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return unrepairable_invalid();
    }
    let skill_path = skill_dir.join("SKILL.md");
    let skill = match read_managed_skill_file_for_doctor(&skill_path, "agent skill") {
        Ok(Some(skill)) => skill,
        Ok(None) | Err(_) => return unrepairable_invalid(),
    };
    if !skill.contains(AGENT_SKILL_MARKER) {
        return (
            "unmanaged",
            Some(skill_content_checksum(&skill, None)),
            None,
        );
    }
    let metadata_dir = skill_dir.join("agents");
    if let Err(_err) = validate_managed_skill_metadata_dir_for_doctor(&metadata_dir) {
        return (
            "invalid",
            Some(skill_content_checksum(&skill, None)),
            repair(),
        );
    }
    let metadata_path = metadata_dir.join("openai.yaml");
    let metadata = match read_managed_skill_file_for_doctor(&metadata_path, "agent skill metadata")
    {
        Ok(metadata) => metadata,
        Err(_) => {
            return (
                "invalid",
                Some(skill_content_checksum(&skill, None)),
                repair(),
            );
        }
    };
    let installed_checksum = skill_content_checksum(&skill, metadata.as_deref());
    if installed_checksum == source_checksum {
        ("up_to_date", Some(installed_checksum), None)
    } else {
        ("update_available", Some(installed_checksum), repair())
    }
}

fn validate_managed_skill_metadata_dir_for_doctor(path: &Path) -> CliResult<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(CliError::new(format!(
            "agent skill metadata directory exists but is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn read_managed_skill_file_for_doctor(path: &Path, label: &str) -> CliResult<Option<String>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    let file = File::open(path)?;
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    if stat.len() > MAX_AGENT_SKILL_FILE_BYTES {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            stat.len(),
            MAX_AGENT_SKILL_FILE_BYTES
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(MAX_AGENT_SKILL_FILE_BYTES + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > MAX_AGENT_SKILL_FILE_BYTES {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            text.len(),
            MAX_AGENT_SKILL_FILE_BYTES
        )));
    }
    Ok(Some(text))
}

fn skill_content_checksum(skill: &str, metadata: Option<&str>) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    feed(&mut hash, b"SKILL.md\0");
    feed(&mut hash, skill.as_bytes());
    feed(&mut hash, b"\0agents/openai.yaml\0");
    if let Some(metadata) = metadata {
        feed(&mut hash, metadata.as_bytes());
    }
    format!("fnv1a64:{hash:016x}")
}

fn build_skill_remove_plan(spec: &'static SkillTargetSpec) -> CliResult<SkillRemovePlan> {
    let skill_dir = (spec.skill_dir)();
    let changed = read_managed_skill_dir(&skill_dir)?.is_some();
    Ok(SkillRemovePlan {
        spec,
        skill_dir,
        changed,
    })
}

pub(super) fn supported_skill_targets(
    names: &[String],
) -> CliResult<Vec<&'static SkillTargetSpec>> {
    if names.is_empty() {
        return Ok(SKILL_TARGETS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = normalize_skill_target_name(name);
        let spec = skill_target_spec(&normalized)
            .ok_or_else(|| CliError::new(format!("Unsupported skills target: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&SkillTargetSpec| existing.key == spec.key)
        {
            out.push(spec);
        }
    }
    Ok(out)
}

fn default_skill_setup_targets() -> Vec<&'static SkillTargetSpec> {
    DEFAULT_SKILL_SETUP_TARGET_KEYS
        .iter()
        .map(|key| skill_target_spec(key).expect("default skill setup target exists"))
        .collect()
}

fn skill_target_spec(target: &str) -> Option<&'static SkillTargetSpec> {
    SKILL_TARGETS.iter().find(|spec| spec.key == target)
}

fn normalize_skill_target_name(target: &str) -> String {
    match target.to_lowercase().as_str() {
        "agent" | "agents" | "agent-skills" | "agent_skills" | "open-agent" | "open_agent"
        | "openagents" | "codex" | "pi" | "pi-agent" | "pi_agent" => "agents".to_string(),
        "claude-code" | "claude_code" => "claude".to_string(),
        other => other.to_string(),
    }
}

pub(super) fn agent_skills_dir() -> PathBuf {
    home_dir()
        .join(".agents")
        .join("skills")
        .join(AGENT_SKILL_NAME)
}

pub(super) fn claude_skill_dir() -> PathBuf {
    trimmed_env("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("skills")
        .join(AGENT_SKILL_NAME)
}

fn read_managed_skill_dir(path: &Path) -> CliResult<Option<String>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() {
        return Err(CliError::new(format!(
            "skills: refusing symlinked skill directory {}",
            path.display()
        )));
    }
    if !meta.is_dir() {
        return Err(CliError::new(format!(
            "skills: {} exists but is not a directory",
            path.display()
        )));
    }
    let skill_path = path.join("SKILL.md");
    let Some(text) = read_text_config(&skill_path, "agent skill")? else {
        return Err(CliError::new(format!(
            "skills: {} exists but has no SKILL.md",
            path.display()
        )));
    };
    if !text.contains(AGENT_SKILL_MARKER) {
        return Err(CliError::new(format!(
            "skills: refusing to overwrite unmanaged skill at {}",
            path.display()
        )));
    }
    Ok(Some(text))
}

fn backup_skill_dir(path: &Path) -> CliResult<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    loop {
        let backup = path.with_file_name(format!(
            "{}.bak-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(AGENT_SKILL_NAME),
            next_file_nonce()
        ));
        match fs::rename(path, &backup) {
            Ok(()) => return Ok(Some(backup)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        }
    }
}

fn prune_managed_skill_backups(path: &Path) -> CliResult<Vec<PathBuf>> {
    let Some(parent) = path.parent() else {
        return Ok(Vec::new());
    };
    let prefix = format!(
        "{}.bak-",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(AGENT_SKILL_NAME)
    );
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(nonce) = name.strip_prefix(&prefix) else {
            continue;
        };
        let backup_path = entry.path();
        if is_managed_skill_backup(&backup_path)? {
            let timestamp = nonce
                .split('-')
                .next()
                .and_then(|value| value.parse::<u128>().ok())
                .unwrap_or_default();
            backups.push((timestamp, nonce.to_string(), backup_path));
        }
    }
    backups.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let mut removed = Vec::new();
    for (_, _, backup) in backups.into_iter().skip(MAX_AGENT_SKILL_BACKUPS) {
        fs::remove_dir_all(&backup)?;
        removed.push(backup);
    }
    Ok(removed)
}

fn is_managed_skill_backup(path: &Path) -> CliResult<bool> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Ok(false);
    }
    let Some(text) = read_text_config(&path.join("SKILL.md"), "agent skill backup")? else {
        return Ok(false);
    };
    Ok(text.contains(AGENT_SKILL_MARKER))
}
