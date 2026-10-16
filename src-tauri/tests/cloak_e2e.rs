//! End-to-end test: launch the real CloakBrowser binary with the exact args our
//! code generates (`create_launch_args` + `fingerprint_args`), then verify the
//! spoofed fingerprint via `--dump-dom`.
//!
//! Requires the extracted binary. Run:
//!   CLOAK_E2E_BIN=/path/to/chrome cargo test --test cloak_e2e -- --ignored --nocapture
//!
//! Ignored by default: needs network (binary download) and ~1GB disk.

use donutbrowser_lib::browser::{BrowserType, FingerprintMaterial};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn chrome_bin() -> String {
  std::env::var("CLOAK_E2E_BIN").expect("CLOAK_E2E_BIN must point at the extracted chrome binary")
}

/// HTML page that writes the observable fingerprint into the DOM.
const PROBE_HTML: &str = r#"<!doctype html><html><body><script>
document.write([
  "ua=" + navigator.userAgent,
  "platform=" + navigator.platform,
  "tz=" + Intl.DateTimeFormat().resolvedOptions().timeZone,
  "screen=" + screen.width + "x" + screen.height,
  "lang=" + navigator.language,
  "hwc=" + navigator.hardwareConcurrency
].join("\n"));
</script></body></html>"#;

#[test]
#[ignore]
fn cloak_fingerprint_flags_take_effect_e2e() {
  let chrome = chrome_bin();
  assert!(
    std::path::Path::new(&chrome).exists(),
    "chrome binary not found at {chrome}"
  );

  // The material a real launch would build for a Jakarta macOS profile.
  let fp = FingerprintMaterial {
    seed: Some("e2e-test-seed-0123456789abcdef".to_string()),
    os: Some("macos".to_string()),
    timezone: Some("Asia/Jakarta".to_string()),
    latitude: Some(-6.2),
    longitude: Some(106.8),
    locale: Some("id-ID".to_string()),
    screen_width: Some(1920),
    screen_height: Some(1080),
    webrtc_blocked: false,
  };

  let browser = donutbrowser_lib::browser::create_browser(BrowserType::Cloak);

  // Exactly what launch_cloak_profile passes, minus CDP (dump-dom instead).
  let dir = tempfile::tempdir().expect("tempdir");
  let profile_path = dir.path().join("profile");
  let probe_path = dir.path().join("probe.html");
  std::fs::write(&probe_path, PROBE_HTML).expect("write probe");

  let mut args = browser
    .create_launch_args(
      &profile_path.to_string_lossy(),
      None,
      None,
      None,
      true, // headless
    )
    .expect("launch args");
  args.extend(browser.fingerprint_args(&fp));
  // Test-only: the VM runs as root, which Chromium refuses without this.
  // The real launch path never adds it (end-user machines are not root).
  args.push("--no-sandbox".to_string());
  args.push("--dump-dom".to_string());
  args.push(format!("file://{}", probe_path.display()));

  println!("spawning {chrome} with {} args", args.len());
  let mut child = Command::new(&chrome)
    .args(&args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("spawn chrome");

  let timeout = Duration::from_secs(60);
  let start = std::time::Instant::now();
  let output = loop {
    match child.try_wait().expect("try_wait") {
      Some(status) => {
        assert!(status.success(), "chrome exited with {status}");
        break child.wait_with_output().expect("output");
      }
      None => {
        assert!(start.elapsed() < timeout, "chrome did not exit in 60s");
        std::thread::sleep(Duration::from_millis(200));
      }
    }
  };

  let stdout = String::from_utf8_lossy(&output.stdout);
  println!("--- dump-dom ---\n{stdout}\n----------------");
  let _ = std::io::stdout().flush();

  // Timezone + locale + screen are the flags we can observe headlessly.
  assert!(
    stdout.contains("tz=Asia/Jakarta"),
    "timezone flag did not take effect"
  );
  assert!(
    stdout.contains("lang=id-ID"),
    "locale flag did not take effect"
  );
  assert!(
    stdout.contains("screen=1920x1080"),
    "screen flags did not take effect"
  );
  // Platform spoof: with --fingerprint-platform=macOS the UA should mention
  // macOS rather than Linux/X11.
  assert!(
    stdout.to_ascii_lowercase().contains("mac"),
    "platform flag did not take effect (no 'mac' in UA/platform)"
  );
  assert!(
    !stdout.contains("Linux"),
    "platform flag did not take effect (still reports Linux)"
  );
}

#[test]
#[ignore]
fn cloak_launch_args_smoke_e2e() {
  // Minimal: binary starts headless and answers --version.
  let chrome = chrome_bin();
  let out = Command::new(&chrome)
    .arg("--version")
    .output()
    .expect("run --version");
  let v = String::from_utf8_lossy(&out.stdout);
  println!("version: {v}");
  assert!(v.contains("146"), "expected Chromium 146, got: {v}");
}
