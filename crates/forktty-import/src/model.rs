//! Data types for the browser-import engine. All serde-serializable so the socket
//! front-end (a later integration step) can ship them over JSON-RPC unchanged.

use forktty_core::ProfileId;
use serde::{Deserialize, Serialize};

/// A supported source browser family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFamily {
    Firefox,
    Chrome,
    Chromium,
    Brave,
    Edge,
    Vivaldi,
}

impl BrowserFamily {
    /// Chromium-family browsers share the `Cookies`/`History`/`Bookmarks` layout and
    /// AES cookie encryption; Firefox is its own (plaintext cookies, `places.sqlite`).
    pub fn is_chromium(self) -> bool {
        !matches!(self, BrowserFamily::Firefox)
    }

    /// The Secret Service label whose secret derives the `v11` key (Chromium only).
    pub fn safe_storage_label(self) -> Option<&'static str> {
        match self {
            BrowserFamily::Firefox => None,
            BrowserFamily::Chrome => Some("Chrome Safe Storage"),
            BrowserFamily::Chromium => Some("Chromium Safe Storage"),
            BrowserFamily::Brave => Some("Brave Safe Storage"),
            BrowserFamily::Edge => Some("Microsoft Edge Safe Storage"),
            BrowserFamily::Vivaldi => Some("Vivaldi Safe Storage"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BrowserFamily::Firefox => "Firefox",
            BrowserFamily::Chrome => "Google Chrome",
            BrowserFamily::Chromium => "Chromium",
            BrowserFamily::Brave => "Brave",
            BrowserFamily::Edge => "Microsoft Edge",
            BrowserFamily::Vivaldi => "Vivaldi",
        }
    }
}

/// One discovered source browser with its profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBrowser {
    pub family: BrowserFamily,
    pub profiles: Vec<SourceProfile>,
}

/// One profile inside a source browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProfile {
    pub family: BrowserFamily,
    /// Human-readable name (Firefox profile name / Chromium `profile.info_cache` name,
    /// falling back to the directory name).
    pub display_name: String,
    /// Absolute path to the profile directory.
    pub path: String,
    pub is_default: bool,
}

/// Import destination for one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ImportDestination {
    /// Merge into an existing forktty profile.
    Existing(ProfileId),
    /// Create a new forktty profile with this display name.
    Create(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// All selected sources merge into one destination profile.
    SingleDestination,
    /// Each source maps to its own destination profile.
    SeparateProfiles,
}

/// One unit of the plan: these source profiles flow into this destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportEntry {
    pub sources: Vec<SourceProfile>,
    pub destination: ImportDestination,
}

/// The resolved import plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPlan {
    pub mode: ImportMode,
    pub entries: Vec<ImportEntry>,
}

/// A cookie ready to be written into a forktty profile's session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedCookie {
    pub name: String,
    pub value: String,
    pub host: String,
    pub path: String,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

/// A visited URL read from a source browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedVisit {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
}

/// A bookmark read from a source browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedBookmark {
    pub url: String,
    pub title: String,
}

/// Per-entry result counts after running an import.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    pub cookies: usize,
    pub history: usize,
    pub bookmarks: usize,
    /// Cookies that could not be decrypted/parsed and were skipped.
    pub skipped: usize,
}

impl ImportResult {
    pub fn add(&mut self, other: &ImportResult) {
        self.cookies += other.cookies;
        self.history += other.history;
        self.bookmarks += other.bookmarks;
        self.skipped += other.skipped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_family_is_chromium() {
        assert!(!BrowserFamily::Firefox.is_chromium());
        assert!(BrowserFamily::Chrome.is_chromium());
        assert!(BrowserFamily::Chromium.is_chromium());
        assert!(BrowserFamily::Brave.is_chromium());
        assert!(BrowserFamily::Edge.is_chromium());
        assert!(BrowserFamily::Vivaldi.is_chromium());
    }

    #[test]
    fn test_browser_family_safe_storage_label() {
        assert_eq!(BrowserFamily::Firefox.safe_storage_label(), None);
        assert_eq!(
            BrowserFamily::Chrome.safe_storage_label(),
            Some("Chrome Safe Storage")
        );
        assert_eq!(
            BrowserFamily::Chromium.safe_storage_label(),
            Some("Chromium Safe Storage")
        );
        assert_eq!(
            BrowserFamily::Brave.safe_storage_label(),
            Some("Brave Safe Storage")
        );
        assert_eq!(
            BrowserFamily::Edge.safe_storage_label(),
            Some("Microsoft Edge Safe Storage")
        );
        assert_eq!(
            BrowserFamily::Vivaldi.safe_storage_label(),
            Some("Vivaldi Safe Storage")
        );
    }

    #[test]
    fn test_browser_family_label() {
        assert_eq!(BrowserFamily::Firefox.label(), "Firefox");
        assert_eq!(BrowserFamily::Chrome.label(), "Google Chrome");
        assert_eq!(BrowserFamily::Chromium.label(), "Chromium");
        assert_eq!(BrowserFamily::Brave.label(), "Brave");
        assert_eq!(BrowserFamily::Edge.label(), "Microsoft Edge");
        assert_eq!(BrowserFamily::Vivaldi.label(), "Vivaldi");
    }

    #[test]
    fn test_import_result_add() {
        let mut res1 = ImportResult {
            cookies: 10,
            history: 20,
            bookmarks: 30,
            skipped: 5,
        };
        let res2 = ImportResult {
            cookies: 5,
            history: 10,
            bookmarks: 15,
            skipped: 2,
        };
        res1.add(&res2);
        assert_eq!(res1.cookies, 15);
        assert_eq!(res1.history, 30);
        assert_eq!(res1.bookmarks, 45);
        assert_eq!(res1.skipped, 7);
    }
}
