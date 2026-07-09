use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TELEMETRY_ENDPOINT: &str = "https://forktty.dev/api/telemetry/ping";
const TELEMETRY_STAMP_FILE: &str = "telemetry-ping.json";
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelemetryStamp {
    last_ping_date: String,
}

#[allow(dead_code)]
pub fn maybe_start_anonymous_ping(config: &forktty_core::AppConfig) {
    maybe_start_anonymous_ping_with(config, telemetry_stamp_path(), now_unix_seconds(), true);
}

fn maybe_start_anonymous_ping_with(
    config: &forktty_core::AppConfig,
    stamp_path: Option<PathBuf>,
    now_seconds: i64,
    dispatch_ping: bool,
) {
    if !config.telemetry.anonymous_ping || !valid_endpoint(TELEMETRY_ENDPOINT) {
        return;
    }
    let Some(stamp_path) = stamp_path else {
        return;
    };
    let today = utc_date_from_unix_seconds(now_seconds);
    let last_ping_date = read_last_ping_date(&stamp_path);
    if !ping_due(true, last_ping_date.as_deref(), &today) {
        return;
    }
    if record_ping_attempt(&stamp_path, &today).is_err() {
        return;
    }

    if !dispatch_ping {
        return;
    }

    let version = env!("CARGO_PKG_VERSION").to_string();
    std::thread::spawn(move || {
        let _ = send_daily_ping(TELEMETRY_ENDPOINT, &version, &today);
    });
}

#[allow(dead_code)]
fn telemetry_stamp_path() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("forktty").join(TELEMETRY_STAMP_FILE))
}

#[allow(dead_code)]
fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn utc_date_from_unix_seconds(seconds: i64) -> String {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().expect("epoch timestamp"))
        .format("%Y-%m-%d")
        .to_string()
}

fn ping_due(enabled: bool, last_ping_date: Option<&str>, today: &str) -> bool {
    enabled && last_ping_date != Some(today)
}

fn daily_ping_payload(version: &str, date: &str) -> Value {
    json!({
        "schema": 1,
        "kind": "daily_ping",
        "app": "forktty",
        "version": version,
        "date": date,
    })
}

fn read_last_ping_date(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let stamp = serde_json::from_slice::<TelemetryStamp>(&bytes).ok()?;
    Some(stamp.last_ping_date)
}

fn record_ping_attempt(path: &Path, date: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let stamp = TelemetryStamp {
        last_ping_date: date.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&stamp)?;
    let tmp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        tmp.write_all(&bytes)?;
        tmp.sync_all()?;
        fs::rename(&tmp_path, path)?;
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn send_daily_ping(endpoint: &str, version: &str, date: &str) -> Result<(), String> {
    if !valid_endpoint(endpoint) {
        return Err("telemetry endpoint must use https".to_string());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .https_only(true)
        .build()
        .into();
    let response = agent
        .post(endpoint)
        .header("User-Agent", format!("ForkTTY/{version}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(daily_ping_payload(version, date).to_string())
        .map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    if (200..=299).contains(&status) {
        Ok(())
    } else {
        Err(format!("telemetry endpoint returned HTTP {status}"))
    }
}

fn valid_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_contains_only_anonymous_daily_fields() {
        let payload = daily_ping_payload("0.2.0-alpha.12", "2026-06-13");

        assert_eq!(
            payload,
            serde_json::json!({
                "schema": 1,
                "kind": "daily_ping",
                "app": "forktty",
                "version": "0.2.0-alpha.12",
                "date": "2026-06-13",
            })
        );
    }

    #[test]
    fn ping_due_respects_disabled_config_and_same_day_stamp() {
        assert!(!ping_due(false, None, "2026-06-13"));
        assert!(ping_due(true, None, "2026-06-13"));
        assert!(!ping_due(true, Some("2026-06-13"), "2026-06-13"));
        assert!(ping_due(true, Some("2026-06-12"), "2026-06-13"));
    }

    #[test]
    fn utc_date_uses_calendar_day() {
        assert_eq!(utc_date_from_unix_seconds(0), "1970-01-01");
        assert_eq!(utc_date_from_unix_seconds(1_765_584_000), "2025-12-13");
    }

    #[test]
    fn stamp_round_trips_last_ping_date() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("telemetry-ping.json");

        assert_eq!(read_last_ping_date(&path), None);

        record_ping_attempt(&path, "2026-06-13").expect("record stamp");

        assert_eq!(read_last_ping_date(&path).as_deref(), Some("2026-06-13"));
    }

    #[test]
    fn endpoint_must_be_https() {
        assert!(valid_endpoint("https://forktty.dev/api/telemetry/ping"));
        assert!(!valid_endpoint("http://forktty.dev/api/telemetry/ping"));
    }

    #[test]
    fn test_maybe_start_anonymous_ping_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stamp_path = dir.path().join(TELEMETRY_STAMP_FILE);

        let mut config = forktty_core::AppConfig::default();
        config.telemetry.anonymous_ping = false;

        maybe_start_anonymous_ping_with(&config, Some(stamp_path.clone()), 1_780_000_000, false);

        assert!(!stamp_path.exists());
    }

    #[test]
    fn test_maybe_start_anonymous_ping_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stamp_path = dir.path().join(TELEMETRY_STAMP_FILE);
        let now = 1_780_000_000;

        let mut config = forktty_core::AppConfig::default();
        config.telemetry.anonymous_ping = true;

        maybe_start_anonymous_ping_with(&config, Some(stamp_path.clone()), now, false);

        assert!(stamp_path.exists());
        let date = read_last_ping_date(&stamp_path).unwrap();
        assert_eq!(date, utc_date_from_unix_seconds(now));
    }

    #[test]
    fn test_maybe_start_anonymous_ping_updates_if_old() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stamp_path = dir.path().join(TELEMETRY_STAMP_FILE);
        let now = 1_780_000_000;

        record_ping_attempt(&stamp_path, "1999-01-01").expect("record stamp");

        let mut config = forktty_core::AppConfig::default();
        config.telemetry.anonymous_ping = true;

        maybe_start_anonymous_ping_with(&config, Some(stamp_path.clone()), now, false);

        let date = read_last_ping_date(&stamp_path).unwrap();
        assert_eq!(date, utc_date_from_unix_seconds(now));
    }

    #[test]
    fn test_maybe_start_anonymous_ping_skips_if_recently_pinged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stamp_path = dir.path().join(TELEMETRY_STAMP_FILE);
        let now = 1_780_000_000;
        let today = utc_date_from_unix_seconds(now);

        let custom_json = json!({
            "last_ping_date": today,
            "custom_marker": "do_not_overwrite",
        });
        fs::write(
            &stamp_path,
            serde_json::to_vec_pretty(&custom_json).unwrap(),
        )
        .unwrap();

        let mut config = forktty_core::AppConfig::default();
        config.telemetry.anonymous_ping = true;

        maybe_start_anonymous_ping_with(&config, Some(stamp_path.clone()), now, false);

        let bytes = fs::read(&stamp_path).unwrap();
        let stamp: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stamp["custom_marker"], "do_not_overwrite");
    }
}
