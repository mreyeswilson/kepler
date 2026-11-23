//! `kepler vision setup`: install the worker into the data directory. The worker's TypeScript
//! sources are embedded in the binary (Node 22.18+ runs them with built-in type stripping, so
//! there is no compile step); the only download is the pinned `playwright` npm package, and only
//! when the user runs this command explicitly. No browser is downloaded.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Pinned: matches `workers/vision/package-lock.json`.
pub const PLAYWRIGHT_VERSION: &str = "1.65.1";

const FILES: &[(&str, &str)] = &[
    (
        "src/allowlist.ts",
        include_str!("../../../workers/vision/src/allowlist.ts"),
    ),
    (
        "src/driver.ts",
        include_str!("../../../workers/vision/src/driver.ts"),
    ),
    (
        "src/main.ts",
        include_str!("../../../workers/vision/src/main.ts"),
    ),
    (
        "src/probe.ts",
        include_str!("../../../workers/vision/src/probe.ts"),
    ),
    (
        "src/protocol.ts",
        include_str!("../../../workers/vision/src/protocol.ts"),
    ),
    (
        "src/worker.ts",
        include_str!("../../../workers/vision/src/worker.ts"),
    ),
];

/// Entry point of an installed worker under `dir`.
pub fn entry_point(dir: &Path) -> PathBuf {
    dir.join("src").join("main.ts")
}

/// Whether `dir` holds a complete install (sources and the Playwright package).
pub fn is_installed(dir: &Path) -> bool {
    entry_point(dir).is_file() && dir.join("node_modules").join("playwright").is_dir()
}

fn package_json() -> String {
    format!(
        "{{\n  \"name\": \"kepler-vision-worker-installed\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"dependencies\": {{ \"playwright\": \"{PLAYWRIGHT_VERSION}\" }}\n}}\n"
    )
}

/// Write the worker sources into `dir` (no network).
pub fn write_sources(dir: &Path) -> std::io::Result<()> {
    for (rel, text) in FILES {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Normalize line endings so a Windows checkout embeds the same bytes as a Unix one.
        std::fs::write(p, text.replace("\r\n", "\n"))?;
    }
    std::fs::write(dir.join("package.json"), package_json())
}

fn npm_program() -> std::ffi::OsString {
    if let Some(p) = std::env::var_os("KEPLER_NPM").filter(|p| !p.is_empty()) {
        return p;
    }
    if cfg!(windows) {
        "npm.cmd".into()
    } else {
        "npm".into()
    }
}

/// Install the worker into `dir`: sources, then `npm install --omit=dev` for the pinned
/// `playwright` package (scripts disabled: nothing else runs).
pub fn install(dir: &Path) -> Result<(), String> {
    write_sources(dir).map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
    let status = Command::new(npm_program())
        .args([
            "install",
            "--omit=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ])
        .current_dir(dir)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run npm (is Node.js installed? set KEPLER_NPM): {e}"))?;
    if !status.success() {
        return Err(format!("`npm install` failed ({status})"));
    }
    if !is_installed(dir) {
        return Err("npm finished but the playwright package is missing".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_written_and_pin_playwright() {
        let d = std::env::temp_dir().join(format!("kepler-vsetup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        write_sources(&d).unwrap();
        let main = std::fs::read_to_string(entry_point(&d)).unwrap();
        assert!(main.contains("runWorker"));
        assert!(!main.contains('\r'));
        let pkg = std::fs::read_to_string(d.join("package.json")).unwrap();
        assert!(pkg.contains(PLAYWRIGHT_VERSION));
        assert!(!is_installed(&d), "no node_modules yet");
        let _ = std::fs::remove_dir_all(&d);
    }
}
