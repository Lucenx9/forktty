//! Browser-pane profiles (SP3 P2): stable per-profile identity plus file-backed
//! metadata. A profile isolates one browsing identity (cookies, storage, history);
//! each `Browser` surface carries a `ProfileId`.

use std::fmt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_PROFILE_STORE_BYTES: u64 = 1024 * 1024;

/// Stable identity for a browser profile. Serializes as its hyphenated lowercase
/// UUID string, which is also the on-disk directory name under `browser_profiles/`.
/// `Default` is the well-known P1 Default profile, so sessions created before the
/// profile system keep their data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(Uuid);

impl ProfileId {
    /// A fresh, random profile id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        // 00000000-0000-0000-0000-000000000001 — matches browser_session::DEFAULT_PROFILE_ID.
        Self(Uuid::from_u128(1))
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ProfileId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Persisted metadata for one profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub id: ProfileId,
    pub display_name: String,
    /// Unix seconds at creation.
    pub created_at: u64,
    pub is_default: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProfileError {
    CannotDeleteDefault,
    NotFound,
    Io(String),
    InvalidInput(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::CannotDeleteDefault => write!(f, "the default profile cannot be deleted"),
            ProfileError::NotFound => write!(f, "profile not found"),
            ProfileError::Io(e) => write!(f, "profile store io error: {e}"),
            ProfileError::InvalidInput(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for ProfileError {}

/// File-backed list of profiles (`profiles.json`). The Default profile is always
/// present (synthesized if missing). The store owns metadata only; on-disk session
/// data directories are managed by the GTK layer.
#[derive(Debug)]
pub struct ProfileStore {
    path: PathBuf,
    profiles: Vec<ProfileMeta>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_meta() -> ProfileMeta {
    ProfileMeta {
        id: ProfileId::default(),
        display_name: "Default".to_string(),
        created_at: 0,
        is_default: true,
    }
}

fn ensure_default_profile(profiles: &mut Vec<ProfileMeta>) {
    let default_id = ProfileId::default();
    let mut default_profile = None;
    let mut others = Vec::with_capacity(profiles.len());

    for mut profile in profiles.drain(..) {
        if profile.id == default_id {
            if default_profile.is_none() {
                profile.is_default = true;
                if profile.display_name.trim().is_empty() {
                    profile.display_name = "Default".to_string();
                }
                default_profile = Some(profile);
            }
        } else {
            profile.is_default = false;
            others.push(profile);
        }
    }

    profiles.push(default_profile.unwrap_or_else(default_meta));
    profiles.append(&mut others);
}

#[cfg(unix)]
fn ensure_private_profile_store_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if let Some(grandparent) = path.parent() {
        std::fs::create_dir_all(grandparent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
            harden_existing_profile_store_dir(path)
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn harden_existing_profile_store_dir(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::metadata(path)?;
    if metadata.uid() == unsafe { libc::geteuid() as u32 }
        && metadata.gid() == unsafe { libc::getegid() as u32 }
    {
        let mode = metadata.permissions().mode() & 0o777;
        let hardened = mode & 0o700;
        if hardened != mode {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(hardened))?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_profile_store_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

fn backup_corrupt_profile_store(path: &Path, bytes: &[u8]) {
    let backup = crate::backup::reserve_unique_backup_path(path, "json.bak");
    if std::fs::rename(path, &backup).is_err() {
        let _ = std::fs::write(&backup, bytes);
    }
}

#[cfg(unix)]
fn replacement_profile_store_mode(mode: u32, same_uid_and_gid: bool) -> u32 {
    if same_uid_and_gid {
        mode
    } else {
        mode & 0o700
    }
}

#[cfg(unix)]
fn profile_store_mode_for_replacement(
    existing: Option<&std::fs::Metadata>,
    replacement: &std::fs::Metadata,
) -> u32 {
    let Some(existing) = existing else {
        return 0o600;
    };
    let mode = existing.permissions().mode() & 0o777;
    replacement_profile_store_mode(
        mode,
        existing.uid() == replacement.uid() && existing.gid() == replacement.gid(),
    )
}

impl ProfileStore {
    /// Load the store from `path`, creating an in-memory Default-only store if the
    /// file is absent. A present file always has the Default profile ensured.
    pub fn load(path: PathBuf) -> Result<Self, ProfileError> {
        let mut profiles: Vec<ProfileMeta> =
            match read_optional_file_bounded(&path, MAX_PROFILE_STORE_BYTES, "profile store")? {
                Some(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(profiles) => profiles,
                    Err(_) => {
                        backup_corrupt_profile_store(&path, &bytes);
                        Vec::new()
                    }
                },
                None => Vec::new(),
            };
        ensure_default_profile(&mut profiles);
        Ok(Self { path, profiles })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> &[ProfileMeta] {
        &self.profiles
    }

    fn save(&self) -> Result<(), ProfileError> {
        if let Some(parent) = self.path.parent() {
            ensure_private_profile_store_dir(parent)
                .map_err(|e| ProfileError::Io(e.to_string()))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.profiles)
            .map_err(|e| ProfileError::Io(e.to_string()))?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_path = self
            .path
            .with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
        let result = (|| -> Result<(), ProfileError> {
            let mut open_options = std::fs::OpenOptions::new();
            open_options.write(true).create_new(true);
            #[cfg(unix)]
            let existing_metadata = std::fs::metadata(&self.path).ok();
            #[cfg(unix)]
            open_options.mode(0o600);
            let mut tmp_file = open_options
                .open(&tmp_path)
                .map_err(|e| ProfileError::Io(e.to_string()))?;
            std::io::Write::write_all(&mut tmp_file, &bytes)
                .map_err(|e| ProfileError::Io(e.to_string()))?;
            #[cfg(unix)]
            {
                let replacement_metadata = tmp_file
                    .metadata()
                    .map_err(|e| ProfileError::Io(e.to_string()))?;
                let mode = profile_store_mode_for_replacement(
                    existing_metadata.as_ref(),
                    &replacement_metadata,
                );
                tmp_file
                    .set_permissions(std::fs::Permissions::from_mode(mode))
                    .map_err(|e| ProfileError::Io(e.to_string()))?;
            }
            tmp_file
                .sync_all()
                .map_err(|e| ProfileError::Io(e.to_string()))?;
            std::fs::rename(&tmp_path, &self.path).map_err(|e| ProfileError::Io(e.to_string()))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        result
    }

    /// Create a new non-default profile and persist.
    pub fn create(&mut self, display_name: &str) -> Result<ProfileMeta, ProfileError> {
        let name = display_name.trim();
        if name.is_empty() {
            return Err(ProfileError::InvalidInput(
                "display_name must not be empty".to_string(),
            ));
        }
        let meta = ProfileMeta {
            id: ProfileId::new(),
            display_name: name.to_string(),
            created_at: now_secs(),
            is_default: false,
        };
        self.profiles.push(meta.clone());
        if let Err(e) = self.save() {
            self.profiles.pop();
            return Err(e);
        }
        Ok(meta)
    }

    /// Delete a profile by id and persist. Refuses the Default profile.
    pub fn delete(&mut self, id: &ProfileId) -> Result<(), ProfileError> {
        if *id == ProfileId::default() {
            return Err(ProfileError::CannotDeleteDefault);
        }
        let Some(pos) = self.profiles.iter().position(|p| p.id == *id) else {
            return Err(ProfileError::NotFound);
        };
        let removed = self.profiles.remove(pos);
        if let Err(e) = self.save() {
            self.profiles.insert(pos, removed);
            return Err(e);
        }
        Ok(())
    }

    /// Resolve an id-or-display-name string to a `ProfileId`. Display-name match is
    /// trimmed + case-insensitive.
    pub fn resolve(&self, id_or_name: &str) -> Option<ProfileId> {
        if let Ok(id) = id_or_name.parse::<ProfileId>() {
            return self.profiles.iter().find(|p| p.id == id).map(|p| p.id);
        }
        let needle = id_or_name.trim().to_ascii_lowercase();
        self.profiles
            .iter()
            .find(|p| p.display_name.trim().to_ascii_lowercase() == needle)
            .map(|p| p.id)
    }
}

fn read_optional_file_bounded(
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<Option<Vec<u8>>, ProfileError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(ProfileError::Io(err.to_string())),
    };
    if metadata.len() > limit {
        return Err(ProfileError::Io(format!(
            "{label} is {} bytes, exceeds limit of {limit} bytes",
            metadata.len()
        )));
    }
    std::fs::read(path)
        .map(Some)
        .map_err(|err| ProfileError::Io(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, ProfileStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let store = ProfileStore::load(path).unwrap();
        (dir, store)
    }

    #[test]
    fn fresh_store_has_only_the_default_profile() {
        let (_d, store) = temp_store();
        let all = store.list();
        assert_eq!(all.len(), 1);
        assert!(all[0].is_default);
        assert_eq!(all[0].id, ProfileId::default());
    }

    #[test]
    fn create_adds_a_named_profile_and_persists() {
        let (_d, mut store) = temp_store();
        let meta = store.create("Work").unwrap();
        assert_eq!(meta.display_name, "Work");
        assert!(!meta.is_default);
        let reloaded = ProfileStore::load(store.path().to_path_buf()).unwrap();
        assert!(reloaded
            .list()
            .iter()
            .any(|p| p.id == meta.id && p.display_name == "Work"));
    }

    #[test]
    fn delete_removes_a_profile_but_refuses_the_default() {
        let (_d, mut store) = temp_store();
        let meta = store.create("Throwaway").unwrap();
        assert!(store.delete(&meta.id).is_ok());
        assert!(!store.list().iter().any(|p| p.id == meta.id));
        assert!(matches!(
            store.delete(&ProfileId::default()),
            Err(ProfileError::CannotDeleteDefault)
        ));
    }

    #[test]
    fn resolve_matches_by_id_or_display_name_case_insensitively() {
        let (_d, mut store) = temp_store();
        let meta = store.create("Work").unwrap();
        assert_eq!(store.resolve(&meta.id.to_string()), Some(meta.id));
        assert_eq!(store.resolve("work"), Some(meta.id));
        assert_eq!(store.resolve("  WORK "), Some(meta.id));
        assert_eq!(store.resolve("nope"), None);
    }

    #[test]
    fn default_profile_id_renders_to_the_well_known_p1_string() {
        assert_eq!(
            ProfileId::default().to_string(),
            "00000000-0000-0000-0000-000000000001"
        );
    }

    #[test]
    fn profile_id_serde_is_a_plain_string() {
        let id = ProfileId::default();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"00000000-0000-0000-0000-000000000001\"");
        let back: ProfileId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn profile_id_parses_from_string() {
        let id: ProfileId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        assert_eq!(id, ProfileId::default());
        assert!("not-a-uuid".parse::<ProfileId>().is_err());
    }

    #[test]
    fn new_profile_ids_are_unique() {
        assert_ne!(ProfileId::new(), ProfileId::new());
    }

    #[test]
    fn resolve_returns_none_for_valid_but_absent_uuid() {
        let (_d, store) = temp_store();
        assert_eq!(store.resolve("00000000-0000-0000-0000-000000000099"), None);
    }

    #[test]
    fn create_rejects_empty_or_whitespace_name() {
        let (_d, mut store) = temp_store();
        assert!(matches!(
            store.create("   "),
            Err(ProfileError::InvalidInput(_))
        ));
        assert!(matches!(
            store.create(""),
            Err(ProfileError::InvalidInput(_))
        ));
        assert_eq!(store.list().len(), 1); // nothing persisted
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_missing_parent_dir_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("browser_profiles");
        let path = parent.join("profiles.json");
        assert!(!parent.exists());
        let mut store = ProfileStore::load(path).unwrap();
        store.create("Work").unwrap();
        let mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "browser_profiles/ must be created with 0o700");
    }

    #[cfg(unix)]
    #[test]
    fn save_hardens_existing_parent_dir_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("browser_profiles");
        let path = parent.join("profiles.json");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut store = ProfileStore::load(path).unwrap();
        store.create("Work").unwrap();

        let mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "existing browser_profiles/ must be hardened");
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_existing_file_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, serde_json::to_vec(&vec![default_meta()]).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut store = ProfileStore::load(path.clone()).unwrap();
        store.create("Work").unwrap();

        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_mode_drops_shared_bits_when_ownership_differs() {
        assert_eq!(replacement_profile_store_mode(0o640, true), 0o640);
        assert_eq!(replacement_profile_store_mode(0o640, false), 0o600);
        assert_eq!(replacement_profile_store_mode(0o644, false), 0o600);
        assert_eq!(replacement_profile_store_mode(0o750, false), 0o700);
    }

    #[test]
    fn corrupt_profile_store_is_backed_up_and_restarts_with_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, b"{ not json").unwrap();

        let store = ProfileStore::load(path.clone()).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].id, ProfileId::default());
        assert!(path.with_extension("json.bak").exists());
    }

    #[test]
    fn oversized_profile_store_is_rejected_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_PROFILE_STORE_BYTES + 1).unwrap();

        let err = ProfileStore::load(path).unwrap_err();
        assert!(err.to_string().contains("exceeds limit"));
    }

    #[test]
    fn load_inserts_well_known_default_even_if_other_profile_claims_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let rogue = ProfileMeta {
            id: ProfileId::new(),
            display_name: "Rogue Default".to_string(),
            created_at: 7,
            is_default: true,
        };
        std::fs::write(&path, serde_json::to_vec(&vec![rogue.clone()]).unwrap()).unwrap();

        let store = ProfileStore::load(path).unwrap();
        let profiles = store.list();

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id, ProfileId::default());
        assert!(profiles[0].is_default);
        assert_eq!(profiles[1].id, rogue.id);
        assert!(!profiles[1].is_default);
    }

    #[test]
    fn load_promotes_existing_default_id_and_clears_other_default_flags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let default = ProfileMeta {
            id: ProfileId::default(),
            display_name: "Default".to_string(),
            created_at: 0,
            is_default: false,
        };
        let work = ProfileMeta {
            id: ProfileId::new(),
            display_name: "Work".to_string(),
            created_at: 8,
            is_default: true,
        };
        std::fs::write(
            &path,
            serde_json::to_vec(&vec![work.clone(), default]).unwrap(),
        )
        .unwrap();

        let store = ProfileStore::load(path).unwrap();
        let profiles = store.list();

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id, ProfileId::default());
        assert!(profiles[0].is_default);
        assert_eq!(profiles[1].id, work.id);
        assert!(!profiles[1].is_default);
    }
}
