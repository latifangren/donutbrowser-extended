use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProxySettings {
  pub proxy_type: String,
  pub host: String,
  pub port: u16,
  pub username: Option<String>,
  pub password: Option<String>,
  /// The share link of a `vless`, `vmess`, `trojan` or `hysteria2` proxy. It
  /// holds the whole server configuration; the name predates the other three.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub vless_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BrowserType {
  Wayfern,
  Cloak,
}

impl BrowserType {
  pub fn as_str(&self) -> &'static str {
    match self {
      BrowserType::Wayfern => "wayfern",
      BrowserType::Cloak => "cloak",
    }
  }

  /// Brand name for user-facing strings. `as_str` is the internal id and is
  /// the wrong thing to put in a message the user reads.
  pub fn display_name(&self) -> &'static str {
    match self {
      BrowserType::Wayfern => "Wayfern",
      BrowserType::Cloak => "CloakBrowser",
    }
  }

  pub fn from_str(s: &str) -> Result<Self, String> {
    match s {
      "wayfern" => Ok(BrowserType::Wayfern),
      "cloak" => Ok(BrowserType::Cloak),
      _ => Err(format!("Unknown browser type: {s}")),
    }
  }
}

#[allow(dead_code)]
pub trait Browser: Send + Sync {
  fn get_executable_path(&self, install_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>>;
  fn create_launch_args(
    &self,
    profile_path: &str,
    proxy_settings: Option<&ProxySettings>,
    url: Option<String>,
    remote_debugging_port: Option<u16>,
    headless: bool,
  ) -> Result<Vec<String>, Box<dyn std::error::Error>>;
  fn is_version_downloaded(&self, version: &str, binaries_dir: &Path) -> bool;
  fn prepare_executable(&self, executable_path: &Path) -> Result<(), Box<dyn std::error::Error>>;
}

/// First file named `file_name` under `dir`, depth-first. Used by engines
/// whose archives unpack the browser binary at an unpredictable depth.
fn find_file_named(dir: &Path, file_name: &str) -> Option<PathBuf> {
  let mut found = Vec::new();
  find_all_named(dir, file_name, &mut found);
  found.into_iter().next()
}

/// Every file named `file_name` under `dir`, depth-first.
fn find_all_named(dir: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
  let Ok(entries) = std::fs::read_dir(dir) else {
    return;
  };
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      find_all_named(&path, file_name, out);
    } else if entry.file_name().to_string_lossy() == file_name {
      out.push(path);
    }
  }
}

// Platform-specific modules
#[cfg(target_os = "macos")]
mod macos {
  use super::*;

  pub fn get_wayfern_executable_path(
    install_dir: &Path,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Newer builds ship as Wayfern.app; older ones as Chromium.app. Either way,
    // find the .app bundle in the version directory.
    let app_path = std::fs::read_dir(install_dir)?
      .filter_map(Result::ok)
      .find(|entry| entry.path().extension().is_some_and(|ext| ext == "app"))
      .ok_or("Wayfern app not found")?;

    // Construct the browser executable path
    let mut executable_dir = app_path.path();
    executable_dir.push("Contents");
    executable_dir.push("MacOS");

    // Find the main executable inside Contents/MacOS. The renamed builds name it
    // `Wayfern`; older Chromium-named builds name it `Chromium`. Helper binaries
    // such as `chrome_crashpad_handler` contain neither token and are skipped.
    let executable_path = std::fs::read_dir(&executable_dir)?
      .filter_map(Result::ok)
      .find(|entry| {
        let binding = entry.file_name();
        let name = binding.to_string_lossy();
        name.contains("Wayfern") || name.contains("Chromium")
      })
      .map(|entry| entry.path())
      .ok_or("No Wayfern executable found in MacOS directory")?;

    Ok(executable_path)
  }

  pub fn is_wayfern_version_downloaded(install_dir: &Path) -> bool {
    // On macOS, check for the .app bundle (Wayfern.app or legacy Chromium.app)
    if let Ok(entries) = std::fs::read_dir(install_dir) {
      for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|ext| ext == "app") {
          return true;
        }
      }
    }
    false
  }

  pub fn get_cloak_executable_path(
    install_dir: &Path,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // CloakBrowser archives unpack either straight into the version dir or
    // into a single nested folder; the bundle name varies by release
    // (Chromium.app, CloakBrowser.app, ...).
    let mut search_roots = vec![install_dir.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(install_dir) {
      search_roots.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for root in search_roots {
      let Ok(entries) = std::fs::read_dir(&root) else {
        continue;
      };
      for entry in entries.flatten() {
        let bundle = entry.path();
        if !bundle.is_dir() || bundle.extension().is_none_or(|ext| ext != "app") {
          continue;
        }
        let macos_dir = bundle.join("Contents").join("MacOS");
        let stem = bundle.file_stem().unwrap_or_default().to_string_lossy();
        for name in ["Chromium", stem.as_ref()] {
          let candidate = macos_dir.join(name);
          if candidate.is_file() {
            return Ok(candidate);
          }
        }
      }
    }
    Err("CloakBrowser app bundle not found".into())
  }

  pub fn is_cloak_version_downloaded(install_dir: &Path) -> bool {
    get_cloak_executable_path(install_dir).is_ok()
  }

  #[allow(dead_code)]
  pub fn prepare_executable(_executable_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // On macOS, no special preparation needed
    Ok(())
  }
}

#[cfg(target_os = "linux")]
mod linux {
  use super::*;
  use std::os::unix::fs::PermissionsExt;

  /// Candidate paths for the Wayfern executable on Linux, in priority order.
  /// Newer builds ship the binary named `wayfern`; the `chromium`/`chrome` names
  /// are retained as fallbacks so versions extracted before the rename still
  /// launch. Each name is probed at the version root and in the subdirectory
  /// layouts the archive may unpack into.
  fn wayfern_executable_candidates(install_dir: &Path) -> Vec<PathBuf> {
    const NAMES: [&str; 3] = ["wayfern", "chromium", "chrome"];
    let dirs = [
      install_dir.to_path_buf(),
      install_dir.join("wayfern"),
      install_dir.join("wayfern-linux"),
      install_dir.join("chrome-linux"),
    ];
    dirs
      .iter()
      .flat_map(|dir| NAMES.iter().map(move |name| dir.join(name)))
      .collect()
  }

  pub fn get_wayfern_executable_path(
    install_dir: &Path,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    for executable_path in wayfern_executable_candidates(install_dir) {
      if executable_path.exists() && executable_path.is_file() {
        return Ok(executable_path);
      }
    }

    Err(format!("Wayfern executable not found in {}", install_dir.display()).into())
  }

  pub fn is_wayfern_version_downloaded(install_dir: &Path) -> bool {
    wayfern_executable_candidates(install_dir)
      .iter()
      .any(|exe_path| exe_path.exists() && exe_path.is_file())
  }

  pub fn get_cloak_executable_path(
    install_dir: &Path,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // The free CloakBrowser archives unpack a stock-layout Chromium tree with
    // the binary named `chrome`, possibly nested one level deep.
    super::find_file_named(install_dir, "chrome")
      .filter(|p| p.is_file())
      .ok_or_else(|| {
        format!(
          "CloakBrowser executable not found in {}",
          install_dir.display()
        )
        .into()
      })
  }

  pub fn is_cloak_version_downloaded(install_dir: &Path) -> bool {
    get_cloak_executable_path(install_dir).is_ok()
  }

  #[allow(dead_code)]
  pub fn prepare_executable(executable_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // On Linux, ensure the executable has proper permissions

    let metadata = std::fs::metadata(executable_path)?;
    let mut permissions = metadata.permissions();

    // Add execute permissions for owner, group, and others
    let mode = permissions.mode();
    permissions.set_mode(mode | 0o755);

    std::fs::set_permissions(executable_path, permissions)?;

    log::debug!("Execute permission set path={}", executable_path.display());
    Ok(())
  }
}

#[cfg(target_os = "windows")]
mod windows {
  use super::*;

  /// Candidate paths for the Wayfern executable on Windows, in priority order.
  /// Newer builds ship `wayfern.exe`; the `chromium.exe`/`chrome.exe` names are
  /// retained as fallbacks so versions extracted before the rename still launch.
  /// Each name is probed at the version root and in the subdirectory layouts the
  /// archive may unpack into.
  fn wayfern_executable_candidates(install_dir: &Path) -> Vec<PathBuf> {
    const NAMES: [&str; 3] = ["wayfern.exe", "chromium.exe", "chrome.exe"];
    let dirs = [
      install_dir.to_path_buf(),
      install_dir.join("bin"),
      install_dir.join("wayfern"),
      install_dir.join("wayfern-win"),
      install_dir.join("chrome-win"),
    ];
    dirs
      .iter()
      .flat_map(|dir| NAMES.iter().map(move |name| dir.join(name)))
      .collect()
  }

  /// Whether `path` is an .exe whose name looks like the browser (Wayfern or a
  /// legacy Chromium-named build). Guards against archives wrongly given a
  /// `*.exe` name by requiring a valid PE header.
  fn is_wayfern_exe(path: &Path) -> bool {
    if path.extension().is_none_or(|ext| ext != "exe") || !is_pe_executable(path) {
      return false;
    }
    let name = path
      .file_stem()
      .unwrap_or_default()
      .to_string_lossy()
      .to_lowercase();
    name.contains("wayfern") || name.contains("chromium") || name.contains("chrome")
  }

  pub fn get_wayfern_executable_path(
    install_dir: &Path,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    for path in wayfern_executable_candidates(install_dir) {
      if path.exists() && path.is_file() {
        return Ok(path);
      }
    }

    // Look for any .exe file that might be the browser
    if let Ok(entries) = std::fs::read_dir(install_dir) {
      for entry in entries.flatten() {
        let path = entry.path();
        if is_wayfern_exe(&path) {
          return Ok(path);
        }
      }
    }

    Err("Wayfern executable not found in Windows installation directory".into())
  }

  pub fn is_wayfern_version_downloaded(install_dir: &Path) -> bool {
    if wayfern_executable_candidates(install_dir)
      .iter()
      .any(|exe_path| exe_path.exists() && exe_path.is_file() && has_sibling_dll(exe_path))
    {
      return true;
    }

    // Check for any .exe file that looks like the browser
    if let Ok(entries) = std::fs::read_dir(install_dir) {
      for entry in entries.flatten() {
        let path = entry.path();
        if is_wayfern_exe(&path) && has_sibling_dll(&path) {
          return true;
        }
      }
    }

    false
  }

  pub fn get_cloak_executable_path(
    install_dir: &Path,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // The free CloakBrowser archives unpack a stock-layout Chromium tree with
    // `chrome.exe` at some depth. A lone .exe without its sibling libraries is
    // a gutted install (fails in the Windows loader), so it is rejected.
    let mut found = Vec::new();
    super::find_all_named(install_dir, "chrome.exe", &mut found);
    found
      .into_iter()
      .find(|p| p.is_file() && super::has_sibling_dll(p))
      .ok_or_else(|| {
        format!(
          "CloakBrowser executable not found in {}",
          install_dir.display()
        )
        .into()
      })
  }

  pub fn is_cloak_version_downloaded(install_dir: &Path) -> bool {
    get_cloak_executable_path(install_dir).is_ok()
  }

  #[allow(dead_code)]
  pub fn prepare_executable(_executable_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // On Windows, no special preparation needed
    Ok(())
  }
}

/// Wayfern is a Chromium-based anti-detect browser with CDP-based fingerprint injection
pub struct WayfernBrowser;

impl WayfernBrowser {
  pub fn new() -> Self {
    Self
  }
}

impl Browser for WayfernBrowser {
  fn get_executable_path(&self, install_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    return macos::get_wayfern_executable_path(install_dir);

    #[cfg(target_os = "linux")]
    return linux::get_wayfern_executable_path(install_dir);

    #[cfg(target_os = "windows")]
    return windows::get_wayfern_executable_path(install_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err("Unsupported platform".into())
  }

  fn create_launch_args(
    &self,
    profile_path: &str,
    proxy_settings: Option<&ProxySettings>,
    url: Option<String>,
    remote_debugging_port: Option<u16>,
    headless: bool,
  ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Wayfern uses Chromium-style arguments
    let mut args = vec![
      format!("--user-data-dir={}", profile_path),
      "--no-default-browser-check".to_string(),
      "--disable-background-mode".to_string(),
      "--disable-component-update".to_string(),
      "--crash-server-url=".to_string(),
      "--disable-updater".to_string(),
      "--disable-session-crashed-bubble".to_string(),
      "--hide-crash-restore-bubble".to_string(),
      "--disable-infobars".to_string(),
      // Wayfern-specific args for automation
      "--disable-features=DialMediaRouteProvider".to_string(),
      "--use-mock-keychain".to_string(),
      "--password-store=basic".to_string(),
    ];

    // Add remote debugging port (required for CDP fingerprint injection)
    if let Some(port) = remote_debugging_port {
      args.push("--remote-debugging-address=127.0.0.1".to_string());
      args.push(format!("--remote-debugging-port={port}"));
    }

    // Add headless mode if requested
    if headless {
      args.push("--headless=new".to_string());
    }

    // Add proxy configuration if provided
    if let Some(proxy) = proxy_settings {
      args.push(format!(
        "--proxy-server=http://{}:{}",
        proxy.host, proxy.port
      ));
    }

    if let Some(url) = url {
      args.push(url);
    }

    Ok(args)
  }

  fn is_version_downloaded(&self, version: &str, binaries_dir: &Path) -> bool {
    let install_dir = binaries_dir.join("wayfern").join(version);

    #[cfg(target_os = "macos")]
    return macos::is_wayfern_version_downloaded(&install_dir);

    #[cfg(target_os = "linux")]
    return linux::is_wayfern_version_downloaded(&install_dir);

    #[cfg(target_os = "windows")]
    return windows::is_wayfern_version_downloaded(&install_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    false
  }

  fn prepare_executable(&self, executable_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    return macos::prepare_executable(executable_path);

    #[cfg(target_os = "linux")]
    return linux::prepare_executable(executable_path);

    #[cfg(target_os = "windows")]
    return windows::prepare_executable(executable_path);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err("Unsupported platform".into())
  }
}

/// GitHub API listing CloakBrowser releases, newest first.
pub const CLOAKBROWSER_RELEASES_API: &str =
  "https://api.github.com/repos/CloakHQ/CloakBrowser/releases?per_page=30";

/// This platform's CloakBrowser archive name, or `None` where no free build
/// ships. Only the keyless (non-`-pro`) builds are ever downloaded.
pub fn cloak_asset_name() -> Option<&'static str> {
  match () {
    _ if cfg!(all(target_os = "macos", target_arch = "aarch64")) => {
      Some("cloakbrowser-darwin-arm64.tar.gz")
    }
    _ if cfg!(all(target_os = "macos", target_arch = "x86_64")) => {
      Some("cloakbrowser-darwin-x64.tar.gz")
    }
    _ if cfg!(all(target_os = "windows", target_arch = "x86_64")) => {
      Some("cloakbrowser-windows-x64.zip")
    }
    _ if cfg!(all(target_os = "linux", target_arch = "x86_64")) => {
      Some("cloakbrowser-linux-x64.tar.gz")
    }
    _ if cfg!(all(target_os = "linux", target_arch = "aarch64")) => {
      Some("cloakbrowser-linux-arm64.tar.gz")
    }
    _ => None,
  }
}

/// Version carried by a free (keyless) CloakBrowser release tag.
///
/// Tags look like `chromium-v146.0.7680.177.5`. `-pro` tags publish no
/// downloadable binary and require a CloakBrowser license key, so they are
/// never selected: this keeps the engine free without any account.
pub fn cloak_free_version_from_tag(tag: &str) -> Option<String> {
  if tag.ends_with("-pro") {
    return None;
  }
  tag.strip_prefix("chromium-v").map(|v| v.to_string())
}

/// Whether `version` only exists as a key-gated `-pro` build.
pub fn cloak_version_is_pro_only(releases: &[GithubRelease], version: &str) -> bool {
  let pro_tag = format!("chromium-v{version}-pro");
  releases
    .iter()
    .any(|r| r.tag_name == pro_tag && cloak_free_version_from_tag(&r.tag_name).is_none())
}

/// Newest free release carrying this platform's archive, or the pinned
/// `version` when given. Releases arrive newest-first from the GitHub API.
pub fn select_cloak_release(
  releases: &[GithubRelease],
  version: Option<&str>,
  asset_name: &str,
) -> Option<(String, String)> {
  releases.iter().find_map(|release| {
    let v = cloak_free_version_from_tag(&release.tag_name)?;
    if version.is_some_and(|want| v != want) {
      return None;
    }
    let asset = release.assets.iter().find(|a| a.name == asset_name)?;
    Some((v, asset.browser_download_url.clone()))
  })
}

/// CloakBrowser: open-core Chromium with C++-level fingerprint patching.
///
/// Only the free, keyless builds are ever used; the `-pro` builds (license
/// key required) are excluded by the release selection above, so no
/// CloakBrowser account is needed. Per-profile fingerprint flags arrive in a
/// later milestone; the free build already patches fingerprints natively.
pub struct CloakBrowser;

impl CloakBrowser {
  pub fn new() -> Self {
    Self
  }
}

impl Browser for CloakBrowser {
  fn get_executable_path(&self, install_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    return macos::get_cloak_executable_path(install_dir);

    #[cfg(target_os = "linux")]
    return linux::get_cloak_executable_path(install_dir);

    #[cfg(target_os = "windows")]
    return windows::get_cloak_executable_path(install_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err("Unsupported platform".into())
  }

  fn create_launch_args(
    &self,
    profile_path: &str,
    proxy_settings: Option<&ProxySettings>,
    url: Option<String>,
    remote_debugging_port: Option<u16>,
    headless: bool,
  ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Stock Chromium args. No fingerprint flags: the free CloakBrowser build
    // patches fingerprints at the C++ level, and per-profile flags are a
    // later milestone.
    let mut args = vec![
      format!("--user-data-dir={}", profile_path),
      "--no-default-browser-check".to_string(),
      "--disable-background-mode".to_string(),
      "--disable-component-update".to_string(),
      "--disable-features=DialMediaRouteProvider".to_string(),
      "--use-mock-keychain".to_string(),
      "--password-store=basic".to_string(),
    ];

    if let Some(port) = remote_debugging_port {
      args.push("--remote-debugging-address=127.0.0.1".to_string());
      args.push(format!("--remote-debugging-port={port}"));
    }

    if headless {
      args.push("--headless=new".to_string());
    }

    if let Some(proxy) = proxy_settings {
      // Chromium's --proxy-server takes a scheme; auth is negotiated
      // interactively and is out of scope for a headless launch arg.
      let scheme = match proxy.proxy_type.to_ascii_lowercase().as_str() {
        "socks5" => "socks5",
        "socks4" => "socks4",
        _ => "http",
      };
      args.push(format!(
        "--proxy-server={scheme}://{}:{}",
        proxy.host, proxy.port
      ));
    }

    if let Some(url) = url {
      args.push(url);
    }

    Ok(args)
  }

  fn is_version_downloaded(&self, version: &str, binaries_dir: &Path) -> bool {
    let install_dir = binaries_dir.join("cloak").join(version);

    #[cfg(target_os = "macos")]
    return macos::is_cloak_version_downloaded(&install_dir);

    #[cfg(target_os = "linux")]
    return linux::is_cloak_version_downloaded(&install_dir);

    #[cfg(target_os = "windows")]
    return windows::is_cloak_version_downloaded(&install_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    false
  }

  fn prepare_executable(&self, executable_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    return macos::prepare_executable(executable_path);

    #[cfg(target_os = "linux")]
    return linux::prepare_executable(executable_path);

    #[cfg(target_os = "windows")]
    return windows::prepare_executable(executable_path);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err("Unsupported platform".into())
  }
}

pub struct BrowserFactory;

impl BrowserFactory {
  fn new() -> Self {
    Self
  }

  pub fn instance() -> &'static BrowserFactory {
    &BROWSER_FACTORY
  }

  pub fn create_browser(&self, browser_type: BrowserType) -> Box<dyn Browser> {
    match browser_type {
      BrowserType::Wayfern => Box::new(WayfernBrowser::new()),
      BrowserType::Cloak => Box::new(CloakBrowser::new()),
    }
  }
}

/// Whether the directory holding `exe_path` also contains at least one `.dll`.
///
/// A Chromium build on Windows cannot start without its sibling libraries
/// (`chrome.dll` and friends) and its `.manifest`; a lone `.exe` is a gutted
/// install, and launching it fails inside the Windows loader with os error
/// 14001 (`ERROR_SXS_CANT_GEN_ACTCTX`, "side-by-side configuration is
/// incorrect"). Treating such a directory as downloaded is what made that state
/// permanent: the registry rescan re-added it as a healthy install, so no
/// re-download was ever offered. The check is scoped to the executable's own
/// directory because the payload may sit at the version root or in a `bin/`,
/// `wayfern/`, `wayfern-win/` or `chrome-win/` subdirectory.
#[cfg(any(target_os = "windows", test))]
fn has_sibling_dll(exe_path: &Path) -> bool {
  let Some(dir) = exe_path.parent() else {
    return false;
  };
  let Ok(entries) = std::fs::read_dir(dir) else {
    return false;
  };
  entries.flatten().any(|entry| {
    entry
      .path()
      .extension()
      .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
  })
}

/// Check if a file is a valid PE executable by reading its magic bytes (MZ).
/// Returns false for archive files (.zip starts with PK, etc.) that were
/// incorrectly named with a .exe extension.
#[cfg(target_os = "windows")]
fn is_pe_executable(path: &Path) -> bool {
  use std::io::Read;
  let Ok(mut file) = std::fs::File::open(path) else {
    return false;
  };
  let mut magic = [0u8; 2];
  if file.read_exact(&mut magic).is_err() {
    return false;
  }
  magic == [0x4D, 0x5A] // MZ
}

// Factory function to create browser instances (kept for backward compatibility)
pub fn create_browser(browser_type: BrowserType) -> Box<dyn Browser> {
  BrowserFactory::instance().create_browser(browser_type)
}

// Add GithubRelease and GithubAsset structs to browser.rs if they don't already exist
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubRelease {
  pub tag_name: String,
  #[serde(default)]
  pub name: String,
  pub assets: Vec<GithubAsset>,
  #[serde(default)]
  pub published_at: String,
  #[serde(default)]
  pub is_nightly: bool,
  #[serde(default)]
  pub prerelease: bool,
  #[serde(default)]
  pub draft: bool,
  #[serde(default)]
  pub body: Option<String>,
  #[serde(default)]
  pub html_url: Option<String>,
  #[serde(default)]
  pub id: Option<u64>,
  #[serde(default)]
  pub node_id: Option<String>,
  #[serde(default)]
  pub target_commitish: Option<String>,
  #[serde(default)]
  pub created_at: Option<String>,
  #[serde(default)]
  pub tarball_url: Option<String>,
  #[serde(default)]
  pub zipball_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubAsset {
  pub name: String,
  pub browser_download_url: String,
  #[serde(default)]
  pub size: u64,
  #[serde(default)]
  pub download_count: Option<u64>,
  #[serde(default)]
  pub id: Option<u64>,
  #[serde(default)]
  pub node_id: Option<String>,
  #[serde(default)]
  pub label: Option<String>,
  #[serde(default)]
  pub content_type: Option<String>,
  #[serde(default)]
  pub state: Option<String>,
  #[serde(default)]
  pub created_at: Option<String>,
  #[serde(default)]
  pub updated_at: Option<String>,
}

// Global singleton instance
static BROWSER_FACTORY: std::sync::LazyLock<BrowserFactory> =
  std::sync::LazyLock::new(BrowserFactory::new);

#[cfg(test)]
mod tests {
  use super::*;

  #[cfg(target_os = "macos")]
  #[test]
  fn test_wayfern_named_app_bundle_is_found() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    // New release layout: Wayfern.app/Contents/MacOS/Wayfern
    let macos_dir = install_dir
      .join("Wayfern.app")
      .join("Contents")
      .join("MacOS");
    std::fs::create_dir_all(&macos_dir).unwrap();
    std::fs::File::create(macos_dir.join("Wayfern")).unwrap();
    // Helper binaries in the same dir must not be picked as the main executable.
    std::fs::File::create(macos_dir.join("chrome_crashpad_handler")).unwrap();

    let exe = WayfernBrowser::new()
      .get_executable_path(install_dir)
      .expect("Wayfern executable should be found");
    assert_eq!(exe.file_name().unwrap().to_str().unwrap(), "Wayfern");
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn test_legacy_chromium_app_bundle_still_found() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    // Builds extracted before the rename keep the Chromium.app layout.
    let macos_dir = install_dir
      .join("Chromium.app")
      .join("Contents")
      .join("MacOS");
    std::fs::create_dir_all(&macos_dir).unwrap();
    std::fs::File::create(macos_dir.join("Chromium")).unwrap();

    let exe = WayfernBrowser::new()
      .get_executable_path(install_dir)
      .expect("legacy Chromium executable should still be found");
    assert_eq!(exe.file_name().unwrap().to_str().unwrap(), "Chromium");
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn test_wayfern_linux_executable_preferred_over_legacy() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    // Both the new `wayfern` and a legacy `chrome` are present; wayfern wins.
    std::fs::File::create(install_dir.join("chrome")).unwrap();
    std::fs::File::create(install_dir.join("wayfern")).unwrap();

    let exe = WayfernBrowser::new()
      .get_executable_path(install_dir)
      .expect("Wayfern executable should be found");
    assert_eq!(exe.file_name().unwrap().to_str().unwrap(), "wayfern");
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn test_wayfern_linux_subdir_layout_found() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    // Archive that unpacks into a wayfern-linux/ subdirectory.
    let subdir = install_dir.join("wayfern-linux");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::File::create(subdir.join("wayfern")).unwrap();

    let exe = WayfernBrowser::new()
      .get_executable_path(install_dir)
      .expect("Wayfern executable in subdir should be found");
    assert!(exe.ends_with(std::path::Path::new("wayfern-linux").join("wayfern")));
  }

  #[cfg(target_os = "windows")]
  #[test]
  fn test_wayfern_windows_executable_preferred_over_legacy() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    std::fs::File::create(install_dir.join("chrome.exe")).unwrap();
    std::fs::File::create(install_dir.join("wayfern.exe")).unwrap();

    let exe = WayfernBrowser::new()
      .get_executable_path(install_dir)
      .expect("Wayfern executable should be found");
    assert_eq!(exe.file_name().unwrap().to_str().unwrap(), "wayfern.exe");
  }

  #[cfg(target_os = "windows")]
  #[test]
  fn test_wayfern_windows_subdir_layout_found() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    // Archive that unpacks into a wayfern-win/ subdirectory.
    let subdir = install_dir.join("wayfern-win");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::File::create(subdir.join("wayfern.exe")).unwrap();

    let exe = WayfernBrowser::new()
      .get_executable_path(install_dir)
      .expect("Wayfern executable in subdir should be found");
    assert!(exe.ends_with(std::path::Path::new("wayfern-win").join("wayfern.exe")));
  }

  /// A gutted Windows install (the `.exe` survived a cleanup pass that deleted
  /// every `.dll` and the `.manifest`) must not read as downloaded, otherwise it
  /// is re-registered as healthy and launching it fails with os error 14001.
  /// Runs on every platform because the predicate is platform-independent.
  #[test]
  fn test_lone_exe_is_not_a_valid_windows_install() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    let exe = install_dir.join("chrome.exe");
    std::fs::File::create(&exe).unwrap();
    assert!(
      !has_sibling_dll(&exe),
      "an .exe with no sibling .dll is a gutted install"
    );

    std::fs::File::create(install_dir.join("chrome.dll")).unwrap();
    assert!(
      has_sibling_dll(&exe),
      "an .exe next to its libraries is a complete install"
    );
  }

  /// The DLL check is scoped to the executable's own directory, so the nested
  /// `chrome-win/` and `wayfern-win/` layouts are not falsely rejected because
  /// the version root happens to hold no libraries.
  #[test]
  fn test_sibling_dll_check_is_scoped_to_the_executable_directory() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    let subdir = install_dir.join("chrome-win");
    std::fs::create_dir_all(&subdir).unwrap();
    let exe = subdir.join("chrome.exe");
    std::fs::File::create(&exe).unwrap();
    std::fs::File::create(subdir.join("CHROME.DLL")).unwrap();

    assert!(
      has_sibling_dll(&exe),
      "libraries beside the executable count regardless of case or nesting"
    );

    let root_exe = install_dir.join("chrome.exe");
    std::fs::File::create(&root_exe).unwrap();
    assert!(
      !has_sibling_dll(&root_exe),
      "libraries in a sibling subdirectory must not validate a bare root .exe"
    );
  }

  #[test]
  fn test_cloak_browser_type_from_str() {
    assert_eq!(BrowserType::from_str("cloak").unwrap(), BrowserType::Cloak);
    assert_eq!(BrowserType::Cloak.as_str(), "cloak");
    assert_eq!(BrowserType::Cloak.display_name(), "CloakBrowser");
    assert!(BrowserType::from_str("wayfern").is_ok());
  }

  #[test]
  fn test_cloak_free_version_from_tag() {
    assert_eq!(
      cloak_free_version_from_tag("chromium-v146.0.7680.177.5"),
      Some("146.0.7680.177.5".to_string())
    );
    // Key-gated -pro builds are never selected.
    assert_eq!(
      cloak_free_version_from_tag("chromium-v154.0.8037.57.2-pro"),
      None
    );
    assert_eq!(cloak_free_version_from_tag("v146.0.7680.177.5"), None);
  }

  fn cloak_test_release(tag: &str, assets: &[&str]) -> GithubRelease {
    GithubRelease {
      tag_name: tag.to_string(),
      name: String::new(),
      assets: assets
        .iter()
        .map(|name| GithubAsset {
          name: name.to_string(),
          browser_download_url: format!("https://example.com/{tag}/{name}"),
          size: 0,
          download_count: None,
          id: None,
          node_id: None,
          label: None,
          content_type: None,
          state: None,
          created_at: None,
          updated_at: None,
        })
        .collect(),
      published_at: String::new(),
      is_nightly: false,
      prerelease: false,
      draft: false,
      body: None,
      html_url: None,
      id: None,
      node_id: None,
      target_commitish: None,
      created_at: None,
      tarball_url: None,
      zipball_url: None,
    }
  }

  #[test]
  fn test_select_cloak_release_picks_newest_free_with_asset() {
    let releases = vec![
      // Newest, but key-gated: no binary asset and a -pro tag.
      cloak_test_release("chromium-v154.0.8037.57.2-pro", &["SHA256SUMS"]),
      cloak_test_release(
        "chromium-v146.0.7680.177.5",
        &["cloakbrowser-linux-x64.tar.gz", "SHA256SUMS"],
      ),
      cloak_test_release(
        "chromium-v145.0.7632.159.9",
        &["cloakbrowser-linux-x64.tar.gz", "SHA256SUMS"],
      ),
    ];
    let (version, url) =
      select_cloak_release(&releases, None, "cloakbrowser-linux-x64.tar.gz").unwrap();
    assert_eq!(version, "146.0.7680.177.5");
    assert!(url.contains("chromium-v146.0.7680.177.5"));
  }

  #[test]
  fn test_select_cloak_release_version_pin() {
    let releases = vec![
      cloak_test_release(
        "chromium-v146.0.7680.177.5",
        &["cloakbrowser-linux-x64.tar.gz"],
      ),
      cloak_test_release(
        "chromium-v145.0.7632.159.9",
        &["cloakbrowser-linux-x64.tar.gz"],
      ),
    ];
    let (version, _) = select_cloak_release(
      &releases,
      Some("145.0.7632.159.9"),
      "cloakbrowser-linux-x64.tar.gz",
    )
    .unwrap();
    assert_eq!(version, "145.0.7632.159.9");
    assert!(select_cloak_release(
      &releases,
      Some("999.0.0.0"),
      "cloakbrowser-linux-x64.tar.gz"
    )
    .is_none());
  }

  #[test]
  fn test_cloak_launch_args_base() {
    let args = CloakBrowser::new()
      .create_launch_args("/tmp/p", None, None, Some(9222), false)
      .unwrap();
    assert!(args.contains(&"--user-data-dir=/tmp/p".to_string()));
    assert!(args.contains(&"--remote-debugging-port=9222".to_string()));
    assert!(!args.iter().any(|a| a.starts_with("--headless")));
  }

  #[test]
  fn test_cloak_launch_args_proxy_scheme() {
    let proxy = |t: &str| ProxySettings {
      proxy_type: t.to_string(),
      host: "127.0.0.1".to_string(),
      port: 8080,
      username: None,
      password: None,
      vless_uri: None,
    };
    let arg_for = |t: &str| {
      CloakBrowser::new()
        .create_launch_args("/tmp/p", Some(&proxy(t)), None, None, false)
        .unwrap()
        .into_iter()
        .find(|a| a.starts_with("--proxy-server="))
        .unwrap()
    };
    assert_eq!(arg_for("http"), "--proxy-server=http://127.0.0.1:8080");
    assert_eq!(arg_for("socks5"), "--proxy-server=socks5://127.0.0.1:8080");
    assert_eq!(arg_for("socks4"), "--proxy-server=socks4://127.0.0.1:8080");
    assert_eq!(arg_for("SOCKS5"), "--proxy-server=socks5://127.0.0.1:8080");
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn test_cloak_linux_nested_chrome_found() {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let install_dir = temp.path();

    // Archive that unpacks into a single nested folder.
    let subdir = install_dir.join("cloakbrowser-linux-x64");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::File::create(subdir.join("chrome")).unwrap();

    let exe = CloakBrowser::new()
      .get_executable_path(install_dir)
      .expect("CloakBrowser executable should be found");
    assert_eq!(exe.file_name().unwrap().to_str().unwrap(), "chrome");

    // is_version_downloaded probes <binaries>/cloak/<version>.
    let binaries = temp.path().join("binaries");
    let version_dir = binaries.join("cloak").join("146.0.7680.177.5");
    std::fs::create_dir_all(&version_dir).unwrap();
    std::fs::File::create(version_dir.join("chrome")).unwrap();
    assert!(CloakBrowser::new().is_version_downloaded("146.0.7680.177.5", &binaries));
    assert!(!CloakBrowser::new().is_version_downloaded("999.0.0.0", &binaries));
  }

  #[test]
  fn test_proxy_settings_serialization() {
    let proxy = ProxySettings {
      proxy_type: "http".to_string(),
      host: "127.0.0.1".to_string(),
      port: 8080,
      username: None,
      password: None,
      vless_uri: None,
    };

    // Test that it can be serialized (implements Serialize)
    let json = serde_json::to_string(&proxy).expect("Failed to serialize proxy settings");
    assert!(json.contains("127.0.0.1"), "JSON should contain host IP");
    assert!(json.contains("8080"), "JSON should contain port number");
    assert!(json.contains("http"), "JSON should contain proxy type");

    // Test that it can be deserialized (implements Deserialize)
    let deserialized: ProxySettings =
      serde_json::from_str(&json).expect("Failed to deserialize proxy settings");
    assert_eq!(
      deserialized.proxy_type, proxy.proxy_type,
      "Proxy type should match"
    );
    assert_eq!(deserialized.host, proxy.host, "Host should match");
    assert_eq!(deserialized.port, proxy.port, "Port should match");
  }

  #[test]
  fn test_wayfern_config_has_no_executable_path() {
    // Verify WayfernConfig does not store executable_path
    let config = crate::wayfern_manager::WayfernConfig::default();
    let json = serde_json::to_value(&config).unwrap();
    assert!(
      json.get("executable_path").is_none(),
      "WayfernConfig should not have executable_path field"
    );
  }

  #[test]
  fn test_profile_data_path_is_dynamic() {
    use crate::profile::BrowserProfile;
    let profiles_dir = std::path::PathBuf::from("/fake/profiles");
    let profile = BrowserProfile {
      id: uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
      name: "test".to_string(),
      browser: "wayfern".to_string(),
      version: "1.0.0".to_string(),
      proxy_id: None,
      vpn_id: None,
      launch_hook: None,
      process_id: None,
      last_launch: None,
      release_type: "stable".to_string(),
      wayfern_config: None,
      group_id: None,
      tags: Vec::new(),
      note: None,
      window_color: None,
      sync_mode: crate::profile::types::SyncMode::Disabled,
      encryption_salt: None,
      last_sync: None,
      host_os: None,
      ephemeral: false,
      temporary: false,
      extension_group_id: None,
      proxy_bypass_rules: Vec::new(),
      created_by_id: None,
      created_by_email: None,
      dns_blocklist: None,
      dns_server: None,
      status_id: None,
      start_urls: Vec::new(),
      password_protected: false,
      clear_on_close: false,
      created_at: None,
      updated_at: None,
    };

    let path = profile.get_profile_data_path(&profiles_dir);
    assert_eq!(
      path,
      profiles_dir
        .join("12345678-1234-1234-1234-123456789abc")
        .join("profile")
    );
  }
}
