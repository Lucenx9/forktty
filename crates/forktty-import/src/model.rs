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
    fn browser_family_chromium_classification_matches_supported_sources() {
        let cases = [
            (BrowserFamily::Firefox, false),
            (BrowserFamily::Chrome, true),
            (BrowserFamily::Chromium, true),
            (BrowserFamily::Brave, true),
            (BrowserFamily::Edge, true),
            (BrowserFamily::Vivaldi, true),
        ];

        for (family, is_chromium) in cases {
            assert_eq!(family.is_chromium(), is_chromium, "{family:?}");
        }
    }

    #[test]
    fn browser_family_safe_storage_labels_match_secret_service_names() {
        let cases = [
            (BrowserFamily::Firefox, None),
            (BrowserFamily::Chrome, Some("Chrome Safe Storage")),
            (BrowserFamily::Chromium, Some("Chromium Safe Storage")),
            (BrowserFamily::Brave, Some("Brave Safe Storage")),
            (BrowserFamily::Edge, Some("Microsoft Edge Safe Storage")),
            (BrowserFamily::Vivaldi, Some("Vivaldi Safe Storage")),
        ];

        for (family, label) in cases {
            assert_eq!(family.safe_storage_label(), label, "{family:?}");
        }
    }

    #[test]
    fn browser_family_labels_match_display_names() {
        let cases = [
            (BrowserFamily::Firefox, "Firefox"),
            (BrowserFamily::Chrome, "Google Chrome"),
            (BrowserFamily::Chromium, "Chromium"),
            (BrowserFamily::Brave, "Brave"),
            (BrowserFamily::Edge, "Microsoft Edge"),
            (BrowserFamily::Vivaldi, "Vivaldi"),
        ];

        for (family, label) in cases {
            assert_eq!(family.label(), label, "{family:?}");
        }
    }

    #[test]
    fn import_result_add_accumulates_counts() {
        let mut result = ImportResult::default();
        result.add(&ImportResult {
            cookies: 10,
            history: 20,
            bookmarks: 30,
            skipped: 5,
        });
        result.add(&ImportResult {
            cookies: 5,
            history: 10,
            bookmarks: 15,
            skipped: 2,
        });

        assert_eq!(
            result,
            ImportResult {
                cookies: 15,
                history: 30,
                bookmarks: 45,
                skipped: 7,
            }
        );
    }
}
