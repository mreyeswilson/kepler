//! Where the per-user daemon listens. Naming rules are part of the protocol (ADR-0007).

use std::fmt;

pub const DEFAULT_NAME: &str = "daemon";
pub const ENV_NAME: &str = "KEPLER_IPC_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidName;

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("endpoint name must match [a-z0-9_-]{1,32}")
    }
}

impl std::error::Error for InvalidName {}

impl Endpoint {
    /// `name` must match `[a-z0-9_-]{1,32}` so it is safe in a file or pipe name.
    pub fn new(name: &str) -> Result<Self, InvalidName> {
        let ok = !name.is_empty()
            && name.len() <= 32
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if ok {
            Ok(Self {
                name: name.to_string(),
            })
        } else {
            Err(InvalidName)
        }
    }

    /// `KEPLER_IPC_NAME` if set and valid, else `daemon`.
    pub fn from_env() -> Self {
        std::env::var(ENV_NAME)
            .ok()
            .and_then(|n| Self::new(&n).ok())
            .unwrap_or_else(|| Self {
                name: DEFAULT_NAME.to_string(),
            })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sanitized OS user name used to keep endpoints of different users apart.
    pub fn user_tag() -> String {
        let raw = ["USERNAME", "USER", "LOGNAME"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "user".to_string());
        let clean: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(32)
            .collect();
        clean
    }

    /// Windows named pipe path.
    pub fn pipe_name(&self) -> String {
        format!(r"\\.\pipe\kepler-{}-{}", Self::user_tag(), self.name)
    }

    /// Human readable location (for `kepler doctor`).
    pub fn describe(&self) -> String {
        #[cfg(windows)]
        {
            self.pipe_name()
        }
        #[cfg(unix)]
        {
            match crate::transport::unix_socket_path(self) {
                Ok(p) => p.display().to_string(),
                Err(e) => format!("<unavailable: {e}>"),
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            format!("<unsupported platform: {}>", self.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated() {
        assert!(Endpoint::new("daemon").is_ok());
        assert!(Endpoint::new("test-1_a").is_ok());
        for bad in ["", "A", "a/b", "..", "a b", &"x".repeat(33), "a\\b"] {
            assert!(Endpoint::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pipe_name_has_user_and_name() {
        let n = Endpoint::new("x1").unwrap().pipe_name();
        assert!(n.starts_with(r"\\.\pipe\kepler-"));
        assert!(n.ends_with("-x1"));
        assert!(!Endpoint::user_tag().contains(['/', '\\', ' ']));
    }
}
