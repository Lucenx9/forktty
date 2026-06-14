//! Browser source discovery. Probe functions take an explicit root path for
//! testability; `discover()` calls them with the real `dirs::home_dir()`-based roots.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::model::{BrowserFamily, SourceBrowser, SourceProfile};

const MAX_DISCOVERY_TEXT_BYTES: u64 = 1024 * 1024;

// ─── Firefox ──────────────────────────────────────────────────────────────────

/// Parse a minimal subset of INI: lines of the form `Key=Value` inside
/// `[SectionHeader]` blocks. Returns a list of sections, each as a `Vec<(key, value)>`.
/// Section headers are included as the first entry with key `""` and value = header name.
fn parse_ini(text: &str) -> Vec<Vec<(String, String)>> {
    let mut sections: Vec<Vec<(String, String)>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].to_string();
            sections.push(vec![("".to_string(), name)]);
        } else if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_string();
            let val = line[eq + 1..].trim().to_string();
            if let Some(sec) = sections.last_mut() {
                sec.push((key, val));
            }
        }
    }
    sections
}

fn ini_get<'a>(section: &'a [(String, String)], key: &str) -> Option<&'a str> {
    section
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

fn scan_firefox_profiles(root: &Path) -> Vec<SourceProfile> {
    let mut profiles = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_profile_dir = entry
                .file_type()
                .is_ok_and(|ft| ft.is_dir() || (ft.is_symlink() && path.is_dir()));
            if is_profile_dir && has_firefox_importable_data(&path) {
                let name = entry.file_name().to_string_lossy().into_owned();
                profiles.push(SourceProfile {
                    family: BrowserFamily::Firefox,
                    display_name: name,
                    path: path.to_string_lossy().into_owned(),
                    is_default: false,
                });
            }
        }
    }
    profiles
}

fn has_firefox_importable_data(profile_dir: &Path) -> bool {
    profile_dir.join("cookies.sqlite").exists() || profile_dir.join("places.sqlite").exists()
}

fn has_chromium_importable_data(profile_dir: &Path) -> bool {
    profile_dir.join("Cookies").exists()
        || profile_dir.join("History").exists()
        || profile_dir.join("Bookmarks").exists()
}

/// Discover Firefox profiles under `root` (typically `~/.mozilla/firefox`).
/// Returns `None` if `root` does not exist.
pub fn discover_firefox(root: &Path) -> Option<SourceBrowser> {
    if !root.exists() {
        return None;
    }

    let ini_path = root.join("profiles.ini");
    let profiles = if ini_path.exists() {
        // Parse profiles.ini
        let text = read_small_text_file(&ini_path).unwrap_or_default();
        let sections = parse_ini(&text);
        let mut profiles = Vec::new();
        for section in &sections {
            // Section header is stored as ("", header_name)
            let header = match section.first() {
                Some((k, v)) if k.is_empty() => v.as_str(),
                _ => continue,
            };
            // Only consider [ProfileN] sections (Profile0, Profile1, …)
            if !header
                .strip_prefix("Profile")
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
            {
                continue;
            }
            let name = match ini_get(section, "Name") {
                Some(n) => n.to_string(),
                None => continue,
            };
            let path_val = match ini_get(section, "Path") {
                Some(p) => p,
                None => continue,
            };
            let is_relative = ini_get(section, "IsRelative")
                .map(|v| v != "0")
                .unwrap_or(true); // default: relative
            let profile_dir: PathBuf = if is_relative {
                root.join(path_val)
            } else {
                PathBuf::from(path_val)
            };
            if !has_firefox_importable_data(&profile_dir) {
                continue;
            }
            let is_default = ini_get(section, "Default")
                .map(|v| v == "1")
                .unwrap_or(false);
            profiles.push(SourceProfile {
                family: BrowserFamily::Firefox,
                display_name: name,
                path: profile_dir.to_string_lossy().into_owned(),
                is_default,
            });
        }
        if profiles.is_empty() {
            scan_firefox_profiles(root)
        } else {
            profiles
        }
    } else {
        scan_firefox_profiles(root)
    };

    if profiles.is_empty() {
        None
    } else {
        Some(SourceBrowser {
            family: BrowserFamily::Firefox,
            profiles,
        })
    }
}

// ─── Chromium family ──────────────────────────────────────────────────────────

/// Discover Chromium-family browser profiles under `root`.
/// Returns `None` if `root` does not exist.
pub fn discover_chromium_family(family: BrowserFamily, root: &Path) -> Option<SourceBrowser> {
    if !root.exists() {
        return None;
    }

    // Load Local State for display names (lenient — failure → fall back to dir names).
    let name_map = load_local_state_names(root);

    // Candidate dirs: "Default" + "Profile *"
    let mut candidates: Vec<String> = Vec::new();
    let default_dir = root.join("Default");
    if default_dir.is_dir() {
        candidates.push("Default".to_string());
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut extra: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("Profile ") && e.path().is_dir() {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        extra.sort();
        candidates.extend(extra);
    }

    let mut profiles: Vec<SourceProfile> = Vec::new();
    for dir_name in &candidates {
        let dir_path = root.join(dir_name);
        if !has_chromium_importable_data(&dir_path) {
            continue;
        }
        let display_name = name_map
            .as_ref()
            .and_then(|m| m.get(dir_name.as_str()).cloned())
            .unwrap_or_else(|| dir_name.clone());
        let is_default = dir_name == "Default";
        profiles.push(SourceProfile {
            family,
            display_name,
            path: dir_path.to_string_lossy().into_owned(),
            is_default,
        });
    }

    if profiles.is_empty() {
        None
    } else {
        Some(SourceBrowser { family, profiles })
    }
}

/// Load `profile.info_cache` display names from `<root>/Local State`.
/// Returns `None` on any parse failure (callers fall back to dir names).
fn load_local_state_names(root: &Path) -> Option<std::collections::HashMap<String, String>> {
    let text = read_small_text_file(&root.join("Local State"))?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let cache = v
        .get("profile")
        .and_then(|p| p.get("info_cache"))
        .and_then(|c| c.as_object())?;
    let map = cache
        .iter()
        .filter_map(|(dir, info)| {
            let name = info.get("name")?.as_str()?.to_string();
            Some((dir.clone(), name))
        })
        .collect();
    Some(map)
}

fn read_small_text_file(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DISCOVERY_TEXT_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_DISCOVERY_TEXT_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_DISCOVERY_TEXT_BYTES {
        return None;
    }
    Some(text)
}

// ─── Top-level discover ───────────────────────────────────────────────────────

/// Discover all installed browsers on the current user's machine.
/// Probes well-known config roots under `$HOME`.
pub fn discover() -> Vec<SourceBrowser> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let mut browsers = Vec::new();

    // Firefox
    if let Some(b) = discover_firefox(&home.join(".mozilla/firefox")) {
        browsers.push(b);
    }

    // Chromium family
    let chromium_roots: &[(BrowserFamily, &str)] = &[
        (BrowserFamily::Chrome, ".config/google-chrome"),
        (BrowserFamily::Chromium, ".config/chromium"),
        (BrowserFamily::Brave, ".config/BraveSoftware/Brave-Browser"),
        (BrowserFamily::Edge, ".config/microsoft-edge"),
        (BrowserFamily::Vivaldi, ".config/vivaldi"),
    ];
    for &(family, rel) in chromium_roots {
        if let Some(b) = discover_chromium_family(family, &home.join(rel)) {
            browsers.push(b);
        }
    }

    browsers
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use std::sync::MutexGuard;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard<'a> {
        _guard: MutexGuard<'a, ()>,
        saved_home: Option<String>,
        saved_userprofile: Option<String>,
    }

    impl<'a> EnvGuard<'a> {
        fn new(temp_home: &Path) -> Self {
            let _guard = ENV_MUTEX.lock().unwrap();
            let saved_home = std::env::var("HOME").ok();
            let saved_userprofile = std::env::var("USERPROFILE").ok();

            std::env::set_var("HOME", temp_home);
            std::env::set_var("USERPROFILE", temp_home);

            Self {
                _guard,
                saved_home,
                saved_userprofile,
            }
        }
    }

    impl<'a> Drop for EnvGuard<'a> {
        fn drop(&mut self) {
            if let Some(ref h) = self.saved_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(ref u) = self.saved_userprofile {
                std::env::set_var("USERPROFILE", u);
            } else {
                std::env::remove_var("USERPROFILE");
            }
        }
    }

    #[test]
    fn discover_firefox_reads_profiles_ini_names() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("abc.default-release")).unwrap();
        fs::write(ff.join("abc.default-release/cookies.sqlite"), b"x").unwrap();
        fs::write(
            ff.join("profiles.ini"),
            "[Profile0]\nName=default-release\nPath=abc.default-release\nDefault=1\n",
        )
        .unwrap();
        let browser = discover_firefox(&ff).unwrap();
        assert_eq!(browser.family, BrowserFamily::Firefox);
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "default-release");
        assert!(browser.profiles[0].is_default);
    }

    #[test]
    fn discover_chromium_reads_local_state_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".config/chromium");
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Cookies"), b"x").unwrap();
        fs::create_dir_all(root.join("Profile 1")).unwrap();
        fs::write(root.join("Profile 1/Cookies"), b"x").unwrap();
        fs::write(
            root.join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"You"},"Profile 1":{"name":"Work"}}}}"#,
        )
        .unwrap();
        let browser = discover_chromium_family(BrowserFamily::Chromium, &root).unwrap();
        let mut names: Vec<_> = browser
            .profiles
            .iter()
            .map(|p| p.display_name.clone())
            .collect();
        names.sort();
        assert_eq!(names, vec!["Work", "You"]);
        assert!(browser
            .profiles
            .iter()
            .any(|p| p.display_name == "You" && p.is_default));
    }

    #[test]
    fn discover_chromium_falls_back_to_dir_name_without_local_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".config/chromium");
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Cookies"), b"x").unwrap();
        let browser = discover_chromium_family(BrowserFamily::Chromium, &root).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "Default");
    }

    #[test]
    fn discover_firefox_fallback_scan_without_ini() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("xyz.default")).unwrap();
        fs::write(ff.join("xyz.default/cookies.sqlite"), b"x").unwrap();
        // No profiles.ini written → fallback scan path
        let browser = discover_firefox(&ff).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "xyz.default");
        assert!(!browser.profiles[0].is_default);
    }

    #[cfg(unix)]
    #[test]
    fn discover_firefox_fallback_scan_follows_symlinked_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        let target = dir.path().join("external/default");
        fs::create_dir_all(&ff).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("places.sqlite"), b"x").unwrap();
        std::os::unix::fs::symlink(&target, ff.join("linked.default")).unwrap();

        let browser = discover_firefox(&ff).unwrap();

        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "linked.default");
        assert_eq!(
            browser.profiles[0].path,
            ff.join("linked.default").to_string_lossy()
        );
    }

    #[test]
    fn discover_firefox_includes_history_only_profile() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("history.default")).unwrap();
        fs::write(ff.join("history.default/places.sqlite"), b"x").unwrap();
        fs::write(
            ff.join("profiles.ini"),
            "[Profile0]\nName=history\nPath=history.default\nDefault=1\n",
        )
        .unwrap();
        let browser = discover_firefox(&ff).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "history");
        assert!(browser.profiles[0].is_default);
    }

    #[test]
    fn discover_chromium_includes_bookmark_only_profile() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".config/chromium");
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Bookmarks"), b"{}").unwrap();
        let browser = discover_chromium_family(BrowserFamily::Chromium, &root).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "Default");
        assert!(browser.profiles[0].is_default);
    }

    #[test]
    fn discover_firefox_fallback_scan_with_unusable_ini() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("xyz.default")).unwrap();
        fs::write(ff.join("xyz.default/cookies.sqlite"), b"x").unwrap();
        fs::write(
            ff.join("profiles.ini"),
            "[General]\nStartWithLastProfile=1\n",
        )
        .unwrap();
        let browser = discover_firefox(&ff).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "xyz.default");
        assert!(!browser.profiles[0].is_default);
    }

    #[test]
    fn discover_firefox_fallback_scan_with_oversized_ini() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("xyz.default")).unwrap();
        fs::write(ff.join("xyz.default/cookies.sqlite"), b"x").unwrap();
        std::fs::File::create(ff.join("profiles.ini"))
            .unwrap()
            .set_len(MAX_DISCOVERY_TEXT_BYTES + 1)
            .unwrap();
        let browser = discover_firefox(&ff).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "xyz.default");
        assert!(!browser.profiles[0].is_default);
    }

    #[test]
    fn discover_chromium_falls_back_when_local_state_is_oversized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".config/chromium");
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Cookies"), b"x").unwrap();
        std::fs::File::create(root.join("Local State"))
            .unwrap()
            .set_len(MAX_DISCOVERY_TEXT_BYTES + 1)
            .unwrap();
        let browser = discover_chromium_family(BrowserFamily::Chromium, &root).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "Default");
    }

    #[test]
    fn discover_returns_none_for_absent_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover_firefox(&dir.path().join("nope")).is_none());
        assert!(
            discover_chromium_family(BrowserFamily::Chrome, &dir.path().join("nope")).is_none()
        );
    }

    #[test]
    fn discover_finds_all_browsers_in_home_dir() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        let _guard = EnvGuard::new(home);

        // Set up Firefox
        let ff_root = home.join(".mozilla/firefox");
        fs::create_dir_all(ff_root.join("ff.default")).unwrap();
        fs::write(ff_root.join("ff.default/cookies.sqlite"), b"x").unwrap();
        fs::write(
            ff_root.join("profiles.ini"),
            "[Profile0]\nName=default\nPath=ff.default\nDefault=1\n",
        )
        .unwrap();

        // Set up Chrome
        let chrome_root = home.join(".config/google-chrome");
        fs::create_dir_all(chrome_root.join("Default")).unwrap();
        fs::write(chrome_root.join("Default/Cookies"), b"x").unwrap();

        // Set up Brave
        let brave_root = home.join(".config/BraveSoftware/Brave-Browser");
        fs::create_dir_all(brave_root.join("Default")).unwrap();
        fs::write(brave_root.join("Default/Cookies"), b"x").unwrap();

        let browsers = discover();

        assert_eq!(browsers.len(), 3);

        let has_ff = browsers.iter().any(|b| b.family == BrowserFamily::Firefox);
        let has_chrome = browsers.iter().any(|b| b.family == BrowserFamily::Chrome);
        let has_brave = browsers.iter().any(|b| b.family == BrowserFamily::Brave);

        assert!(has_ff);
        assert!(has_chrome);
        assert!(has_brave);
    }

    #[test]
    fn discover_returns_empty_for_empty_home_dir() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::new(dir.path());

        let browsers = discover();
        assert!(browsers.is_empty());
    }
}
