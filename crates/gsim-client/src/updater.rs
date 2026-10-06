//! Self-update from the rolling "latest" GitHub release.
//!
//! Builds made by CI on `master` carry the commit they were built from. At start-up a
//! background thread compares that with `version.txt` in the release and, if they differ,
//! downloads the new client. The update is applied when the player is in the menu (then the
//! game restarts) or on exit. Local and branch builds carry no version and never update.

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Commit this binary was built from; set by CI for rolling-release builds only.
const BUILD_VERSION: Option<&str> = option_env!("GSIM_BUILD_VERSION");
const DEFAULT_BASE_URL: &str = "https://github.com/Petr1Furious/potato-gsim/releases/download/latest";
/// Set on the restarted process so a half-published release cannot cause an update loop.
const JUST_UPDATED_ENV: &str = "GSIM_JUST_UPDATED";

#[cfg(target_os = "windows")]
const ASSET: &str = "gsim-client-windows-x86_64.exe";
#[cfg(target_os = "linux")]
const ASSET: &str = "gsim-client-linux-x86_64";
#[cfg(target_os = "macos")]
const ASSET: &str = "gsim-client-macos-universal.tar.gz";

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Not a release build, or switched off.
    Disabled,
    Checking,
    UpToDate,
    Downloading { percent: u8 },
    /// Downloaded and waiting to be applied.
    Ready,
    Failed(String),
}

pub struct Updater {
    status: Arc<Mutex<Status>>,
    payload: Arc<Mutex<Option<Vec<u8>>>>,
    /// The version to compare against, if this build updates itself at all.
    local: Option<&'static str>,
}

pub fn build_version() -> &'static str {
    BUILD_VERSION.filter(|v| !v.is_empty()).unwrap_or("dev")
}

fn base_url() -> String {
    // Overridable for testing against a local file server.
    std::env::var("GSIM_UPDATE_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn fetch(url: &str, mut progress: impl FnMut(usize, usize)) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(5)).timeout(Duration::from_secs(120)).build();
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("server answered {code}"),
        other => other.to_string(),
    })?;
    let total: usize = response.header("Content-Length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let mut reader = response.into_reader().take(256 * 1024 * 1024);
    let mut bytes = Vec::with_capacity(total);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        progress(bytes.len(), total);
    }
    Ok(bytes)
}

impl Updater {
    pub fn start(enabled: bool) -> Self {
        let just_updated = std::env::var_os(JUST_UPDATED_ENV).is_some();
        let version = BUILD_VERSION.filter(|v| !v.is_empty());
        let me = Self { status: Arc::new(Mutex::new(Status::Disabled)), payload: Arc::new(Mutex::new(None)), local: version.filter(|_| enabled) };
        if just_updated {
            if me.local.is_some() {
                *me.status.lock().unwrap() = Status::UpToDate;
            }
            return me;
        }
        me.check();
        me
    }

    /// Whether asking again makes sense: this build updates itself and is not busy doing so.
    pub fn can_check(&self) -> bool {
        self.local.is_some() && matches!(self.status(), Status::UpToDate | Status::Failed(_))
    }

    /// Look for a newer release now, and download it if there is one.
    pub fn check(&self) {
        let Some(local) = self.local else { return };
        if matches!(self.status(), Status::Checking | Status::Downloading { .. } | Status::Ready) {
            return;
        }
        *self.status.lock().unwrap() = Status::Checking;
        let (status, payload) = (self.status.clone(), self.payload.clone());
        let set = move |s: Status| *status.lock().unwrap() = s;
        std::thread::Builder::new()
            .name("gsim-updater".into())
            .spawn(move || {
                let base = base_url();
                let remote = match fetch(&format!("{base}/version.txt"), |_, _| {}) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_string(),
                    Err(e) => return set(Status::Failed(format!("update check failed: {e}"))),
                };
                if remote.is_empty() || remote == local {
                    return set(Status::UpToDate);
                }
                set(Status::Downloading { percent: 0 });
                let progress = |done: usize, total: usize| {
                    if total > 0 {
                        set(Status::Downloading { percent: (done * 100 / total).min(100) as u8 });
                    }
                };
                match fetch(&format!("{base}/{ASSET}"), progress) {
                    Ok(bytes) if bytes.len() > 100_000 => {
                        *payload.lock().unwrap() = Some(bytes);
                        set(Status::Ready);
                    }
                    Ok(_) => set(Status::Failed("update download was truncated".into())),
                    Err(e) => set(Status::Failed(format!("update download failed: {e}"))),
                }
            })
            .ok();
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Install a downloaded update, if there is one. With `restart` the new version is
    /// launched with the same arguments and this process exits.
    pub fn apply(&self, restart: bool) {
        let Some(bytes) = self.payload.lock().unwrap().take() else { return };
        match install(&bytes) {
            Ok(exe) => {
                *self.status.lock().unwrap() = Status::UpToDate;
                if restart {
                    let args: Vec<String> = std::env::args().skip(1).collect();
                    if Command::new(exe).args(args).env(JUST_UPDATED_ENV, "1").spawn().is_ok() {
                        std::process::exit(0);
                    }
                }
            }
            Err(e) => *self.status.lock().unwrap() = Status::Failed(format!("could not install update: {e}")),
        }
    }
}

/// Replace the running executable. Returns the path to launch afterwards.
#[cfg(not(target_os = "macos"))]
fn install(new_binary: &[u8]) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let temp = std::env::temp_dir().join(format!("gsim-client-update-{}", std::process::id()));
    std::fs::write(&temp, new_binary).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let result = self_replace::self_replace(&temp).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&temp);
    result.map(|_| exe)
}

/// Replace the whole `.app` bundle with the one in the downloaded archive (`update.app`).
#[cfg(target_os = "macos")]
fn install(archive: &[u8]) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = exe
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .filter(|b| b.extension().is_some_and(|e| e == "app"))
        .ok_or("not running from an .app bundle")?
        .to_path_buf();
    let tag = std::process::id();
    let extract = std::env::temp_dir().join(format!("gsim-update-{tag}"));
    let backup = std::env::temp_dir().join(format!("gsim-update-backup-{tag}"));
    let _ = std::fs::remove_dir_all(&extract);
    std::fs::create_dir_all(&extract).map_err(|e| e.to_string())?;
    tar::Archive::new(flate2::read::GzDecoder::new(archive)).unpack(&extract).map_err(|e| e.to_string())?;
    let fresh = extract.join("update.app");
    if !fresh.join("Contents/MacOS").is_dir() {
        return Err("update archive has no app bundle".into());
    }
    // Swap, and put the old bundle back if the new one cannot be moved into place.
    std::fs::rename(&bundle, &backup).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&fresh, &bundle) {
        let _ = std::fs::rename(&backup, &bundle);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&extract);
    Ok(exe)
}
