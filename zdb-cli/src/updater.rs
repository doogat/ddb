use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const GITHUB_RELEASES_API: &str = "https://api.github.com/repos/doogat/zdb/releases/latest";
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60); // 1 hour
const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024; // 100 MB
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateState {
    pub last_check: String,
    pub latest_version: String,
    pub download_url: String,
    /// Set after a background auto-update; cleared after notification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_from: Option<String>,
    /// Version of the binary backed up at `config_dir()/zdb.previous`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".config")
        })
        .join("zetteldb")
}

fn state_path() -> PathBuf {
    config_dir().join("update-check.json")
}

fn backup_path() -> PathBuf {
    config_dir().join("zdb.previous")
}

#[derive(Deserialize, Default)]
struct UpdateSection {
    auto: Option<bool>,
}

#[derive(Deserialize, Default)]
struct CliConfig {
    update: Option<UpdateSection>,
}

fn auto_update_enabled() -> bool {
    let path = config_dir().join("config.toml");
    let Ok(contents) = fs::read_to_string(path) else {
        return true;
    };
    let Ok(cfg) = toml::from_str::<CliConfig>(&contents) else {
        return true;
    };
    cfg.update.and_then(|u| u.auto).unwrap_or(true)
}

fn backup_current_binary() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate current binary: {e}"))?;
    let dest = backup_path();
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create config dir: {e}"))?;
    fs::copy(&exe, &dest).map_err(|e| format!("backup failed: {e}"))?;
    Ok(())
}

fn read_state() -> Option<UpdateState> {
    let data = fs::read_to_string(state_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn write_state(state: &UpdateState) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let tmp = dir.join("update-check.json.tmp");
    let target = dir.join("update-check.json");
    if let Ok(data) = serde_json::to_string_pretty(state) {
        if fs::write(&tmp, data).is_ok() {
            let _ = fs::rename(&tmp, &target);
        }
    }
}

fn should_check() -> bool {
    let state = match read_state() {
        Some(s) => s,
        None => return true,
    };
    let last: chrono::DateTime<chrono::Utc> = match state.last_check.parse() {
        Ok(t) => t,
        Err(_) => return true,
    };
    let elapsed = chrono::Utc::now().signed_duration_since(last);
    elapsed.to_std().unwrap_or(CHECK_INTERVAL) >= CHECK_INTERVAL
}

fn target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Some("aarch64-apple-darwin");
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Some("x86_64-apple-darwin");
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return Some("x86_64-unknown-linux-gnu");
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        return Some("aarch64-unknown-linux-gnu");
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return Some("x86_64-pc-windows-msvc");
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        return Some("aarch64-pc-windows-msvc");
    }
    #[allow(unreachable_code)]
    None
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("zdb/{CURRENT_VERSION}"))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))
}

fn fetch_latest_release() -> Result<(Version, String), String> {
    let triple = target_triple().ok_or("unsupported platform")?;
    let client = http_client()?;
    let resp: GitHubRelease = client
        .get(GITHUB_RELEASES_API)
        .send()
        .map_err(|e| format!("request failed: {e}"))?
        .json()
        .map_err(|e| format!("invalid response: {e}"))?;

    let version_str = resp.tag_name.strip_prefix('v').unwrap_or(&resp.tag_name);
    let version: Version = version_str
        .parse()
        .map_err(|e| format!("invalid version \"{version_str}\": {e}"))?;

    let expected_name = format!("zdb-v{version}-{triple}.tar.gz");
    let asset = resp
        .assets
        .iter()
        .find(|a| a.name == expected_name)
        .ok_or_else(|| format!("no asset for {triple}"))?;

    Ok((version, asset.browser_download_url.clone()))
}

/// Background entry point: check for update and auto-apply if newer. Silent on error.
pub fn check_and_update() {
    let now = chrono::Utc::now().to_rfc3339();
    let current: Version = match CURRENT_VERSION.parse() {
        Ok(v) => v,
        Err(_) => return,
    };

    match fetch_latest_release() {
        Ok((version, url)) => {
            if version > current && auto_update_enabled() {
                // Attempt auto-update
                if download_and_replace(&url, &version).is_ok() {
                    let state = UpdateState {
                        last_check: now,
                        latest_version: version.to_string(),
                        download_url: url,
                        updated_from: Some(current.to_string()),
                        previous_version: Some(current.to_string()),
                    };
                    write_state(&state);
                    return;
                }
            }
            // No update needed or download failed — just record check
            let prev = read_state();
            let state = UpdateState {
                last_check: now,
                latest_version: version.to_string(),
                download_url: url,
                updated_from: prev.as_ref().and_then(|s| s.updated_from.clone()),
                previous_version: prev.and_then(|s| s.previous_version),
            };
            write_state(&state);
        }
        Err(_) => {
            // Update last_check to avoid retrying on every command when offline
            if let Some(mut state) = read_state() {
                state.last_check = now;
                write_state(&state);
            } else {
                let state = UpdateState {
                    last_check: now,
                    latest_version: String::new(),
                    download_url: String::new(),
                    updated_from: None,
                    previous_version: None,
                };
                write_state(&state);
            }
        }
    }
}

/// Spawn detached `zdb __update-check` process.
pub fn spawn_background_check() {
    if !should_check() {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("__update-check")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Create new process group so the child survives parent exit
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

/// Notify user if an auto-update was applied, then clear the flag.
pub fn notify_if_updated() {
    let mut state = match read_state() {
        Some(s) => s,
        None => return,
    };
    if let Some(old_version) = state.updated_from.take() {
        eprintln!(
            "zdb updated v{old_version} -> v{}. restart your shell to use the new version.",
            state.latest_version
        );
        write_state(&state);
    }
}

/// Synchronous update: check -> download -> verify -> extract -> replace.
pub fn run_update() -> Result<(), String> {
    let current: Version = CURRENT_VERSION
        .parse()
        .map_err(|e| format!("bad current version: {e}"))?;

    eprintln!("checking for updates...");
    let (latest, url) = fetch_latest_release()?;

    if latest <= current {
        eprintln!("zdb v{current} is already the latest version.");
        return Ok(());
    }

    eprintln!("downloading zdb v{latest}...");
    download_and_replace(&url, &latest)?;

    // Update state file
    let state = UpdateState {
        last_check: chrono::Utc::now().to_rfc3339(),
        latest_version: latest.to_string(),
        download_url: url,
        updated_from: None, // explicit command, no deferred notification needed
        previous_version: Some(current.to_string()),
    };
    write_state(&state);

    eprintln!("updated zdb v{current} -> v{latest}. restart your shell to use the new version.");
    Ok(())
}

/// Restore the previously backed-up binary.
pub fn rollback() -> Result<(), String> {
    let backup = backup_path();
    if !backup.exists() {
        return Err("no backup found — nothing to roll back".into());
    }

    let prev_version = read_state()
        .and_then(|s| s.previous_version.clone())
        .unwrap_or_else(|| "unknown".into());

    self_replace::self_replace(&backup).map_err(|e| {
        format!("rollback failed: {e}. manually copy {} over the zdb binary", backup.display())
    })?;

    // Clear update-related state but keep the backup file
    if let Some(mut state) = read_state() {
        state.updated_from = None;
        state.previous_version = None;
        write_state(&state);
    }

    eprintln!("rolled back to v{prev_version}. restart your shell to use the restored version.");
    Ok(())
}

fn download_and_replace(url: &str, expected_version: &Version) -> Result<(), String> {
    let client = http_client()?;

    // Download archive
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download returned {}", resp.status()));
    }
    let mut archive_bytes = Vec::new();
    resp.by_ref()
        .take(MAX_DOWNLOAD_BYTES + 1)
        .read_to_end(&mut archive_bytes)
        .map_err(|e| format!("read failed: {e}"))?;
    if archive_bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err(format!(
            "download exceeds {}MB limit",
            MAX_DOWNLOAD_BYTES / 1024 / 1024
        ));
    }

    // Verify SHA-256 checksum
    let checksum_url = format!("{url}.sha256");
    let checksum_resp = client
        .get(&checksum_url)
        .send()
        .map_err(|e| format!("checksum fetch failed: {e}"))?;
    if !checksum_resp.status().is_success() {
        return Err(format!(
            "checksum download returned {}",
            checksum_resp.status()
        ));
    } else {
        let text = checksum_resp
            .text()
            .map_err(|e| format!("checksum read failed: {e}"))?;
        let expected_hash = text
            .split_whitespace()
            .next()
            .filter(|s| !s.is_empty())
            .ok_or("checksum file is empty")?;
        let actual_hash = hex::encode(Sha256::digest(&archive_bytes));
        if actual_hash != expected_hash {
            return Err(format!(
                "SHA-256 mismatch: expected {expected_hash}, got {actual_hash}"
            ));
        }
    }

    // Extract binary from tar.gz
    let decoder = flate2::read::GzDecoder::new(&archive_bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    let tmp_dir = std::env::temp_dir().join(format!("zdb-update-{}", std::process::id()));
    let _ = fs::create_dir_all(&tmp_dir);

    let mut found_binary = None;
    for entry in archive.entries().map_err(|e| format!("tar error: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar entry error: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("tar path error: {e}"))?
            .to_path_buf();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "zdb" || name == "zdb.exe" {
            let dest = tmp_dir.join("zdb");
            entry
                .unpack(&dest)
                .map_err(|e| format!("extract failed: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
            }
            found_binary = Some(dest);
            break;
        }
    }
    let binary_path = found_binary.ok_or("archive does not contain a `zdb` binary")?;

    // Verify extracted binary runs
    let output = std::process::Command::new(&binary_path)
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to verify new binary: {e}"))?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err("new binary failed --version check".into());
    }
    let version_output = String::from_utf8_lossy(&output.stdout);
    if !version_output.contains(&expected_version.to_string()) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "binary version mismatch: expected {expected_version}, got: {version_output}"
        ));
    }

    // Back up current binary before replacing
    backup_current_binary()?;

    // Replace self
    self_replace::self_replace(&binary_path).map_err(|e| {
        format!(
            "self-replace failed: {e}. download manually from https://github.com/doogat/zdb/releases"
        )
    })?;

    let _ = fs::remove_dir_all(&tmp_dir);
    Ok(())
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_empty() {
        assert_eq!(hex::encode(b""), "");
    }

    #[test]
    fn hex_encode_known() {
        assert_eq!(hex::encode(b"\xde\xad\xbe\xef"), "deadbeef");
        assert_eq!(hex::encode(b"\x00\xff"), "00ff");
    }

    #[test]
    fn state_round_trip() {
        let state = UpdateState {
            last_check: "2026-03-04T12:00:00Z".into(),
            latest_version: "0.2.0".into(),
            download_url: "https://example.com/zdb.tar.gz".into(),
            updated_from: None,
            previous_version: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: UpdateState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_check, state.last_check);
        assert_eq!(parsed.latest_version, state.latest_version);
        assert_eq!(parsed.download_url, state.download_url);
        assert!(parsed.updated_from.is_none());
        assert!(parsed.previous_version.is_none());
    }

    #[test]
    fn state_round_trip_with_updated_from() {
        let state = UpdateState {
            last_check: "2026-03-04T12:00:00Z".into(),
            latest_version: "0.2.0".into(),
            download_url: "https://example.com/zdb.tar.gz".into(),
            updated_from: Some("0.1.1".into()),
            previous_version: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: UpdateState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.updated_from, Some("0.1.1".into()));
    }

    #[test]
    fn state_deserialize_handles_extra_fields() {
        let json = r#"{"last_check":"2026-01-01T00:00:00Z","latest_version":"1.0.0","download_url":"https://x.com","extra":"ignored"}"#;
        let state: UpdateState = serde_json::from_str(json).unwrap();
        assert_eq!(state.latest_version, "1.0.0");
        assert!(state.updated_from.is_none());
    }

    #[test]
    fn state_deserialize_without_updated_from() {
        // Old state files won't have updated_from — should default to None
        let json = r#"{"last_check":"2026-01-01T00:00:00Z","latest_version":"1.0.0","download_url":"https://x.com"}"#;
        let state: UpdateState = serde_json::from_str(json).unwrap();
        assert!(state.updated_from.is_none());
    }

    #[test]
    fn target_triple_returns_some() {
        let triple = target_triple();
        assert!(
            triple.is_some(),
            "target_triple() returned None on this platform"
        );
        let t = triple.unwrap();
        assert!(t.contains("apple") || t.contains("linux") || t.contains("windows"));
    }

    #[test]
    fn version_comparison_newer() {
        let current: Version = "0.1.1".parse().unwrap();
        let latest: Version = "0.2.0".parse().unwrap();
        assert!(latest > current);
    }

    #[test]
    fn version_comparison_same() {
        let current: Version = "0.1.1".parse().unwrap();
        let latest: Version = "0.1.1".parse().unwrap();
        assert!(latest <= current);
    }

    #[test]
    fn version_comparison_older() {
        let current: Version = "0.2.0".parse().unwrap();
        let latest: Version = "0.1.1".parse().unwrap();
        assert!(latest <= current);
    }

    #[test]
    fn empty_version_string_does_not_parse() {
        assert!("".parse::<Version>().is_err());
    }

    #[test]
    fn sha256_checksum_matches() {
        use sha2::{Digest, Sha256};
        let data = b"hello world";
        let hash = hex::encode(Sha256::digest(data));
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn state_read_write_with_temp_dir() {
        let dir = std::env::temp_dir().join(format!("zdb-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("update-check.json");

        let state = UpdateState {
            last_check: chrono::Utc::now().to_rfc3339(),
            latest_version: "0.3.0".into(),
            download_url: "https://example.com/zdb.tar.gz".into(),
            updated_from: None,
            previous_version: Some("0.2.0".into()),
        };

        let data = serde_json::to_string_pretty(&state).unwrap();
        fs::write(&path, &data).unwrap();

        let read_data = fs::read_to_string(&path).unwrap();
        let parsed: UpdateState = serde_json::from_str(&read_data).unwrap();
        assert_eq!(parsed.latest_version, "0.3.0");
        assert_eq!(parsed.previous_version, Some("0.2.0".into()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_interval_is_1h() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(3600));
    }

    #[test]
    fn current_version_parses() {
        let v: Version = CURRENT_VERSION
            .parse()
            .expect("CARGO_PKG_VERSION should be valid semver");
        assert!(v.major == 0 || v.major >= 1);
    }
}
