//! Browser-pane profiles (SP3 P2): stable per-profile identity plus file-backed
//! metadata. A profile isolates one browsing identity (cookies, storage, history);
//! each `Browser` surface carries a `ProfileId`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
