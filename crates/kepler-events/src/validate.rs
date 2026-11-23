use crate::error::EventError;

/// Identifiers: 1..=max bytes of `[A-Za-z0-9._:-]`. No path separators,
/// whitespace or control characters, so they are safe in logs and file names.
pub fn is_valid_identifier(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

pub(crate) fn check_identifier(field: &'static str, s: &str, max: usize) -> Result<(), EventError> {
    if s.len() > max {
        return Err(EventError::FieldTooLarge { field, max });
    }
    if !is_valid_identifier(s, max) {
        return Err(EventError::field(field, "must match [A-Za-z0-9._:-]+"));
    }
    Ok(())
}

pub(crate) fn check_text(field: &'static str, s: &str, max: usize) -> Result<(), EventError> {
    if s.len() > max {
        return Err(EventError::FieldTooLarge { field, max });
    }
    if s.contains('\0') {
        return Err(EventError::field(field, "contains NUL"));
    }
    Ok(())
}

/// Strict RFC 3339 `date-time` check without extra dependencies:
/// `YYYY-MM-DDTHH:MM:SS[.frac](Z|(+|-)HH:MM)`.
pub fn is_valid_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 || b.len() > 40 || !s.is_ascii() {
        return false;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = &s[r];
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (
        num(0..4),
        num(5..7),
        num(8..10),
        num(11..13),
        num(14..16),
        num(17..19),
    ) else {
        return false;
    };
    if b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b't') {
        return false;
    }
    if b[13] != b':' || b[16] != b':' {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if d < 1 || d > dim || h > 23 || mi > 59 || sec > 60 {
        return false;
    }
    let mut i = 19;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start || i - start > 9 {
            return false;
        }
    }
    match &b[i..] {
        [b'Z'] | [b'z'] => true,
        [sign, ..] if (*sign == b'+' || *sign == b'-') && b.len() - i == 6 => {
            match (num(i + 1..i + 3), num(i + 4..i + 6)) {
                (Some(oh), Some(om)) => b[i + 3] == b':' && oh <= 23 && om <= 59,
                _ => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_accepts_valid() {
        for ok in [
            "2026-01-02T03:04:05Z",
            "2026-01-02T03:04:05.123Z",
            "2024-02-29T00:00:00+02:00",
            "2026-12-31T23:59:59.999999999-05:30",
        ] {
            assert!(is_valid_rfc3339(ok), "{ok}");
        }
    }

    #[test]
    fn rfc3339_rejects_invalid() {
        for bad in [
            "",
            "2026-01-02",
            "2026-13-02T03:04:05Z",
            "2026-02-30T03:04:05Z",
            "2025-02-29T03:04:05Z",
            "2026-01-02T25:04:05Z",
            "2026-01-02 03:04:05Z",
            "2026-01-02T03:04:05",
            "2026-01-02T03:04:05.Z",
            "2026-01-02T03:04:05+0200",
            "2026-01-02T03:04:05ZZ",
            "２０２６-01-02T03:04:05Z",
        ] {
            assert!(!is_valid_rfc3339(bad), "{bad}");
        }
    }

    #[test]
    fn identifiers() {
        assert!(is_valid_identifier("sess-1.a:b_c", 128));
        assert!(!is_valid_identifier("", 128));
        assert!(!is_valid_identifier("a/b", 128));
        assert!(!is_valid_identifier("..\\x", 128));
        assert!(!is_valid_identifier("a b", 128));
        assert!(!is_valid_identifier("a\n", 128));
        assert!(!is_valid_identifier(&"a".repeat(129), 128));
    }
}
