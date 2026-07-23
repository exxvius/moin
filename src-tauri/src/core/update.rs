//! Update check against the project's GitHub releases. moin ships tiny and fetches
//! things on demand, so the updater is deliberately light: it asks GitHub for the
//! latest release, compares it to the running version, and hands back what it found
//! (version, notes, the installer asset for this platform, and the release page).
//! Downloading + installing is left to the shell/opener — a running executable can't
//! overwrite itself in place, so the actual swap is the installer's job.

use serde::Serialize;

/// The public repo releases are published to.
const REPO: &str = "exxvius/moin";

/// What the settings About/Update card needs to show — the running version, the
/// latest published one, whether it's newer, the notes, and where to get it.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub notes: Option<String>,
    /// Direct download for this platform's installer, when one is attached.
    pub asset_url: Option<String>,
    /// The release page — the fallback "get it here" link.
    pub page_url: String,
}

/// Ask GitHub for the latest release and compare it to `current` (a bare SemVer,
/// no leading `v`). Network + parse failures surface as an error the UI shows.
pub async fn check(current: &str) -> Result<UpdateInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent(concat!("moin/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub returned {}", resp.status()));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("couldn't read GitHub's reply: {e}"))?;

    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
    let latest = tag.trim_start_matches('v').trim().to_string();
    let notes = v
        .get("body")
        .and_then(|b| b.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let page_url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases"));
    let asset_url = pick_asset(&v);
    let available = is_newer(&latest, current);

    Ok(UpdateInfo {
        current: current.to_string(),
        latest: (!latest.is_empty()).then_some(latest),
        available,
        notes,
        asset_url,
        page_url,
    })
}

/// Pick the release asset that installs moin on the running platform: the Windows
/// installer (`.msi` / NSIS `-setup.exe`), the macOS `.dmg`, or a Linux
/// `.AppImage` / `.deb`. `None` when the release has no matching asset.
fn pick_asset(release: &serde_json::Value) -> Option<String> {
    let assets = release.get("assets")?.as_array()?;
    let matches = |name: &str| -> bool {
        let n = name.to_ascii_lowercase();
        if cfg!(target_os = "windows") {
            n.ends_with(".msi") || (n.contains("setup") && n.ends_with(".exe"))
        } else if cfg!(target_os = "macos") {
            n.ends_with(".dmg")
        } else {
            n.ends_with(".appimage") || n.ends_with(".deb")
        }
    };
    assets
        .iter()
        .find_map(|a| {
            let name = a.get("name").and_then(|n| n.as_str())?;
            matches(name)
                .then(|| a.get("browser_download_url").and_then(|u| u.as_str()))
                .flatten()
        })
        .map(str::to_string)
}

/// Whether `latest` is a newer SemVer than `current`. Missing/short parts count as
/// 0, and a non-numeric part stops the comparison (treated as not newer) so a
/// pre-release tag can't masquerade as an upgrade.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split(['.', '-', '+'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    if latest.is_empty() {
        return false;
    }
    let (l, c) = (parse(latest), parse(current));
    for i in 0..l.len().max(c.len()) {
        let (a, b) = (
            l.get(i).copied().unwrap_or(0),
            c.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn newer_when_a_part_increases() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn not_newer_when_equal_or_older() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("", "0.1.0"));
    }
}
