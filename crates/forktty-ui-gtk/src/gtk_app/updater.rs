use adw::prelude::*;
use forktty_core::{select_newest_update, AssetKind, AvailableUpdate, ReleaseAsset, TargetArch};
use gtk::glib;
use gtk4 as gtk;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const UPDATE_CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;
const UPDATE_CHECK_POLL_INTERVAL: Duration = Duration::from_millis(250);
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
const RELEASES_URL: &str = "https://api.github.com/repos/Lucenx9/forktty/releases?per_page=10";
const GITHUB_API_VERSION: &str = "2026-03-10";
const RELEASES_JSON_LIMIT_BYTES: u64 = 4 * 1024 * 1024;
const SHA256SUMS_LIMIT_BYTES: u64 = 256 * 1024;
const APPIMAGE_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppImageTarget {
    pub(super) original_path: PathBuf,
    pub(super) canonical_path: PathBuf,
    pub(super) dir: PathBuf,
}

#[derive(Debug)]
pub(super) enum AppImageUpdateError {
    Io(std::io::Error),
    MissingChecksum(String),
    ChecksumMismatch {
        expected: String,
        actual: String,
        asset_name: String,
    },
    DifferentDirectory,
}

impl fmt::Display for AppImageUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::MissingChecksum(asset) => write!(f, "checksum for {asset} was not found"),
            Self::ChecksumMismatch {
                expected,
                actual,
                asset_name,
            } => write!(
                f,
                "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
            ),
            Self::DifferentDirectory => {
                write!(f, "temporary AppImage must be in the target directory")
            }
        }
    }
}

impl std::error::Error for AppImageUpdateError {}

impl From<std::io::Error> for AppImageUpdateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Debug)]
enum UpdateCheckOutcome {
    Available(AvailableUpdate),
    None,
    Failed(String),
}

#[derive(Debug, Clone)]
struct AppImageUpdatePlan {
    target: AppImageTarget,
    appimage: ReleaseAsset,
    sha256sums: ReleaseAsset,
}

#[derive(Debug)]
enum UpdateInstallError {
    Io(io::Error),
    Network(String),
    Http(u16),
    InsecureUrl(String),
    AppImage(AppImageUpdateError),
}

impl fmt::Display for UpdateInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Network(err) => write!(f, "{err}"),
            Self::Http(status) => write!(f, "download failed with HTTP {status}"),
            Self::InsecureUrl(url) => write!(f, "refusing non-HTTPS update URL: {url}"),
            Self::AppImage(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for UpdateInstallError {}

impl From<io::Error> for UpdateInstallError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<AppImageUpdateError> for UpdateInstallError {
    fn from(err: AppImageUpdateError) -> Self {
        Self::AppImage(err)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdateStamp {
    last_attempt_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_after_ms: Option<u64>,
}

pub(super) fn maybe_start_update_check(
    parent: &adw::ApplicationWindow,
    config: &forktty_core::AppConfig,
) {
    if !config.updates.auto_check {
        return;
    }
    let Some(stamp_path) = update_stamp_path() else {
        return;
    };
    let now_ms = current_time_ms();
    if !update_check_due_at(&stamp_path, now_ms) {
        return;
    }
    if let Err(err) = record_update_attempt_at(&stamp_path, now_ms, None) {
        eprintln!("forktty: could not record update check attempt: {err}");
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = check_for_update(&stamp_path, now_ms);
        let _ = tx.send(outcome);
    });

    let parent = parent.clone();
    glib::timeout_add_local(UPDATE_CHECK_POLL_INTERVAL, move || match rx.try_recv() {
        Ok(UpdateCheckOutcome::Available(update)) => {
            show_update_available_dialog(&parent, update);
            glib::ControlFlow::Break
        }
        Ok(UpdateCheckOutcome::None) => glib::ControlFlow::Break,
        Ok(UpdateCheckOutcome::Failed(err)) => {
            eprintln!("forktty: update check failed: {err}");
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

pub(super) fn update_check_due_at(stamp_path: &Path, now_ms: u64) -> bool {
    let Ok(bytes) = fs::read(stamp_path) else {
        return true;
    };
    let Ok(stamp) = serde_json::from_slice::<UpdateStamp>(&bytes) else {
        return true;
    };
    if let Some(deadline) = stamp.retry_after_ms {
        if now_ms <= deadline {
            return false;
        }
    }
    now_ms.saturating_sub(stamp.last_attempt_ms) >= UPDATE_CHECK_INTERVAL_MS
}

pub(super) fn record_update_attempt_at(
    stamp_path: &Path,
    now_ms: u64,
    retry_after_ms: Option<u64>,
) -> std::io::Result<()> {
    if let Some(parent) = stamp_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let stamp = UpdateStamp {
        last_attempt_ms: now_ms,
        retry_after_ms,
    };
    let bytes = serde_json::to_vec_pretty(&stamp)?;
    let tmp_path = stamp_path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        tmp.write_all(&bytes)?;
        tmp.sync_all()?;
        fs::rename(&tmp_path, stamp_path)?;
        if let Some(parent) = stamp_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn update_stamp_path() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .map(|base| base.join("forktty").join("update-check.json"))
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn check_for_update(stamp_path: &Path, attempt_ms: u64) -> UpdateCheckOutcome {
    let agent = github_agent();
    let response = match github_get_text(&agent, RELEASES_URL, RELEASES_JSON_LIMIT_BYTES) {
        Ok(response) => response,
        Err(err) => return UpdateCheckOutcome::Failed(err.to_string()),
    };
    if !(200..=299).contains(&response.status) {
        let retry_after_ms = rate_limit_retry_after_ms(
            response.status,
            response.retry_after.as_deref(),
            response.x_rate_limit_remaining.as_deref(),
            response.x_rate_limit_reset.as_deref(),
            attempt_ms,
        );
        if let Some(deadline) = retry_after_ms {
            if let Err(err) = record_update_attempt_at(stamp_path, attempt_ms, Some(deadline)) {
                eprintln!("forktty: could not record update rate-limit deadline: {err}");
            }
        }
        return UpdateCheckOutcome::Failed(format!(
            "GitHub releases returned HTTP {}",
            response.status
        ));
    }
    let Some(arch) = current_target_arch() else {
        return UpdateCheckOutcome::None;
    };
    match select_newest_update(&response.body, env!("CARGO_PKG_VERSION"), arch) {
        Ok(Some(update)) => UpdateCheckOutcome::Available(update),
        Ok(None) => UpdateCheckOutcome::None,
        Err(err) => UpdateCheckOutcome::Failed(err.to_string()),
    }
}

fn current_target_arch() -> Option<TargetArch> {
    match std::env::consts::ARCH {
        "x86_64" => Some(TargetArch::X86_64),
        "aarch64" => Some(TargetArch::Aarch64),
        _ => None,
    }
}

struct HttpTextResponse {
    status: u16,
    body: String,
    retry_after: Option<String>,
    x_rate_limit_remaining: Option<String>,
    x_rate_limit_reset: Option<String>,
}

fn github_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .https_only(true)
        .build()
        .into()
}

fn github_get_text(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
) -> Result<HttpTextResponse, UpdateInstallError> {
    ensure_https_url(url)?;
    let mut response = github_get(agent, url, "application/vnd.github+json")?;
    let status = response.status().as_u16();
    let retry_after = header_to_string(response.headers(), "retry-after");
    let x_rate_limit_remaining = header_to_string(response.headers(), "x-ratelimit-remaining");
    let x_rate_limit_reset = header_to_string(response.headers(), "x-ratelimit-reset");
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|err| UpdateInstallError::Network(err.to_string()))?;
    Ok(HttpTextResponse {
        status,
        body,
        retry_after,
        x_rate_limit_remaining,
        x_rate_limit_reset,
    })
}

fn github_get(
    agent: &ureq::Agent,
    url: &str,
    accept: &str,
) -> Result<ureq::http::Response<ureq::Body>, UpdateInstallError> {
    ensure_https_url(url)?;
    agent
        .get(url)
        .header(
            "User-Agent",
            format!("ForkTTY/{}", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .call()
        .map_err(|err| UpdateInstallError::Network(err.to_string()))
}

fn header_to_string(headers: &ureq::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn ensure_https_url(url: &str) -> Result<(), UpdateInstallError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(UpdateInstallError::InsecureUrl(url.to_string()))
    }
}

pub(super) fn rate_limit_retry_after_ms(
    status: u16,
    retry_after: Option<&str>,
    x_rate_limit_remaining: Option<&str>,
    x_rate_limit_reset: Option<&str>,
    now_ms: u64,
) -> Option<u64> {
    if !matches!(status, 403 | 429) {
        return None;
    }
    if let Some(seconds) = retry_after.and_then(|value| value.trim().parse::<u64>().ok()) {
        return Some(now_ms.saturating_add(seconds.saturating_mul(1000)));
    }
    if let Some(deadline) = retry_after
        .and_then(|value| chrono::DateTime::parse_from_rfc2822(value.trim()).ok())
        .map(|date| date.timestamp_millis())
        .and_then(|deadline| u64::try_from(deadline).ok())
        .filter(|deadline| *deadline > now_ms)
    {
        return Some(deadline);
    }
    if x_rate_limit_remaining.map(str::trim) == Some("0") {
        return x_rate_limit_reset
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|seconds| seconds.saturating_mul(1000))
            .filter(|deadline| *deadline > now_ms);
    }
    None
}

fn show_update_available_dialog(parent: &adw::ApplicationWindow, update: AvailableUpdate) {
    if let Some(plan) = appimage_update_plan(&update) {
        show_appimage_update_dialog(parent, update, plan);
    } else {
        show_release_page_update_dialog(parent, update);
    }
}

fn show_appimage_update_dialog(
    parent: &adw::ApplicationWindow,
    update: AvailableUpdate,
    plan: AppImageUpdatePlan,
) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading(format!("ForkTTY {} available", update.version))
        .body("The new AppImage can be downloaded, verified with SHA256SUMS, and installed over the current AppImage.")
        .build();
    dialog.add_responses(&[("later", "Later"), ("update", "Update now")]);
    dialog.set_close_response("later");
    dialog.set_default_response(Some("update"));
    dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);

    let parent = parent.clone();
    let release_url = update.release_url.clone();
    dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
        dialog.close();
        if response == "update" {
            start_appimage_update(parent.clone(), plan.clone(), release_url.clone());
        }
    });
    dialog.present();
}

fn show_release_page_update_dialog(parent: &adw::ApplicationWindow, update: AvailableUpdate) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading(format!("ForkTTY {} available", update.version))
        .body("This installation is not a writable AppImage. Open the GitHub release to download the package for your system.")
        .build();
    dialog.add_responses(&[("later", "Later"), ("release", "Open Release")]);
    dialog.set_close_response("later");
    dialog.set_default_response(Some("release"));
    dialog.set_response_appearance("release", adw::ResponseAppearance::Suggested);

    let parent = parent.clone();
    let release_url = update.release_url.clone();
    dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
        dialog.close();
        if response == "release" {
            open_release_page(&parent, &release_url);
        }
    });
    dialog.present();
}

fn appimage_update_plan(update: &AvailableUpdate) -> Option<AppImageUpdatePlan> {
    let appimage_env = std::env::var_os("APPIMAGE");
    let extract_and_run = std::env::var("APPIMAGE_EXTRACT_AND_RUN").ok();
    let target = appimage_target_from_env(
        appimage_env.as_deref().map(Path::new),
        extract_and_run.as_deref(),
    )?;
    Some(AppImageUpdatePlan {
        target,
        appimage: update.asset(AssetKind::AppImage)?.clone(),
        sha256sums: update.asset(AssetKind::Sha256Sums)?.clone(),
    })
}

fn start_appimage_update(
    parent: adw::ApplicationWindow,
    plan: AppImageUpdatePlan,
    release_url: String,
) {
    let progress = adw::MessageDialog::builder()
        .transient_for(&parent)
        .modal(true)
        .heading("Updating ForkTTY")
        .body("Downloading and verifying the new AppImage...")
        .build();
    progress.add_response("hide", "Hide");
    progress.set_close_response("hide");
    progress.present();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = download_and_install_appimage_update(plan);
        let _ = tx.send(result);
    });

    glib::timeout_add_local(UPDATE_CHECK_POLL_INTERVAL, move || match rx.try_recv() {
        Ok(Ok(target)) => {
            progress.close();
            show_restart_dialog(&parent, target);
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            progress.close();
            show_update_failed_dialog(&parent, &release_url, &err);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            progress.close();
            glib::ControlFlow::Break
        }
    });
}

fn show_restart_dialog(parent: &adw::ApplicationWindow, target: AppImageTarget) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading("ForkTTY update installed")
        .body("Restart ForkTTY to run the updated AppImage.")
        .build();
    dialog.add_responses(&[("later", "Later"), ("restart", "Restart")]);
    dialog.set_close_response("later");
    dialog.set_default_response(Some("restart"));
    dialog.set_response_appearance("restart", adw::ResponseAppearance::Suggested);

    let parent = parent.clone();
    dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
        dialog.close();
        if response == "restart" {
            restart_from_appimage(&parent, &target);
        }
    });
    dialog.present();
}

fn show_update_failed_dialog(
    parent: &adw::ApplicationWindow,
    release_url: &str,
    err: &UpdateInstallError,
) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading("ForkTTY update failed")
        .body(format!(
            "{err}\n\nThe current AppImage was left untouched. Open the release page to install manually."
        ))
        .build();
    dialog.add_responses(&[("close", "Close"), ("release", "Open Release")]);
    dialog.set_close_response("close");
    dialog.set_response_appearance("release", adw::ResponseAppearance::Suggested);

    let parent = parent.clone();
    let release_url = release_url.to_string();
    dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
        dialog.close();
        if response == "release" {
            open_release_page(&parent, &release_url);
        }
    });
    dialog.present();
}

fn open_release_page(parent: &adw::ApplicationWindow, release_url: &str) {
    gtk::show_uri(
        Some(parent.upcast_ref::<gtk::Window>()),
        release_url,
        gtk::gdk::CURRENT_TIME,
    );
}

fn restart_from_appimage(parent: &adw::ApplicationWindow, target: &AppImageTarget) {
    match Command::new(&target.original_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => shutdown_after_appimage_restart(parent),
        Err(err) => {
            let dialog = adw::MessageDialog::builder()
                .transient_for(parent)
                .modal(true)
                .heading("Could not restart ForkTTY")
                .body(format!(
                    "{err}\n\nStart the updated AppImage manually from its install location."
                ))
                .build();
            dialog.add_response("close", "Close");
            dialog.set_close_response("close");
            dialog.present();
        }
    }
}

fn shutdown_after_appimage_restart(parent: &adw::ApplicationWindow) {
    parent.close();
}

pub(super) fn appimage_target_from_env(
    appimage: Option<&Path>,
    extract_and_run: Option<&str>,
) -> Option<AppImageTarget> {
    if extract_and_run == Some("1") {
        return None;
    }
    let original_path = appimage?.to_path_buf();
    let canonical_path = fs::canonicalize(&original_path).ok()?;
    let metadata = fs::metadata(&canonical_path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let dir = canonical_path.parent()?.to_path_buf();
    if !directory_accepts_temp_file(&dir) {
        return None;
    }
    Some(AppImageTarget {
        original_path,
        canonical_path,
        dir,
    })
}

fn directory_accepts_temp_file(dir: &Path) -> bool {
    let path = dir.join(format!(".forktty-write-test-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => {
            let _ = file.sync_all();
            let _ = fs::remove_file(path);
            true
        }
        Err(_) => false,
    }
}

fn download_and_install_appimage_update(
    plan: AppImageUpdatePlan,
) -> Result<AppImageTarget, UpdateInstallError> {
    let agent = github_agent();
    let target = plan.target.clone();
    let temp = update_temp_path(&target, &plan.appimage.name);
    let result = (|| {
        download_asset_to_file(&agent, &plan.appimage, &temp)?;
        let sha256sums = download_asset_text(&agent, &plan.sha256sums, SHA256SUMS_LIMIT_BYTES)?;
        replace_appimage_with_verified_temp(
            &target.canonical_path,
            &temp,
            &plan.appimage.name,
            &sha256sums,
        )?;
        Ok(target.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn download_asset_text(
    agent: &ureq::Agent,
    asset: &ReleaseAsset,
    limit: u64,
) -> Result<String, UpdateInstallError> {
    let response = github_get_text(agent, &asset.browser_download_url, limit)?;
    if !(200..=299).contains(&response.status) {
        return Err(UpdateInstallError::Http(response.status));
    }
    Ok(response.body)
}

fn download_asset_to_file(
    agent: &ureq::Agent,
    asset: &ReleaseAsset,
    target: &Path,
) -> Result<(), UpdateInstallError> {
    ensure_https_url(&asset.browser_download_url)?;
    let mut response = github_get(
        agent,
        &asset.browser_download_url,
        "application/octet-stream",
    )?;
    let status = response.status().as_u16();
    if !(200..=299).contains(&status) {
        return Err(UpdateInstallError::Http(status));
    }
    let mut file = create_private_update_file(target)?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(APPIMAGE_LIMIT_BYTES)
        .reader();
    io::copy(&mut reader, &mut file)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn create_private_update_file(target: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(target)
}

fn update_temp_path(target: &AppImageTarget, asset_name: &str) -> PathBuf {
    let safe_name = asset_name
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' | '_' => ch,
            _ => '_',
        })
        .collect::<String>();
    target.dir.join(format!(
        ".forktty-update-{}-{}-{safe_name}.tmp",
        std::process::id(),
        current_time_ms()
    ))
}

pub(super) fn replace_appimage_with_verified_temp(
    target: &Path,
    temp: &Path,
    asset_name: &str,
    sha256sums: &str,
) -> Result<(), AppImageUpdateError> {
    let result = (|| {
        if target.parent() != temp.parent() {
            return Err(AppImageUpdateError::DifferentDirectory);
        }
        let expected = checksum_for_asset(sha256sums, asset_name)
            .ok_or_else(|| AppImageUpdateError::MissingChecksum(asset_name.to_string()))?;
        let actual = sha256_file_hex(temp)?;
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(AppImageUpdateError::ChecksumMismatch {
                expected,
                actual,
                asset_name: asset_name.to_string(),
            });
        }

        #[cfg(unix)]
        fs::set_permissions(temp, fs::Permissions::from_mode(0o755))?;
        fs::File::open(temp)?.sync_all()?;
        fs::rename(temp, target)?;
        if let Some(parent) = target.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn checksum_for_asset(sha256sums: &str, asset_name: &str) -> Option<String> {
    sha256sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let checksum = fields.next()?;
        let file = fields
            .next()?
            .trim_start_matches('*')
            .trim_start_matches("./");
        (file == asset_name).then(|| checksum.to_string())
    })
}

fn sha256_file_hex(path: &Path) -> Result<String, std::io::Error> {
    let bytes = fs::read(path)?;
    Ok(sha256_hex(&bytes))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "gtk-ghostty")]
    #[test]
    fn appimage_restart_runs_window_close_request_cleanup() {
        let _ = crate::test_env::with_gtk_test(|| {
            use std::cell::Cell;
            use std::rc::Rc;

            let app = adw::Application::builder()
                .application_id("dev.forktty.forktty.UpdaterTest")
                .build();
            app.register(None::<&gtk::gio::Cancellable>).unwrap();
            let parent = adw::ApplicationWindow::builder().application(&app).build();
            let close_requested = Rc::new(Cell::new(false));
            let close_requested_for_signal = Rc::clone(&close_requested);
            parent.connect_close_request(move |_| {
                close_requested_for_signal.set(true);
                glib::Propagation::Proceed
            });
            parent.present();
            while glib::MainContext::default().iteration(false) {}

            shutdown_after_appimage_restart(&parent);
            while glib::MainContext::default().iteration(false) {}

            assert!(close_requested.get());
        });
    }

    #[test]
    fn rate_limit_retry_after_accepts_http_date() {
        let now_ms = 1_781_524_800_000;

        assert_eq!(
            rate_limit_retry_after_ms(
                429,
                Some("Mon, 15 Jun 2026 12:00:05 GMT"),
                None,
                None,
                now_ms,
            ),
            Some(now_ms + 5_000)
        );
    }
}
