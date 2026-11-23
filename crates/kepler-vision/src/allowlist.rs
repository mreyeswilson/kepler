//! URL allowlist (the worker applies the same rules again, also to every sub-request).
//! Default: `http(s)` to `localhost`, `127.0.0.1`, `[::1]` and `*.localhost` only. Extra hosts
//! are opt-in (`KEPLER_VISION_ALLOW_HOSTS`: exact names or `*.suffix`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Invalid,
    Scheme,
    Credentials,
    Host,
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "not a valid http(s) URL",
            Self::Scheme => "only http and https are allowed",
            Self::Credentials => "credentials in the URL are not allowed",
            Self::Host => "host is not on the allowlist (default: localhost only)",
        })
    }
}

impl std::error::Error for UrlError {}

/// Comma separated list from the environment value; invalid entries are dropped, at most 32.
pub fn parse_allowed_hosts(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| {
            !h.is_empty()
                && h.len() <= 253
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '*' | '_' | '-'))
        })
        .take(32)
        .collect()
}

fn is_local(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host.ends_with(".localhost")
}

fn host_allowed(host: &str, extra: &[String]) -> bool {
    is_local(host)
        || extra.iter().any(|e| match e.strip_prefix("*.") {
            Some(suffix) => host.ends_with(&format!(".{suffix}")),
            None => host == e,
        })
}

/// Validate a URL string without any URL library: strict scheme, no userinfo, explicit host.
pub fn check_url(raw: &str, extra_hosts: &[String]) -> Result<(), UrlError> {
    if raw.is_empty()
        || raw.len() > 2048
        || raw
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(UrlError::Invalid);
    }
    let lower = raw.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("http://") {
        r
    } else if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if lower.contains("://")
        || lower.starts_with("data:")
        || lower.starts_with("javascript:")
    {
        return Err(UrlError::Scheme);
    } else {
        return Err(UrlError::Invalid);
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(UrlError::Credentials);
    }
    let (host, port) = if let Some(end) = authority.find(']') {
        // `[::1]:8080`
        let (h, p) = authority.split_at(end + 1);
        (h, p.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(UrlError::Invalid);
    }
    if let Some(p) = port {
        if p.is_empty() || p.parse::<u16>().is_err() {
            return Err(UrlError::Invalid);
        }
    }
    if host_allowed(host, extra_hosts) {
        Ok(())
    } else {
        Err(UrlError::Host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_urls_pass_everything_else_fails() {
        for ok in [
            "http://localhost:3000/a?x=1",
            "https://127.0.0.1/",
            "http://[::1]:8080/p",
            "http://app.localhost",
            "HTTP://LOCALHOST/",
        ] {
            assert_eq!(check_url(ok, &[]), Ok(()), "{ok}");
        }
        for (bad, err) in [
            ("https://example.com/", UrlError::Host),
            ("http://localhost.evil.com/", UrlError::Host),
            ("http://127.0.0.1.evil.com/", UrlError::Host),
            ("http://user:pw@localhost/", UrlError::Credentials),
            ("http://localhost@evil.com/", UrlError::Credentials),
            ("file:///etc/passwd", UrlError::Scheme),
            ("ftp://localhost/", UrlError::Scheme),
            ("javascript:alert(1)", UrlError::Scheme),
            ("localhost:3000", UrlError::Invalid),
            ("http://", UrlError::Invalid),
            ("http://localhost:99999/", UrlError::Invalid),
            ("http://local host/", UrlError::Invalid),
            ("http://localhost\\@evil.com", UrlError::Invalid),
            ("http://192.168.0.1/", UrlError::Host),
            ("http://169.254.169.254/latest", UrlError::Host),
            ("http://[::ffff:127.0.0.1]/", UrlError::Host),
        ] {
            assert_eq!(check_url(bad, &[]), Err(err), "{bad}");
        }
    }

    #[test]
    fn extra_hosts_are_explicit_and_wildcards_are_suffix_only() {
        let extra =
            parse_allowed_hosts(Some("staging.example.com, *.dev.example.org, bad host!, "));
        assert_eq!(extra, vec!["staging.example.com", "*.dev.example.org"]);
        assert_eq!(check_url("https://staging.example.com/", &extra), Ok(()));
        assert_eq!(check_url("https://a.dev.example.org/", &extra), Ok(()));
        assert!(check_url("https://dev.example.org/", &extra).is_err());
        assert!(check_url("https://evilstaging.example.com/", &extra).is_err());
        assert!(parse_allowed_hosts(None).is_empty());
    }
}
