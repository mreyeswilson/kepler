//! `audit_findings`: Sentinel and Vision results. Text is redacted and clipped before it is
//! stored, evidence is a short excerpt (never a secret: secret rules store only the kind),
//! and the table is capped per project.

use crate::redact::redact;
use crate::StorageError;
use rusqlite::{params, Connection};

pub const MAX_FINDING_MESSAGE_CHARS: usize = 240;
pub const MAX_FINDING_EVIDENCE_CHARS: usize = 160;
pub const MAX_FINDINGS_PER_PROJECT: i64 = 5_000;
/// Findings accepted per call (bounded batches).
pub const MAX_FINDINGS_PER_BATCH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "info" => Self::Info,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => return None,
        })
    }
}

/// `Confirmed`: the pattern itself is the defect (a secret in added text). `Suspected`: a
/// heuristic that may be legitimate (an `unsafe` block, a CI change); never presented as a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Confirmed,
    Suspected,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Suspected => "suspected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFinding {
    pub project_id: String,
    pub session_id: String,
    /// `sentinel` or `vision`.
    pub source: String,
    pub rule_id: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub path: String,
    pub line: Option<u32>,
    pub message: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRow {
    pub id: i64,
    pub ts: String,
    pub project_id: String,
    pub session_id: String,
    pub source: String,
    pub rule_id: String,
    pub severity: String,
    pub confidence: String,
    pub path: String,
    pub line: Option<i64>,
    pub message: String,
    pub evidence: String,
    pub status: String,
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// FNV-1a: stable across runs (unlike `DefaultHasher`), good enough for de-duplication.
fn fingerprint(f: &NewFinding, evidence: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [&f.source, &f.rule_id, &f.path, evidence] {
        for b in part.as_bytes().iter().chain(std::iter::once(&0u8)) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Insert findings, skipping duplicates (same source/rule/path/evidence in the project, so an
/// edited line number does not create a new row). Returns how many rows were new.
pub fn store_findings(conn: &Connection, findings: &[NewFinding]) -> Result<usize, StorageError> {
    let mut inserted = 0;
    let mut projects: Vec<&str> = Vec::new();
    for f in findings.iter().take(MAX_FINDINGS_PER_BATCH) {
        let evidence = clip(&redact(&f.evidence), MAX_FINDING_EVIDENCE_CHARS);
        let message = clip(&redact(&f.message), MAX_FINDING_MESSAGE_CHARS);
        let fp = fingerprint(f, &evidence);
        let n = conn.execute(
            "INSERT OR IGNORE INTO audit_findings
             (project_id, session_id, source, rule_id, severity, confidence, path, line,
              message, evidence, fingerprint)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                f.project_id,
                f.session_id,
                f.source,
                f.rule_id,
                f.severity.as_str(),
                f.confidence.as_str(),
                clip(&redact(&f.path), 512),
                f.line,
                message,
                evidence,
                fp
            ],
        )?;
        inserted += n;
        if !projects.contains(&f.project_id.as_str()) {
            projects.push(&f.project_id);
        }
    }
    for p in projects {
        conn.execute(
            "DELETE FROM audit_findings WHERE project_id = ?1 AND id NOT IN
             (SELECT id FROM audit_findings WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![p, MAX_FINDINGS_PER_PROJECT],
        )?;
    }
    Ok(inserted)
}

#[derive(Debug, Clone, Default)]
pub struct FindingFilter {
    pub project_id: Option<String>,
    pub min_severity: Option<Severity>,
    /// Include dismissed findings.
    pub include_dismissed: bool,
    pub limit: u32,
}

/// Newest first; ordering by severity is done by the caller if wanted.
pub fn list_findings(
    conn: &Connection,
    filter: &FindingFilter,
) -> Result<Vec<FindingRow>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, project_id, session_id, source, rule_id, severity, confidence, path,
                line, message, evidence, status
         FROM audit_findings
         WHERE (?1 IS NULL OR project_id = ?1) AND (?2 = 1 OR status = 'open')
         ORDER BY id DESC LIMIT ?3",
    )?;
    let min = filter.min_severity;
    let rows = stmt.query_map(
        params![
            filter.project_id,
            i64::from(filter.include_dismissed),
            filter.limit.clamp(1, 500)
        ],
        |r| {
            Ok(FindingRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                project_id: r.get(2)?,
                session_id: r.get(3)?,
                source: r.get(4)?,
                rule_id: r.get(5)?,
                severity: r.get(6)?,
                confidence: r.get(7)?,
                path: r.get(8)?,
                line: r.get(9)?,
                message: r.get(10)?,
                evidence: r.get(11)?,
                status: r.get(12)?,
            })
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        if min.is_none_or(|m| Severity::parse(&row.severity).is_some_and(|s| s >= m)) {
            out.push(row);
        }
    }
    Ok(out)
}

/// Mark a finding dismissed (kept for audit, hidden from default listings).
pub fn dismiss_finding(conn: &Connection, id: i64) -> Result<bool, StorageError> {
    Ok(conn.execute(
        "UPDATE audit_findings SET status = 'dismissed' WHERE id = ?1 AND status = 'open'",
        [id],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate;

    fn finding(rule: &str, path: &str, evidence: &str, sev: Severity) -> NewFinding {
        NewFinding {
            project_id: "p".into(),
            session_id: "s".into(),
            source: "sentinel".into(),
            rule_id: rule.into(),
            severity: sev,
            confidence: Confidence::Suspected,
            path: path.into(),
            line: Some(3),
            message: "m".into(),
            evidence: evidence.into(),
        }
    }

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c
    }

    #[test]
    fn duplicates_are_ignored_even_when_the_line_moves() {
        let c = db();
        let a = finding("r1", "a.rs", "unsafe { x }", Severity::Medium);
        let mut b = a.clone();
        b.line = Some(99);
        assert_eq!(store_findings(&c, &[a, b]).unwrap(), 1);
        let rows = list_findings(
            &c,
            &FindingFilter {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "open");
    }

    #[test]
    fn text_is_redacted_and_clipped_and_severity_filter_works() {
        let c = db();
        let mut f = finding(
            "r2",
            "b.rs",
            &format!("token ghp_{} {}", "a".repeat(36), "x".repeat(500)),
            Severity::High,
        );
        f.message = "y".repeat(1000);
        store_findings(&c, &[f, finding("r3", "c.rs", "low", Severity::Low)]).unwrap();
        let all = list_findings(
            &c,
            &FindingFilter {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 2);
        let high = all.iter().find(|r| r.rule_id == "r2").unwrap();
        assert!(!high.evidence.contains("ghp_aaaa"), "{}", high.evidence);
        assert!(high.evidence.chars().count() <= MAX_FINDING_EVIDENCE_CHARS);
        assert!(high.message.chars().count() <= MAX_FINDING_MESSAGE_CHARS);
        let only_high = list_findings(
            &c,
            &FindingFilter {
                min_severity: Some(Severity::High),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(only_high.len(), 1);
    }

    #[test]
    fn dismissed_findings_are_hidden_by_default_and_table_is_capped() {
        let c = db();
        store_findings(&c, &[finding("r", "a", "e", Severity::Info)]).unwrap();
        assert!(dismiss_finding(&c, 1).unwrap());
        assert!(!dismiss_finding(&c, 1).unwrap());
        let n = |inc| {
            list_findings(
                &c,
                &FindingFilter {
                    include_dismissed: inc,
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap()
            .len()
        };
        assert_eq!((n(false), n(true)), (0, 1));
        let many: Vec<NewFinding> = (0..MAX_FINDINGS_PER_BATCH)
            .map(|i| finding("r", "a", &format!("e{i}"), Severity::Info))
            .collect();
        for _ in 0..12 {
            let shifted: Vec<NewFinding> = many
                .iter()
                .map(|f| NewFinding {
                    path: format!("{}{}", f.path, c.last_insert_rowid()),
                    ..f.clone()
                })
                .collect();
            store_findings(&c, &shifted).unwrap();
        }
        let total: i64 = c
            .query_row("SELECT COUNT(*) FROM audit_findings", [], |r| r.get(0))
            .unwrap();
        assert!(total <= MAX_FINDINGS_PER_PROJECT, "{total}");
    }
}
