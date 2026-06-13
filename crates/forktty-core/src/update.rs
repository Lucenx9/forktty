use semver::Version;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReleaseParseError {
    #[error("invalid GitHub releases JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("invalid current ForkTTY version: {0}")]
    InvalidCurrentVersion(#[source] semver::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetArch {
    X86_64,
    Aarch64,
}

impl TargetArch {
    pub fn appimage_suffix(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    pub fn deb_suffix(self) -> &'static str {
        match self {
            Self::X86_64 => "amd64",
            Self::Aarch64 => "arm64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    AppImage,
    Deb,
    Sha256Sums,
    Zsync,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub kind: AssetKind,
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub release_url: String,
    pub assets: Vec<ReleaseAsset>,
}

impl AvailableUpdate {
    pub fn asset(&self, kind: AssetKind) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.kind == kind)
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub fn select_newest_update(
    releases_json: &str,
    current_version: &str,
    arch: TargetArch,
) -> Result<Option<AvailableUpdate>, ReleaseParseError> {
    let current =
        Version::parse(current_version).map_err(ReleaseParseError::InvalidCurrentVersion)?;
    let include_prereleases = !current.pre.is_empty();
    let releases: Vec<GithubRelease> = serde_json::from_str(releases_json)?;

    let mut newest: Option<AvailableUpdate> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Some(version) = parse_release_version(&release.tag_name) else {
            continue;
        };
        if version <= current {
            continue;
        }
        if !include_prereleases && (release.prerelease || !version.pre.is_empty()) {
            continue;
        }
        let assets = select_assets(&release.assets, arch);
        if !has_required_assets(&assets) {
            continue;
        }
        let candidate = AvailableUpdate {
            version,
            release_url: release.html_url,
            assets,
        };
        if newest
            .as_ref()
            .is_none_or(|existing| candidate.version > existing.version)
        {
            newest = Some(candidate);
        }
    }

    Ok(newest)
}

fn parse_release_version(tag_name: &str) -> Option<Version> {
    Version::parse(tag_name.trim().trim_start_matches('v')).ok()
}

fn select_assets(assets: &[GithubAsset], arch: TargetArch) -> Vec<ReleaseAsset> {
    let mut selected = Vec::new();
    for asset in assets {
        let Some(kind) = classify_asset_name(&asset.name, arch) else {
            continue;
        };
        if selected
            .iter()
            .any(|selected: &ReleaseAsset| selected.kind == kind)
        {
            continue;
        }
        selected.push(ReleaseAsset {
            kind,
            name: asset.name.clone(),
            browser_download_url: asset.browser_download_url.clone(),
        });
    }
    selected
}

fn classify_asset_name(name: &str, arch: TargetArch) -> Option<AssetKind> {
    if name == "SHA256SUMS" {
        return Some(AssetKind::Sha256Sums);
    }
    if name.starts_with("forktty-")
        && name.ends_with(&format!("-{}.AppImage", arch.appimage_suffix()))
    {
        return Some(AssetKind::AppImage);
    }
    if name.starts_with("forktty-")
        && name.ends_with(&format!("-{}.AppImage.zsync", arch.appimage_suffix()))
    {
        return Some(AssetKind::Zsync);
    }
    if name.starts_with("forktty_") && name.ends_with(&format!("_{}.deb", arch.deb_suffix())) {
        return Some(AssetKind::Deb);
    }
    None
}

fn has_required_assets(assets: &[ReleaseAsset]) -> bool {
    assets.iter().any(|asset| asset.kind == AssetKind::AppImage)
        && assets
            .iter()
            .any(|asset| asset.kind == AssetKind::Sha256Sums)
}
