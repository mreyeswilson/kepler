//! Denylist of sensitive paths and safe path resolution.
//!
//! Everything here is lexical first (no filesystem access needed) so it can be
//! applied to untrusted event payloads. `resolve_within` additionally
//! canonicalizes when the target exists, to defeat symlink escapes.

use std::path::{Component, Path, PathBuf};

const SAFE_ENV_SUFFIXES: &[&str] = &[".example", ".sample", ".template", ".dist"];
const SENSITIVE_NAMES: &[&str] = &[
    ".npmrc",
    ".netrc",
    "_netrc",
    ".pypirc",
    ".git-credentials",
    ".htpasswd",
    "credentials",
    "credentials.json",
    "secrets.json",
    "secrets.yml",
    "secrets.yaml",
    "service-account.json",
    "kubeconfig",
    "terraform.tfstate",
];
const SENSITIVE_PREFIXES: &[&str] = &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"];
const SENSITIVE_EXTS: &[&str] = &[
    "pem", "key", "p12", "pfx", "jks", "keystore", "kdbx", "ppk", "tfvars",
];
const SENSITIVE_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".kube", ".azure"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The path escapes the allowed root (`..` traversal, absolute path or symlink).
    Traversal,
    /// The path matches the sensitive-path denylist.
    Sensitive,
    /// Contains NUL or is otherwise unusable.
    Invalid,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Traversal => "path escapes allowed root",
            Self::Sensitive => "path is on the sensitive-path denylist",
            Self::Invalid => "invalid path",
        })
    }
}

impl std::error::Error for PathError {}

/// Normalize one component the way Windows/NTFS and macOS would see it:
/// strip alternate data stream suffixes, trailing dots/spaces, lowercase.
fn norm_component(c: &str) -> String {
    let base = c.split(':').next().unwrap_or("");
    base.trim_end_matches(['.', ' ']).to_lowercase()
}

fn name_is_sensitive(raw: &str) -> bool {
    // Keep a leading dot (".env") even though trailing dots are stripped.
    let n = norm_component(raw);
    if n.is_empty() {
        return false;
    }
    if n == ".env" {
        return true;
    }
    if let Some(rest) = n.strip_prefix(".env") {
        if rest.starts_with('.') || rest.starts_with('-') || rest.starts_with('_') {
            return !SAFE_ENV_SUFFIXES.contains(&rest);
        }
    }
    if SENSITIVE_NAMES.contains(&n.as_str()) {
        return true;
    }
    if SENSITIVE_PREFIXES.iter().any(|p| n.starts_with(p)) && !n.ends_with(".pub") {
        return true;
    }
    if let Some((_, ext)) = n.rsplit_once('.') {
        if SENSITIVE_EXTS.contains(&ext) {
            return true;
        }
    }
    false
}

fn split(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
}

/// True if any component of `path` (checked before *and* after resolving `..`)
/// is a sensitive file or sits under a sensitive directory.
pub fn is_sensitive_path(path: &str) -> bool {
    if path.contains('\0') {
        return true; // fail closed
    }
    let raw_hit = split(path)
        .any(|c| name_is_sensitive(c) || SENSITIVE_DIRS.contains(&norm_component(c).as_str()));
    if raw_hit {
        return true;
    }
    // Lexically resolve `..` so `a/../.env` and `x/.ssh/../id_rsa` are caught.
    let mut stack: Vec<&str> = Vec::new();
    for c in split(path) {
        if c == ".." {
            stack.pop();
        } else {
            stack.push(c);
        }
    }
    stack
        .iter()
        .any(|c| name_is_sensitive(c) || SENSITIVE_DIRS.contains(&norm_component(c).as_str()))
}

/// True if lexical resolution of a *relative* path would climb above its start.
pub fn escapes_start(path: &str) -> bool {
    let mut depth: i64 = 0;
    for c in split(path) {
        if c == ".." {
            depth -= 1;
            if depth < 0 {
                return true;
            }
        } else {
            depth += 1;
        }
    }
    false
}

/// Resolve `candidate` against `root`, rejecting traversal, sensitive targets
/// and symlink escapes. Returns the joined path (canonical if it exists).
pub fn resolve_within(root: &Path, candidate: &str) -> Result<PathBuf, PathError> {
    if candidate.contains('\0') {
        return Err(PathError::Invalid);
    }
    if is_sensitive_path(candidate) {
        return Err(PathError::Sensitive);
    }
    let cand = Path::new(candidate);
    let joined = if cand.is_absolute() {
        cand.to_path_buf()
    } else {
        root.join(cand)
    };
    // Lexical normalization.
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    return Err(PathError::Traversal);
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    let root_norm = lexical(root);
    if !out.starts_with(&root_norm) {
        return Err(PathError::Traversal);
    }
    if is_sensitive_path(&out.to_string_lossy()) {
        return Err(PathError::Sensitive);
    }
    // Symlink defence: if both exist, the real path must stay inside the real root.
    if let (Ok(real), Ok(real_root)) = (out.canonicalize(), root.canonicalize()) {
        if !real.starts_with(&real_root) {
            return Err(PathError::Traversal);
        }
        if is_sensitive_path(&real.to_string_lossy()) {
            return Err(PathError::Sensitive);
        }
        return Ok(real);
    }
    Ok(out)
}

fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_are_sensitive() {
        for p in [
            ".env",
            "./.env",
            "sub/dir/.env.local",
            "..\\.env",
            ".ENV",
            ".env.",
            ".env ",
            ".env::$DATA",
            "a/../.env",
            ".env.production",
            "C:\\proj\\.env",
        ] {
            assert!(is_sensitive_path(p), "{p}");
        }
    }

    #[test]
    fn keys_and_credentials_are_sensitive() {
        for p in [
            "~/.ssh/id_rsa",
            "/home/u/.ssh/config",
            "C:\\Users\\x\\.aws\\credentials",
            "certs/server.pem",
            "deploy.KEY",
            "id_ed25519",
            ".npmrc",
            "x/.git-credentials",
            "a/.ssh/../id_rsa",
            "bundle.pfx",
            "a\0b",
        ] {
            assert!(is_sensitive_path(p), "{p:?}");
        }
    }

    #[test]
    fn ordinary_files_are_not_sensitive() {
        for p in [
            ".env.example",
            ".env.sample",
            "src/environment.rs",
            "env.rs",
            "README.md",
            "id_rsa.pub",
            "keyboard.rs",
            "src/keys.rs",
            "monkey.txt",
            "docs/credentials-policy.md",
        ] {
            assert!(!is_sensitive_path(p), "{p}");
        }
    }

    #[test]
    fn escapes_start_detection() {
        assert!(escapes_start("../x"));
        assert!(escapes_start("a/../../x"));
        assert!(!escapes_start("a/../x"));
        assert!(!escapes_start("a/b"));
    }

    fn tmp_root() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "kepler-policy-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(p.join("src")).unwrap();
        p
    }

    #[test]
    fn resolve_within_blocks_traversal() {
        let root = tmp_root();
        assert!(resolve_within(&root, "src/main.rs").is_ok());
        assert!(resolve_within(&root, "src/../src/main.rs").is_ok());
        assert_eq!(
            resolve_within(&root, "../outside.txt"),
            Err(PathError::Traversal)
        );
        assert_eq!(
            resolve_within(&root, "src/../../outside.txt"),
            Err(PathError::Traversal)
        );
        assert_eq!(
            resolve_within(&root, "..\\..\\windows\\win.ini"),
            Err(PathError::Traversal)
        );
        let abs_outside = std::env::temp_dir().join("elsewhere.txt");
        assert_eq!(
            resolve_within(&root, &abs_outside.to_string_lossy()),
            Err(PathError::Traversal)
        );
        let abs_inside = root.join("src").join("lib.rs");
        assert!(resolve_within(&root, &abs_inside.to_string_lossy()).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_within_blocks_sensitive_even_inside_root() {
        let root = tmp_root();
        assert_eq!(resolve_within(&root, ".env"), Err(PathError::Sensitive));
        assert_eq!(
            resolve_within(&root, "src/../.env"),
            Err(PathError::Sensitive)
        );
        assert_eq!(resolve_within(&root, "a\0b"), Err(PathError::Invalid));
        let _ = std::fs::remove_dir_all(&root);
    }
}
