//! Managed external tools. Today the only one is **aria2c**, resolved through the
//! same chain every managed tool follows and, on Windows, fetchable from within
//! the app. Nothing here is bundled: moin either finds a binary you already have
//! or downloads a current build into its own data dir.
//!
//! Resolve order (first hit wins):
//! explicit path (settings) → `MOIN_ARIA2` env → app-managed `<data>/bin` →
//! beside moin's exe → `PATH`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Bare binary name, per platform.
#[cfg(windows)]
const ARIA2_BIN: &str = "aria2c.exe";
#[cfg(not(windows))]
const ARIA2_BIN: &str = "aria2c";

/// Environment override checked before the managed copy.
const ARIA2_ENV: &str = "MOIN_ARIA2";

/// The official Windows build moin fetches on demand. Verified against the pinned
/// checksum before it's trusted; the archive nests the binary under a versioned
/// folder.
const ARIA2_URL: &str =
    "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip";
const ARIA2_SHA256: &str = "67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288";
const ARIA2_ZIP_ENTRY: &str = "aria2-1.37.0-win-64bit-build1/aria2c.exe";

/// Which link in the resolve chain provided the binary — shown in the UI so it's
/// clear which aria2c is in effect.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSource {
    /// The explicit path set in settings ("bring your own").
    Override,
    /// The `MOIN_ARIA2` environment variable.
    Env,
    /// The app-managed copy in `<data>/bin`.
    Managed,
    /// A binary sitting beside moin's own executable.
    Beside,
    /// Found on `PATH`.
    Path,
}

/// A tool's availability snapshot for the settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    /// Stable id, e.g. `"aria2"`.
    pub id: String,
    /// Whether a usable binary was found.
    pub present: bool,
    /// The resolved binary path, if any.
    pub path: Option<String>,
    /// Version parsed from `--version`, when the probe succeeded.
    pub version: Option<String>,
    /// Which link in the resolve chain provided it.
    pub source: Option<ToolSource>,
    /// Whether this platform can fetch the binary in-app (Windows only).
    pub can_fetch: bool,
}

/// Resolves (and, on Windows, installs) the aria2c binary. Shared between the
/// aria2 backend and the tool commands, so a fresh download or a new
/// bring-your-own path is picked up everywhere at once.
pub struct Aria2Tool {
    data_dir: PathBuf,
    /// The bring-your-own override path from settings.
    override_path: Mutex<Option<String>>,
}

impl Aria2Tool {
    pub fn new(data_dir: PathBuf, override_path: Option<String>) -> Self {
        Self {
            data_dir,
            override_path: Mutex::new(override_path),
        }
    }

    /// Update the bring-your-own path (cleared with `None`).
    pub fn set_override(&self, path: Option<String>) {
        *self.override_path.lock().unwrap() = path.filter(|p| !p.trim().is_empty());
    }

    /// The managed-copy destination, `<data>/bin/aria2c[.exe]`.
    fn managed_path(&self) -> PathBuf {
        self.data_dir.join("bin").join(ARIA2_BIN)
    }

    /// Walk the resolve chain and return the first binary that exists, with the
    /// link that provided it. No process is spawned here — see [`status`] for the
    /// version probe.
    pub fn resolve(&self) -> Option<(PathBuf, ToolSource)> {
        if let Some(p) = self.override_path.lock().unwrap().clone() {
            let path = PathBuf::from(p);
            if path.is_file() {
                return Some((path, ToolSource::Override));
            }
        }
        if let Some(p) = std::env::var_os(ARIA2_ENV) {
            let path = PathBuf::from(p);
            if path.is_file() {
                return Some((path, ToolSource::Env));
            }
        }
        let managed = self.managed_path();
        if managed.is_file() {
            return Some((managed, ToolSource::Managed));
        }
        if let Some(beside) = beside_exe() {
            if beside.is_file() {
                return Some((beside, ToolSource::Beside));
            }
        }
        if let Some(on_path) = on_path() {
            return Some((on_path, ToolSource::Path));
        }
        None
    }

    /// Just the resolved binary path — what the backend needs to spawn the daemon.
    pub fn binary(&self) -> Option<PathBuf> {
        self.resolve().map(|(p, _)| p)
    }

    /// Cheap presence check for [`DownloadBackend::available`]: does a binary
    /// resolve? Deliberately doesn't run the process.
    pub fn is_available(&self) -> bool {
        self.resolve().is_some()
    }

    /// Full status, including a `--version` probe for display.
    pub async fn status(&self) -> ToolStatus {
        let resolved = self.resolve();
        let version = match &resolved {
            Some((path, _)) => probe_version(path).await,
            None => None,
        };
        ToolStatus {
            id: "aria2".to_string(),
            present: resolved.is_some(),
            path: resolved
                .as_ref()
                .map(|(p, _)| p.to_string_lossy().into_owned()),
            source: resolved.as_ref().map(|(_, s)| *s),
            version,
            can_fetch: cfg!(windows),
        }
    }

    /// Fetch the official Windows build, verify it, and install it into
    /// `<data>/bin`. `progress` is called with (received, total) while the archive
    /// downloads. Returns the refreshed status. Windows only.
    pub async fn install(
        &self,
        progress: impl Fn(u64, Option<u64>) + Send + Sync,
    ) -> Result<ToolStatus, String> {
        if !cfg!(windows) {
            return Err(
                "In-app download is Windows-only. On this platform, install aria2c \
                 (your package manager has it) and point moin at it with \"Use my binary\"."
                    .to_string(),
            );
        }

        let zip_bytes = download_verified(ARIA2_URL, ARIA2_SHA256, &progress).await?;

        let managed = self.managed_path();
        let bin_dir = managed
            .parent()
            .ok_or_else(|| "bad managed path".to_string())?
            .to_path_buf();
        let dest = managed.clone();

        // Zip extraction is blocking (crc + inflate), so keep it off the async
        // runtime's worker threads.
        tokio::task::spawn_blocking(move || {
            extract_entry(&zip_bytes, ARIA2_ZIP_ENTRY, &bin_dir, &dest)
        })
        .await
        .map_err(|e| format!("extract task failed: {e}"))??;

        Ok(self.status().await)
    }
}

/// aria2c living next to moin's own executable, if we can find our exe.
fn beside_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(ARIA2_BIN))
}

/// First `ARIA2_BIN` found on `PATH`.
fn on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(ARIA2_BIN))
        .find(|candidate| candidate.is_file())
}

/// Run `<bin> --version` and pull the version token out of the first line, which
/// aria2 prints as `aria2 version 1.37.0`.
async fn probe_version(bin: &Path) -> Option<String> {
    let output = new_command(bin).arg("--version").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?;
    first
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)
        .map(str::to_string)
        .or_else(|| Some(first.trim().to_string()))
}

/// A tokio command that never flashes a console window on Windows.
pub fn new_command(bin: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(bin);
    #[cfg(windows)]
    {
        // tokio's Command exposes `creation_flags` inherently on Windows.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Stream a URL into memory, hashing as we go, and reject it unless the SHA-256
/// matches. A pin of `__ARIA2_SHA256__` (unset) skips the check — never shipped.
async fn download_verified(
    url: &str,
    expected_sha256: &str,
    progress: &(impl Fn(u64, Option<u64>) + Send + Sync),
) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;

    let client = reqwest::Client::builder()
        .user_agent(concat!("moin/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("couldn't reach the download: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "download failed: server returned {}",
            resp.status()
        ));
    }
    let total = resp.content_length();

    let mut hasher = Sha256::new();
    let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("download interrupted: {e}"))?;
        hasher.update(&chunk);
        buf.extend_from_slice(&chunk);
        progress(buf.len() as u64, total);
    }

    let got = hex_lower(&hasher.finalize());
    let pinned = expected_sha256.trim().to_lowercase();
    if pinned != "__aria2_sha256__" && got != pinned {
        return Err(format!(
            "checksum mismatch — refusing to install (expected {pinned}, got {got})"
        ));
    }
    Ok(buf)
}

/// Extract one entry from an in-memory zip to `dest`, creating `bin_dir` first.
fn extract_entry(zip_bytes: &[u8], entry: &str, bin_dir: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(bin_dir).map_err(|e| format!("couldn't create {bin_dir:?}: {e}"))?;
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("bad archive: {e}"))?;
    let mut file = archive
        .by_name(entry)
        .map_err(|_| format!("archive is missing {entry}"))?;
    let mut out =
        std::fs::File::create(dest).map_err(|e| format!("couldn't write {dest:?}: {e}"))?;
    std::io::copy(&mut file, &mut out).map_err(|e| format!("extract failed: {e}"))?;
    Ok(())
}

/// Lowercase hex of a byte slice.
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
