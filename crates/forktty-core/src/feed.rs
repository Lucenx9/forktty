use std::{collections::BTreeSet, io::Write, path::Path};

use serde::{Deserialize, Serialize};

const MAX_FEED_ENTRIES: usize = 500;
const MAX_FEED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const FEED_SCHEMA_VERSION: u8 = 1;

#[derive(Debug)]
pub enum FeedError {
    Io(String),
    Json(String),
    NotFound(String),
    InvalidState(String),
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::Io(err) => write!(f, "feed io error: {err}"),
            FeedError::Json(err) => write!(f, "feed json error: {err}"),
            FeedError::NotFound(id) => write!(f, "feed entry not found: {id}"),
            FeedError::InvalidState(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for FeedError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedEntryType {
    Approval,
    Notification,
    Status,
    Progress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedApprovalState {
    Pending,
    Approved,
    Denied,
    Dismissed,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: FeedEntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub read: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    pub title: String,
    pub body: String,
    pub workspace_id: Option<String>,
    pub surface_id: Option<String>,
    pub created_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_state: Option<FeedApprovalState>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FeedFile {
    schema: u8,
    entries: Vec<FeedEntry>,
}

#[derive(Debug)]
pub struct FeedStore {
    path: std::path::PathBuf,
    entries: Vec<FeedEntry>,
}

impl FeedStore {
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self, FeedError> {
        let path = path.as_ref().to_path_buf();
        let entries = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                if metadata.len() > MAX_FEED_FILE_BYTES {
                    return Err(FeedError::Io("feed file is too large".to_string()));
                }
                let bytes = std::fs::read(&path).map_err(|err| FeedError::Io(err.to_string()))?;
                if bytes.is_empty() {
                    Vec::new()
                } else {
                    let file = serde_json::from_slice::<FeedFile>(&bytes)
                        .map_err(|err| FeedError::Json(err.to_string()))?;
                    if file.schema != FEED_SCHEMA_VERSION {
                        return Err(FeedError::Json(format!(
                            "unsupported feed schema: {}",
                            file.schema
                        )));
                    }
                    file.entries
                }
            }
            Ok(_) => return Err(FeedError::Io("feed path is not a regular file".to_string())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(FeedError::Io(err.to_string())),
        };
        validate_feed_entries(&entries)?;
        let mut store = Self { path, entries };
        store.bound_entries();
        Ok(store)
    }

    pub fn open_recovering_at(path: impl AsRef<Path>) -> Result<Self, FeedError> {
        let path = path.as_ref().to_path_buf();
        match Self::open_at(&path) {
            Ok(store) => Ok(store),
            Err(FeedError::Json(_) | FeedError::InvalidState(_)) => {
                quarantine_corrupt_feed_file(&path)?;
                Ok(Self {
                    path,
                    entries: Vec::new(),
                })
            }
            Err(err) => Err(err),
        }
    }

    pub fn default_path() -> Option<std::path::PathBuf> {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .map(|dir| dir.join("forktty").join("feed.json"))
    }

    pub fn open_default() -> Result<Option<Self>, FeedError> {
        let Some(path) = Self::default_path() else {
            return Ok(None);
        };
        Self::open_recovering_at(path).map(Some)
    }

    pub fn append(&mut self, entry: FeedEntry) -> Result<(), FeedError> {
        let previous = self.entries.clone();
        let entry = match self.entries.iter().find(|existing| existing.id == entry.id) {
            Some(existing) if should_preserve_approval_decision(existing, &entry) => {
                let mut entry = entry;
                entry.approval_state = existing.approval_state.clone();
                entry
            }
            _ => entry,
        };
        self.entries.retain(|existing| existing.id != entry.id);
        self.entries.push(entry);
        self.bound_entries();
        if let Err(err) = self.save() {
            self.entries = previous;
            return Err(err);
        }
        Ok(())
    }

    pub fn list(&self, workspace_id: Option<&str>, limit: usize) -> Vec<FeedEntry> {
        if limit == 0 {
            return Vec::new();
        }
        let mut entries = self
            .entries
            .iter()
            .filter(|entry| workspace_id.is_none_or(|id| entry.workspace_id.as_deref() == Some(id)))
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_at_ms));
        entries.truncate(limit);
        entries
    }

    pub fn decide_approval(
        &mut self,
        id: &str,
        state: FeedApprovalState,
    ) -> Result<FeedEntry, FeedError> {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.entry_type == FeedEntryType::Approval)
        else {
            return Err(FeedError::NotFound(id.to_string()));
        };
        if entry.approval_state != Some(FeedApprovalState::Pending) {
            return Err(FeedError::InvalidState(format!(
                "feed approval {id} is not pending"
            )));
        }
        let previous = entry.approval_state.clone();
        entry.approval_state = Some(state);
        let entry = entry.clone();
        if let Err(err) = self.save() {
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                entry.approval_state = previous;
            }
            return Err(err);
        }
        Ok(entry)
    }

    pub fn mark_approvals<I>(
        &mut self,
        ids: I,
        state: FeedApprovalState,
    ) -> Result<Vec<FeedEntry>, FeedError>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let ids = ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect::<BTreeSet<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let previous = self.entries.clone();
        let mut changed = Vec::new();
        for entry in &mut self.entries {
            if entry.entry_type != FeedEntryType::Approval
                || entry.approval_state != Some(FeedApprovalState::Pending)
                || !ids.contains(&entry.id)
            {
                continue;
            }
            entry.approval_state = Some(state.clone());
            changed.push(entry.clone());
        }
        if changed.is_empty() {
            return Ok(changed);
        }
        if let Err(err) = self.save() {
            self.entries = previous;
            return Err(err);
        }
        Ok(changed)
    }

    fn bound_entries(&mut self) {
        self.entries
            .sort_by_key(|entry| std::cmp::Reverse(entry.created_at_ms));
        let mut approvals = self
            .entries
            .iter()
            .filter(|entry| entry.entry_type == FeedEntryType::Approval)
            .cloned()
            .collect::<Vec<_>>();
        approvals.truncate(MAX_FEED_ENTRIES);
        let remaining = MAX_FEED_ENTRIES.saturating_sub(approvals.len());
        let mut rest = self
            .entries
            .iter()
            .filter(|entry| entry.entry_type != FeedEntryType::Approval)
            .take(remaining)
            .cloned()
            .collect::<Vec<_>>();
        approvals.append(&mut rest);
        approvals.sort_by_key(|entry| std::cmp::Reverse(entry.created_at_ms));
        self.entries = approvals;
    }

    fn save(&self) -> Result<(), FeedError> {
        if let Some(parent) = self.path.parent() {
            ensure_private_feed_dir(parent)?;
        }
        let file = FeedFile {
            schema: FEED_SCHEMA_VERSION,
            entries: self.entries.clone(),
        };
        let bytes =
            serde_json::to_vec_pretty(&file).map_err(|err| FeedError::Json(err.to_string()))?;
        if bytes.len() as u64 > MAX_FEED_FILE_BYTES {
            return Err(FeedError::Io("feed data is too large".to_string()));
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_path = self
            .path
            .with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
        let result = (|| -> Result<(), FeedError> {
            let mut tmp_file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .map_err(|err| FeedError::Io(err.to_string()))?;
            apply_feed_file_permissions(&tmp_path)?;
            tmp_file
                .write_all(&bytes)
                .map_err(|err| FeedError::Io(err.to_string()))?;
            tmp_file
                .sync_all()
                .map_err(|err| FeedError::Io(err.to_string()))?;
            std::fs::rename(&tmp_path, &self.path).map_err(|err| FeedError::Io(err.to_string()))?;
            apply_feed_file_permissions(&self.path)?;
            sync_parent_dir(&self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        result
    }
}

fn should_preserve_approval_decision(existing: &FeedEntry, entry: &FeedEntry) -> bool {
    existing.entry_type == FeedEntryType::Approval
        && entry.entry_type == FeedEntryType::Approval
        && existing.created_at_ms == entry.created_at_ms
        && matches!(
            existing.approval_state,
            Some(
                FeedApprovalState::Approved
                    | FeedApprovalState::Denied
                    | FeedApprovalState::Dismissed
                    | FeedApprovalState::Stale
            )
        )
        && existing.kind == entry.kind
        && existing.key == entry.key
        && existing.value == entry.value
        && existing.total == entry.total
        && existing.title == entry.title
        && existing.body == entry.body
        && existing.workspace_id == entry.workspace_id
        && existing.surface_id == entry.surface_id
}

fn validate_feed_entries(entries: &[FeedEntry]) -> Result<(), FeedError> {
    let mut ids = BTreeSet::new();
    for entry in entries {
        if entry.id.trim().is_empty()
            || entry.id.len() > 512
            || entry.id.chars().any(char::is_control)
        {
            return Err(FeedError::InvalidState("invalid feed entry id".to_string()));
        }
        if !ids.insert(entry.id.as_str()) {
            return Err(FeedError::InvalidState(format!(
                "duplicate feed entry id: {}",
                entry.id
            )));
        }
        match entry.entry_type {
            FeedEntryType::Approval if entry.approval_state.is_none() => {
                return Err(FeedError::InvalidState(format!(
                    "feed approval {} is missing approval_state",
                    entry.id
                )));
            }
            FeedEntryType::Notification | FeedEntryType::Status | FeedEntryType::Progress
                if entry.approval_state.is_some() =>
            {
                return Err(FeedError::InvalidState(format!(
                    "non-approval feed entry {} has approval_state",
                    entry.id
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn quarantine_corrupt_feed_file(path: &Path) -> Result<(), FeedError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(FeedError::Io("feed path is not a regular file".to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(FeedError::Io(err.to_string())),
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let quarantine_path =
        crate::backup::reserve_unique_backup_path(path, &format!("json.bad-{nonce}"));
    std::fs::rename(path, &quarantine_path).map_err(|err| FeedError::Io(err.to_string()))?;
    sync_parent_dir(&quarantine_path)?;
    Ok(())
}

fn ensure_private_feed_dir(parent: &Path) -> Result<(), FeedError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|err| FeedError::Io(err.to_string()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| FeedError::Io(err.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(parent).map_err(|err| FeedError::Io(err.to_string()))?;
    }
    Ok(())
}

fn apply_feed_file_permissions(path: &Path) -> Result<(), FeedError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| FeedError::Io(err.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn sync_parent_dir(path: &Path) -> Result<(), FeedError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| FeedError::Io(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, workspace_id: &str) -> FeedEntry {
        FeedEntry {
            id: id.to_string(),
            entry_type: FeedEntryType::Notification,
            kind: Some("info".to_string()),
            read: false,
            key: None,
            value: None,
            total: None,
            title: id.to_string(),
            body: "body".to_string(),
            workspace_id: Some(workspace_id.to_string()),
            surface_id: None,
            created_at_ms: id.trim_start_matches("feed-").parse().unwrap_or(0),
            approval_state: None,
        }
    }

    #[test]
    fn feed_store_persists_bounded_entries_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        for id in ["feed-1", "feed-2", "feed-3"] {
            store.append(entry(id, "w1")).unwrap();
        }

        let reopened = FeedStore::open_at(&path).unwrap();
        let items = reopened.list(None, 2);

        assert_eq!(
            items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["feed-3", "feed-2"]
        );
    }

    #[test]
    fn feed_store_filters_by_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        store.append(entry("feed-1", "w1")).unwrap();
        store.append(entry("feed-2", "w2")).unwrap();

        let items = store.list(Some("w1"), 10);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].workspace_id.as_deref(), Some("w1"));
    }

    #[test]
    fn feed_store_records_approval_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        let mut approval = entry("feed-1", "w1");
        approval.entry_type = FeedEntryType::Approval;
        approval.approval_state = Some(FeedApprovalState::Pending);
        store.append(approval).unwrap();

        let decided = store
            .decide_approval("feed-1", FeedApprovalState::Approved)
            .unwrap();

        assert_eq!(decided.approval_state, Some(FeedApprovalState::Approved));
        assert_eq!(
            FeedStore::open_at(&path).unwrap().list(None, 10)[0].approval_state,
            Some(FeedApprovalState::Approved)
        );
    }

    #[test]
    fn append_preserves_recorded_approval_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        let mut approval = entry("feed-1", "w1");
        approval.entry_type = FeedEntryType::Approval;
        approval.approval_state = Some(FeedApprovalState::Pending);
        store.append(approval.clone()).unwrap();
        store
            .decide_approval("feed-1", FeedApprovalState::Approved)
            .unwrap();
        store.append(approval).unwrap();

        assert_eq!(
            store.list(None, 10)[0].approval_state,
            Some(FeedApprovalState::Approved)
        );
    }

    #[test]
    fn append_does_not_reuse_approval_decision_for_newer_entry_with_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        let mut approval = entry("feed-1", "w1");
        approval.entry_type = FeedEntryType::Approval;
        approval.approval_state = Some(FeedApprovalState::Pending);
        store.append(approval.clone()).unwrap();
        store
            .decide_approval("feed-1", FeedApprovalState::Approved)
            .unwrap();

        approval.title = "new prompt".to_string();
        approval.created_at_ms += 1;
        store.append(approval).unwrap();

        let listed = store.list(None, 10);
        assert_eq!(listed[0].title, "new prompt");
        assert_eq!(listed[0].approval_state, Some(FeedApprovalState::Pending));
    }

    #[test]
    fn append_does_not_reuse_approval_decision_for_different_prompt_payload_with_same_id_and_time()
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        let mut approval = entry("feed-1", "w1");
        approval.entry_type = FeedEntryType::Approval;
        approval.kind = Some("prompt".to_string());
        approval.title = "old prompt".to_string();
        approval.body = "run old command?".to_string();
        approval.created_at_ms = 42;
        approval.approval_state = Some(FeedApprovalState::Pending);
        store.append(approval.clone()).unwrap();
        store
            .decide_approval("feed-1", FeedApprovalState::Approved)
            .unwrap();

        approval.title = "new prompt".to_string();
        approval.body = "run new command?".to_string();
        store.append(approval).unwrap();

        let listed = store.list(None, 10);
        assert_eq!(listed[0].title, "new prompt");
        assert_eq!(listed[0].approval_state, Some(FeedApprovalState::Pending));
    }

    #[test]
    fn pending_approvals_survive_feed_churn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        let mut approval = entry("feed-1", "w1");
        approval.entry_type = FeedEntryType::Approval;
        approval.approval_state = Some(FeedApprovalState::Pending);
        store.append(approval).unwrap();
        for index in 2..(MAX_FEED_ENTRIES + 20) {
            store.append(entry(&format!("feed-{index}"), "w1")).unwrap();
        }

        assert!(store.list(None, MAX_FEED_ENTRIES).iter().any(|entry| {
            entry.id == "feed-1" && entry.approval_state == Some(FeedApprovalState::Pending)
        }));
    }

    #[test]
    fn approved_approvals_survive_feed_churn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        let mut store = FeedStore::open_at(&path).unwrap();
        let mut approval = entry("feed-1", "w1");
        approval.entry_type = FeedEntryType::Approval;
        approval.approval_state = Some(FeedApprovalState::Pending);
        store.append(approval).unwrap();
        store
            .decide_approval("feed-1", FeedApprovalState::Approved)
            .unwrap();
        for index in 2..(MAX_FEED_ENTRIES + 20) {
            store.append(entry(&format!("feed-{index}"), "w1")).unwrap();
        }

        assert!(store.list(None, MAX_FEED_ENTRIES).iter().any(|entry| {
            entry.id == "feed-1" && entry.approval_state == Some(FeedApprovalState::Approved)
        }));
    }

    #[test]
    fn recovering_open_quarantines_corrupt_feed_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        std::fs::write(&path, b"{broken json").unwrap();

        let store = FeedStore::open_recovering_at(&path).unwrap();

        assert!(store.list(None, 10).is_empty());
        assert!(!path.exists());
        let quarantined = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            quarantined
                .iter()
                .any(|name| name.starts_with("feed.json.bad-")),
            "expected corrupt feed quarantine sibling, got {quarantined:?}"
        );
    }

    #[test]
    fn feed_store_rejects_duplicate_loaded_entry_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        std::fs::write(
            &path,
            r#"{
                "schema": 1,
                "entries": [
                    {
                        "id": "feed-1",
                        "type": "notification",
                        "title": "First",
                        "body": "body",
                        "workspace_id": "w1",
                        "surface_id": null,
                        "created_at_ms": 1
                    },
                    {
                        "id": "feed-1",
                        "type": "notification",
                        "title": "Second",
                        "body": "body",
                        "workspace_id": "w1",
                        "surface_id": null,
                        "created_at_ms": 2
                    }
                ]
            }"#,
        )
        .unwrap();

        let err = FeedStore::open_at(&path).unwrap_err();

        assert!(matches!(
            err,
            FeedError::InvalidState(message)
                if message.contains("duplicate feed entry id")
        ));
    }

    #[test]
    fn recovering_open_quarantines_invalid_feed_entry_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        std::fs::write(
            &path,
            r#"{
                "schema": 1,
                "entries": [
                    {
                        "id": "feed-1",
                        "type": "notification",
                        "title": "Info",
                        "body": "body",
                        "workspace_id": "w1",
                        "surface_id": null,
                        "created_at_ms": 1,
                        "approval_state": "approved"
                    }
                ]
            }"#,
        )
        .unwrap();

        let store = FeedStore::open_recovering_at(&path).unwrap();

        assert!(store.list(None, 10).is_empty());
        assert!(!path.exists());
        let quarantined = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            quarantined
                .iter()
                .any(|name| name.starts_with("feed.json.bad-")),
            "expected invalid feed quarantine sibling, got {quarantined:?}"
        );
    }

    #[test]
    fn feed_store_rejects_unknown_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.json");
        std::fs::write(&path, r#"{"schema":2,"entries":[]}"#).unwrap();

        assert!(matches!(
            FeedStore::open_at(&path),
            Err(FeedError::Json(message)) if message.contains("unsupported feed schema")
        ));
    }
}
