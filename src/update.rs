//! Self-update from GitHub Releases (Windows only).

#![cfg_attr(not(windows), allow(dead_code))]

use serde::Deserialize;

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/lgpasquale/greedygoblin-app/releases/latest";
const ASSET_NAME: &str = "greedygoblin-app.exe";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Parses `v1.2.3` or `1.2.3`.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.strip_prefix('v').unwrap_or(s).split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(tag), Some(current)) => tag > current,
        _ => false,
    }
}

/// Checks for a newer release and, if there is one, replaces the running
/// executable with it. Returns whether it did: the caller must then restart.
#[cfg(windows)]
pub fn update() -> Result<bool, String> {
    use std::time::Duration;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(concat!("greedygoblin-app/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let release: Release = agent
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .and_then(|mut r| r.body_mut().read_json())
        .map_err(|e| format!("GET {LATEST_RELEASE_URL}: {e}"))?;
    if !is_newer(&release.tag_name, env!("CARGO_PKG_VERSION")) {
        return Ok(false);
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == ASSET_NAME)
        .ok_or_else(|| format!("release {} has no {ASSET_NAME}", release.tag_name))?;
    let exe = agent
        .get(&asset.browser_download_url)
        .call()
        .and_then(|mut r| {
            r.body_mut()
                .with_config()
                .limit(200 * 1024 * 1024)
                .read_to_vec()
        })
        .map_err(|e| format!("GET {}: {e}", asset.browser_download_url))?;
    if !exe.starts_with(b"MZ") {
        return Err(format!(
            "{} is not an executable",
            asset.browser_download_url
        ));
    }
    let tmp = std::env::temp_dir().join(format!("greedygoblin-app-{}.exe", release.tag_name));
    std::fs::write(&tmp, &exe).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let replaced =
        self_replace::self_replace(&tmp).map_err(|e| format!("cannot replace the executable: {e}"));
    let _ = std::fs::remove_file(&tmp);
    replaced?;
    crate::log!("updated to {}", release.tag_name);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }
}
