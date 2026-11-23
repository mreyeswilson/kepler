//! Platform data directories. User-global; never inside a repository.

use std::io;
use std::path::{Path, PathBuf};

pub const ENV_HOME: &str = "KEPLER_HOME";

/// Directory holding Kepler's local data.
///
/// * `KEPLER_HOME` (explicit override, used by tests)
/// * Windows: `%LOCALAPPDATA%\kepler`
/// * macOS: `~/Library/Application Support/kepler`
/// * other Unix: `$XDG_DATA_HOME/kepler` or `~/.local/share/kepler`
pub fn data_dir() -> io::Result<PathBuf> {
    if let Some(p) = std::env::var_os(ENV_HOME).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let missing = || io::Error::other("cannot determine the user data directory");
    if cfg!(windows) {
        var("LOCALAPPDATA")
            .map(|p| p.join("kepler"))
            .ok_or_else(missing)
    } else if cfg!(target_os = "macos") {
        var("HOME")
            .map(|p| p.join("Library").join("Application Support").join("kepler"))
            .ok_or_else(missing)
    } else if let Some(x) = var("XDG_DATA_HOME") {
        Ok(x.join("kepler"))
    } else {
        var("HOME")
            .map(|p| p.join(".local").join("share").join("kepler"))
            .ok_or_else(missing)
    }
}

pub fn db_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("kepler.db"))
}

/// Create the directory (and parents); mode 0700 on Unix.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_lives_under_a_kepler_directory_not_a_repo() {
        // Avoid mutating process env in parallel tests: only check the default shape.
        if std::env::var_os(ENV_HOME).is_none() {
            let p = db_path().unwrap();
            assert!(p.to_string_lossy().to_lowercase().contains("kepler"));
            assert!(p.ends_with("kepler.db"));
        }
    }
}
