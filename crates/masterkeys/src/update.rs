//! Update check against the GitHub Releases of I-MnM-I/MasterKeys, and (on Windows) installing one.

pub const LATEST_URL: &str = "https://api.github.com/repos/I-MnM-I/MasterKeys/releases/latest";
pub const ZIP_NAME: &str = "MasterKeys-windows.zip";
pub const SUMS_NAME: &str = "SHA256SUMS";
/// Passed to the new version, which then waits for this one to quit before claiming the window.
pub const REPLACING_ARG: &str = "--replacing";

/// The latest release, as far as updating needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub page_url: String,
    pub zip_url: Option<String>,
    pub sums_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(serde::Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// From the GitHub API's JSON for one release.
    pub fn from_json(json: &str) -> Result<Release, String> {
        let r: ApiRelease = serde_json::from_str(json).map_err(|e| format!("unexpected release data: {e}"))?;
        let asset = |name: &str| r.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
        Ok(Release { zip_url: asset(ZIP_NAME), sums_url: asset(SUMS_NAME), tag: r.tag_name, page_url: r.html_url })
    }

    /// Whether the app can install it itself (both files are attached).
    pub fn can_install(&self) -> bool {
        self.zip_url.is_some() && self.sums_url.is_some()
    }

    fn version(&self) -> Option<semver::Version> {
        parse_version(&self.tag)
    }
}

fn parse_version(s: &str) -> Option<semver::Version> {
    semver::Version::parse(s.trim().trim_start_matches('v')).ok()
}

/// Whether `tag` is a newer version than `current`; tags that aren't versions never are.
pub fn is_newer(current: &str, tag: &str) -> bool {
    matches!((parse_version(current), parse_version(tag)), (Some(c), Some(t)) if t > c)
}

/// Check `bytes` against the `sha256sum`-style line for `file` in `sums`.
pub fn check_sum(bytes: &[u8], sums: &str, file: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let want = sums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == file)
        .map(|(hash, _)| hash.to_ascii_lowercase())
        .ok_or_else(|| format!("{file} is not listed in {SUMS_NAME}"))?;
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got == want { Ok(()) } else { Err(format!("{file} is damaged (checksum mismatch); try again later")) }
}

/// The update bar's text: empty unless the check found a newer version.
pub fn bar_text(current: &str, result: &Result<Release, String>) -> String {
    match result {
        Ok(r) if is_newer(current, &r.tag) => {
            format!("Version {} is available", r.version().map_or(r.tag.clone(), |v| v.to_string()))
        }
        _ => String::new(),
    }
}

fn agent() -> ureq::Agent {
    #[cfg(windows)]
    {
        use ureq::tls::{TlsConfig, TlsProvider};
        ureq::Agent::config_builder()
            .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).build())
            .build()
            .into()
    }
    #[cfg(not(windows))]
    ureq::Agent::new_with_defaults()
}

fn get(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    let mut response = agent()
        .get(url)
        .header("User-Agent", concat!("MasterKeys/", env!("CARGO_PKG_VERSION")))
        .header("Accept", accept)
        .call()
        .map_err(|e| e.to_string())?;
    response.body_mut().with_config().limit(200 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())
}

/// Ask GitHub for the latest release.
pub fn fetch_latest() -> Result<Release, String> {
    let body = get(LATEST_URL, "application/vnd.github+json")?;
    Release::from_json(&String::from_utf8_lossy(&body))
}

/// Download `release`, check it, put its exe in place of this one and start it. The caller then quits.
#[cfg(windows)]
pub fn install(release: &Release) -> Result<(), String> {
    use std::io::Read;
    let (Some(zip_url), Some(sums_url)) = (&release.zip_url, &release.sums_url) else {
        return Err("this release has no Windows download".into());
    };
    let sums = String::from_utf8_lossy(&get(sums_url, "application/octet-stream")?).into_owned();
    let zip = get(zip_url, "application/octet-stream")?;
    check_sum(&zip, &sums, ZIP_NAME)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| format!("bad download: {e}"))?;
    let mut exe = Vec::new();
    archive
        .by_name("masterkeys.exe")
        .map_err(|e| format!("bad download: {e}"))?
        .read_to_end(&mut exe)
        .map_err(|e| format!("bad download: {e}"))?;
    let new = std::env::temp_dir().join(format!("masterkeys-{}.exe", release.tag));
    std::fs::write(&new, exe).map_err(|e| format!("cannot save the update: {e}"))?;
    self_replace::self_replace(&new).map_err(|e| format!("cannot replace MasterKeys: {e}"))?;
    let _ = std::fs::remove_file(&new);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(me).arg(REPLACING_ARG).spawn().map_err(|e| format!("updated, but cannot restart: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const RELEASE: &str = r#"{
        "tag_name": "v0.3.0",
        "html_url": "https://github.com/I-MnM-I/MasterKeys/releases/tag/v0.3.0",
        "assets": [
            {"name": "MasterKeys-windows.zip", "browser_download_url": "https://example.test/z.zip"},
            {"name": "SHA256SUMS", "browser_download_url": "https://example.test/SHA256SUMS"}
        ]
    }"#;

    #[test]
    fn newer_versions() {
        assert!(is_newer("0.2.0", "v0.3.0"));
        assert!(!is_newer("0.2.0", "v0.2.0"));
        assert!(!is_newer("0.2.0", "v0.1.9"));
        assert!(!is_newer("0.2.0", "nonsense"));
        assert!(is_newer("0.2.0", "0.10.0"));
    }

    #[test]
    fn parses_a_release() {
        let r = Release::from_json(RELEASE).unwrap();
        assert_eq!(r.tag, "v0.3.0");
        assert_eq!(r.page_url, "https://github.com/I-MnM-I/MasterKeys/releases/tag/v0.3.0");
        assert_eq!(r.zip_url.as_deref(), Some("https://example.test/z.zip"));
        assert_eq!(r.sums_url.as_deref(), Some("https://example.test/SHA256SUMS"));
        assert!(r.can_install());
    }

    #[test]
    fn release_without_zip_cannot_install() {
        let r = Release::from_json(r#"{"tag_name":"v1.0.0","html_url":"https://x.test","assets":[]}"#).unwrap();
        assert_eq!(r.zip_url, None);
        assert!(!r.can_install());
    }

    #[test]
    fn bad_hash_is_rejected() {
        let sums = format!("{}  {ZIP_NAME}\n", "0".repeat(64));
        assert!(check_sum(b"zip bytes", &sums, ZIP_NAME).is_err());
        assert!(check_sum(b"zip bytes", "", ZIP_NAME).is_err());
    }

    #[test]
    fn good_hash_is_accepted() {
        let hex: String = Sha256::digest(b"zip bytes").iter().map(|b| format!("{b:02x}")).collect();
        let sums = format!("{}  other.txt\n{hex}  {ZIP_NAME}\n", "1".repeat(64));
        assert_eq!(check_sum(b"zip bytes", &sums, ZIP_NAME), Ok(()));
        assert_eq!(check_sum(b"zip bytes", &sums.to_uppercase().replace("MASTERKEYS-WINDOWS.ZIP", ZIP_NAME), ZIP_NAME), Ok(()));
    }

    #[test]
    fn failed_check_shows_nothing() {
        assert_eq!(bar_text("0.2.0", &Err("offline".into())), "");
    }

    #[test]
    fn newer_release_shows_version() {
        let r = Release::from_json(RELEASE).unwrap();
        assert_eq!(bar_text("0.2.0", &Ok(r.clone())), "Version 0.3.0 is available");
        assert_eq!(bar_text("0.3.0", &Ok(r)), "");
    }
}
