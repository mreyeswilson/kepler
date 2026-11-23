//! Map worker results to `audit_findings` rows (`source = vision`). Only structural facts are
//! kept: counts, rule ids, short selectors and clipped, redacted messages. No page content, no
//! screenshots, no query strings.

use crate::protocol::{CheckResult, ViewportResult};
use kepler_policy::redact::redact;
use kepler_storage::{Confidence, NewFinding, Severity};

fn base(project: &str, session: &str, r: &CheckResult, rule: &str, sev: Severity) -> NewFinding {
    NewFinding {
        project_id: project.into(),
        session_id: session.into(),
        source: "vision".into(),
        rule_id: rule.into(),
        severity: sev,
        confidence: Confidence::Confirmed,
        path: r.url.split(['?', '#']).next().unwrap_or("").to_string(),
        line: None,
        message: String::new(),
        evidence: String::new(),
    }
}

fn clip(s: &str, n: usize) -> String {
    redact(s).chars().take(n).collect()
}

fn issue_severity(rule: &str) -> (Severity, Confidence) {
    match rule {
        "a11y.img_alt" | "a11y.input_label" | "a11y.button_name" | "a11y.link_name" => {
            (Severity::Medium, Confidence::Confirmed)
        }
        "a11y.lang" | "a11y.title" => (Severity::Low, Confidence::Confirmed),
        "layout.element_overflow" => (Severity::Low, Confidence::Suspected),
        "responsive.viewport_meta" => (Severity::Info, Confidence::Confirmed),
        _ => (Severity::Info, Confidence::Suspected),
    }
}

fn viewport_findings(
    out: &mut Vec<NewFinding>,
    project: &str,
    session: &str,
    r: &CheckResult,
    v: &ViewportResult,
) {
    let tag = format!("{} {}x{}", v.name, v.width, v.height);
    if v.horizontal_overflow {
        let mut f = base(
            project,
            session,
            r,
            "vision.horizontal_overflow",
            Severity::Medium,
        );
        f.message = "The page scrolls horizontally at this viewport.".into();
        f.evidence = format!(
            "{tag}: scrollWidth={} clientWidth={}",
            v.scroll_width, v.client_width
        );
        out.push(f);
    }
    for (rule, sev, list) in [
        ("vision.page_error", Severity::Medium, &v.page_errors),
        ("vision.console_error", Severity::Low, &v.console_errors),
        ("vision.failed_request", Severity::Low, &v.failed_requests),
    ] {
        for text in list.iter().take(5) {
            let mut f = base(project, session, r, rule, sev);
            f.message = match rule {
                "vision.page_error" => "Uncaught exception in the page.",
                "vision.console_error" => "console.error output while loading.",
                _ => "A sub-request failed or was blocked by the allowlist.",
            }
            .into();
            f.evidence = format!("{tag}: {}", clip(text, 120));
            out.push(f);
        }
    }
    for issue in &v.issues {
        let (sev, conf) = issue_severity(&issue.rule);
        let mut f = base(project, session, r, &issue.rule, sev);
        f.confidence = conf;
        f.message = format!("{} element(s) affected.", issue.count);
        f.evidence = format!("{tag}: {}", clip(&issue.samples.join(", "), 120));
        out.push(f);
    }
}

/// Findings for one result. Deduplication across runs is by (rule, url, evidence) in storage.
pub fn to_findings(project: &str, session: &str, r: &CheckResult) -> Vec<NewFinding> {
    let mut out = Vec::new();
    for v in &r.viewports {
        viewport_findings(&mut out, project, session, r, v);
    }
    out.truncate(200);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Issue;

    #[test]
    fn maps_overflow_errors_and_issues_without_page_content() {
        let r = CheckResult {
            url: "http://localhost:3000/p".into(),
            title: "Secret title".into(),
            viewports: vec![ViewportResult {
                name: "mobile".into(),
                width: 375,
                height: 812,
                scroll_width: 420,
                client_width: 375,
                horizontal_overflow: true,
                console_errors: vec![format!("failed with ghp_{}", "a1B2".repeat(9))],
                page_errors: vec!["TypeError: x".into()],
                failed_requests: vec![],
                issues: vec![Issue {
                    rule: "a11y.img_alt".into(),
                    count: 2,
                    samples: vec!["img.hero".into()],
                }],
                screenshot: Some("C:/x/shot.png".into()),
            }],
        };
        let f = to_findings("p", "s", &r);
        let rules: Vec<&str> = f.iter().map(|f| f.rule_id.as_str()).collect();
        assert_eq!(
            rules,
            vec![
                "vision.horizontal_overflow",
                "vision.page_error",
                "vision.console_error",
                "a11y.img_alt"
            ]
        );
        assert!(f
            .iter()
            .all(|f| f.source == "vision" && f.path == "http://localhost:3000/p"));
        let all = f.iter().map(|f| f.evidence.clone()).collect::<String>();
        assert!(!all.contains("ghp_a1B2"), "{all}");
        assert!(!all.contains("Secret title") && !all.contains("shot.png"));
    }
}
