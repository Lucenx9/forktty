use forktty_core::config;
use forktty_core::update::{
    select_newest_update, AssetKind, ReleaseAsset, ReleaseParseError, TargetArch,
};

const RELEASES_JSON: &str = r#"
[
  {
    "tag_name": "v0.3.0-alpha.1",
    "prerelease": true,
    "draft": false,
    "html_url": "https://github.com/Lucenx9/forktty/releases/tag/v0.3.0-alpha.1",
    "assets": [
      {
        "name": "forktty-0.3.0-alpha.1-x86_64.AppImage",
        "browser_download_url": "https://example.invalid/forktty-0.3.0-alpha.1-x86_64.AppImage"
      },
      {
        "name": "forktty-0.3.0-alpha.1-x86_64.AppImage.zsync",
        "browser_download_url": "https://example.invalid/forktty-0.3.0-alpha.1-x86_64.AppImage.zsync"
      },
      {
        "name": "forktty_0.3.0.alpha.1_amd64.deb",
        "browser_download_url": "https://example.invalid/forktty_0.3.0.alpha.1_amd64.deb"
      },
      {
        "name": "SHA256SUMS",
        "browser_download_url": "https://example.invalid/SHA256SUMS"
      }
    ]
  },
  {
    "tag_name": "v0.2.0",
    "prerelease": false,
    "draft": false,
    "html_url": "https://github.com/Lucenx9/forktty/releases/tag/v0.2.0",
    "assets": [
      {
        "name": "forktty-0.2.0-x86_64.AppImage",
        "browser_download_url": "https://example.invalid/forktty-0.2.0-x86_64.AppImage"
      },
      {
        "name": "SHA256SUMS",
        "browser_download_url": "https://example.invalid/stable/SHA256SUMS"
      }
    ]
  },
  {
    "tag_name": "v0.1.0",
    "prerelease": false,
    "draft": true,
    "html_url": "https://github.com/Lucenx9/forktty/releases/tag/v0.1.0",
    "assets": []
  }
]
"#;

#[test]
fn updates_config_defaults_to_auto_check_and_loads_explicit_false() {
    let dir = tempfile::tempdir().unwrap();
    let missing = config::load_config_from_path(&dir.path().join("missing.toml")).unwrap();
    assert!(missing.updates.auto_check);

    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
        [general]
        shell = "/bin/sh"

        [updates]
        auto_check = false
        "#,
    )
    .unwrap();

    let loaded = config::load_config_from_path(&path).unwrap();
    assert!(!loaded.updates.auto_check);
}

#[test]
fn prerelease_installations_consider_newer_prereleases() {
    let selected = select_newest_update(RELEASES_JSON, "0.2.0-alpha.12", TargetArch::X86_64)
        .expect("release json parses")
        .expect("newer alpha is available");

    assert_eq!(selected.version.to_string(), "0.3.0-alpha.1");
    assert_eq!(
        selected.release_url,
        "https://github.com/Lucenx9/forktty/releases/tag/v0.3.0-alpha.1"
    );
    assert_eq!(
        selected
            .asset(AssetKind::AppImage)
            .map(|asset| asset.name.as_str()),
        Some("forktty-0.3.0-alpha.1-x86_64.AppImage")
    );
    assert_eq!(
        selected
            .asset(AssetKind::AppImage)
            .map(|asset| asset.browser_download_url.as_str()),
        Some("https://example.invalid/forktty-0.3.0-alpha.1-x86_64.AppImage")
    );
    assert_eq!(
        selected
            .asset(AssetKind::Sha256Sums)
            .map(|asset| asset.browser_download_url.as_str()),
        Some("https://example.invalid/SHA256SUMS")
    );
    assert_eq!(
        selected
            .asset(AssetKind::Zsync)
            .map(|asset| asset.browser_download_url.as_str()),
        Some("https://example.invalid/forktty-0.3.0-alpha.1-x86_64.AppImage.zsync")
    );
}

#[test]
fn stable_installations_ignore_prerelease_candidates() {
    let selected = select_newest_update(RELEASES_JSON, "0.1.0", TargetArch::X86_64)
        .expect("release json parses")
        .expect("newer stable is available");

    assert_eq!(selected.version.to_string(), "0.2.0");
    assert_eq!(
        selected
            .asset(AssetKind::AppImage)
            .map(|asset| asset.name.as_str()),
        Some("forktty-0.2.0-x86_64.AppImage")
    );
}

#[test]
fn no_update_when_current_version_is_newest_for_channel() {
    let selected = select_newest_update(RELEASES_JSON, "0.3.0-alpha.1", TargetArch::X86_64)
        .expect("release json parses");

    assert!(selected.is_none());
}

#[test]
fn release_assets_are_selected_by_name_not_url_construction() {
    let selected = select_newest_update(RELEASES_JSON, "0.2.0-alpha.12", TargetArch::X86_64)
        .unwrap()
        .unwrap();

    let deb = selected.asset(AssetKind::Deb).expect("deb asset exists");
    assert_eq!(
        deb,
        &ReleaseAsset {
            kind: AssetKind::Deb,
            name: "forktty_0.3.0.alpha.1_amd64.deb".to_string(),
            browser_download_url: "https://example.invalid/forktty_0.3.0.alpha.1_amd64.deb"
                .to_string(),
        }
    );
}

#[test]
fn invalid_current_version_is_reported() {
    let err = select_newest_update(RELEASES_JSON, "not-a-version", TargetArch::X86_64).unwrap_err();

    assert!(matches!(err, ReleaseParseError::InvalidCurrentVersion(_)));
}
