//! Static preview renderer: a validated [`Candidate`] plus the project's design tokens become a
//! single self-contained HTML wireframe page.
//!
//! The page is produced by this module's own templates. No model text is ever emitted as markup:
//! every string is HTML-escaped, the structure comes from closed enums, and the only CSS that
//! depends on input are token values that already passed `inspect::clean_value`. The page has
//! no `<script>`, no event handlers, no remote resources, no `<form>` elements and a CSP that
//! forbids everything except inline styles.

use crate::inspect::{clean_value, ProjectContext, Token};
use crate::spec::{Candidate, Component, Screen};

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if c.is_control() => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// A token value usable in CSS (bare HSL triplets from shadcn become `hsl(...)`).
fn css_color(v: &str) -> Option<String> {
    let v = clean_value(v)?;
    if v.contains("/*") || v.contains("*/") {
        return None;
    }
    let parts: Vec<&str> = v.split_whitespace().collect();
    if parts.len() == 3
        && parts[1].ends_with('%')
        && parts[2].ends_with('%')
        && parts[0].parse::<f64>().is_ok()
    {
        return Some(format!("hsl({v})"));
    }
    Some(v)
}

fn css_plain(v: &str) -> Option<String> {
    let v = clean_value(v)?;
    (!v.contains("/*") && !v.contains("*/")).then_some(v)
}

fn pick<'a>(tokens: &'a [Token], names: &[&str]) -> Option<&'a Token> {
    for n in names {
        if let Some(t) = tokens.iter().find(|t| t.name.eq_ignore_ascii_case(n)) {
            return Some(t);
        }
    }
    for n in names {
        if let Some(t) = tokens
            .iter()
            .find(|t| t.name.to_ascii_lowercase().contains(n))
        {
            return Some(t);
        }
    }
    None
}

/// Rough "is this color dark" for choosing readable text on it.
fn is_dark(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    if let Some(h) = l.strip_prefix('#') {
        let h = if h.len() == 3 {
            h.chars().flat_map(|c| [c, c]).collect::<String>()
        } else {
            h.to_string()
        };
        if h.len() >= 6 {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f64;
            return 0.299 * p(0) + 0.587 * p(2) + 0.114 * p(4) < 140.0;
        }
    }
    if let Some(inner) = l.strip_prefix("hsl(").and_then(|r| r.strip_suffix(')')) {
        if let Some(lum) = inner.split([' ', ',']).filter(|s| !s.is_empty()).nth(2) {
            return lum
                .trim_end_matches('%')
                .parse::<f64>()
                .is_ok_and(|n| n < 55.0);
        }
    }
    false
}

struct Theme {
    bg: String,
    surface: String,
    fg: String,
    muted: String,
    primary: String,
    on_primary: String,
    border: String,
    font: String,
    radius: String,
    gap: String,
    from_project: Vec<String>,
}

fn theme(ctx: &ProjectContext) -> Theme {
    let mut from = Vec::new();
    let mut color = |names: &[&str], default: &str, label: &str| -> String {
        match pick(&ctx.tokens.colors, names).and_then(|t| css_color(&t.value).map(|v| (t, v))) {
            Some((t, v)) => {
                from.push(format!("{label}: {} = {}", t.name, v));
                v
            }
            None => default.to_string(),
        }
    };
    let bg = color(
        &["background", "bg", "canvas", "surface"],
        "#f6f7f9",
        "background",
    );
    let surface = color(&["card", "surface", "paper"], "#ffffff", "surface");
    let fg = color(&["foreground", "text", "ink", "fg"], "#14171f", "text");
    let muted = color(
        &["muted-foreground", "muted", "secondary", "gray", "neutral"],
        "#667085",
        "muted",
    );
    let primary = color(&["primary", "brand", "accent"], "#3b5bdb", "primary");
    let border = color(&["border", "outline", "divider"], "#d0d5dd", "border");
    let on_primary = pick(&ctx.tokens.colors, &["primary-foreground", "on-primary"])
        .and_then(|t| css_color(&t.value))
        .unwrap_or_else(|| {
            if is_dark(&primary) {
                "#ffffff".into()
            } else {
                "#111111".into()
            }
        });
    let font = pick(&ctx.tokens.fonts, &["sans", "body", "base", "font"])
        .and_then(|t| css_plain(&t.value))
        .map(|v| format!("{v}, system-ui, -apple-system, 'Segoe UI', sans-serif"))
        .unwrap_or_else(|| "system-ui, -apple-system, 'Segoe UI', sans-serif".into());
    if let Some(t) = pick(&ctx.tokens.fonts, &["sans", "body", "base", "font"]) {
        from.push(format!("font: {}", t.name));
    }
    let radius = pick(&ctx.tokens.radii, &["radius", "lg", "md", "card", "base"])
        .and_then(|t| css_plain(&t.value))
        .unwrap_or_else(|| "8px".into());
    if let Some(t) = pick(&ctx.tokens.radii, &["radius", "lg", "md", "card", "base"]) {
        from.push(format!("radius: {}", t.name));
    }
    let gap = pick(&ctx.tokens.spacing, &["md", "4", "base", "space"])
        .and_then(|t| css_plain(&t.value))
        .unwrap_or_else(|| "16px".into());
    Theme {
        bg,
        surface,
        fg,
        muted,
        primary,
        on_primary,
        border,
        font,
        radius,
        gap,
        from_project: from,
    }
}

const CSS: &str = r#"
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font-family:var(--font);line-height:1.45;padding:24px}
h1,h2,h3{margin:0}
.top{max-width:1100px;margin:0 auto 24px}
.top h1{font-size:24px}
.top p{color:var(--muted);margin:6px 0 0}
.badge{display:inline-block;font-size:12px;border:1px solid var(--border);border-radius:99px;padding:2px 10px;color:var(--muted);margin:10px 6px 0 0}
.screen{max-width:1100px;margin:0 auto 32px}
.screen>h2{font-size:18px;margin-bottom:4px}
.purpose{color:var(--muted);margin:0 0 12px}
.frame{background:var(--bg);border:1px dashed var(--border);border-radius:var(--radius);padding:var(--gap)}
.wire{display:grid;gap:var(--gap)}
.l-single_column{grid-template-columns:minmax(0,640px)}
.l-centered_card{grid-template-columns:minmax(0,420px);justify-content:center}
.l-centered_card .sec{box-shadow:0 1px 3px rgba(0,0,0,.08)}
.l-two_column,.l-split_hero{grid-template-columns:1fr 1fr}
.l-sidebar_content{grid-template-columns:220px 1fr}
.l-grid{grid-template-columns:repeat(auto-fill,minmax(220px,1fr))}
.l-dashboard{grid-template-columns:repeat(3,1fr)}
.l-list_detail{grid-template-columns:1fr 2fr}
.sec{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);padding:var(--gap);display:flex;flex-direction:column;gap:10px}
.sec-name{font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted)}
.c h3{font-size:20px}
.c p{margin:0}
.map{display:inline-block;font-size:10px;font-family:ui-monospace,Consolas,monospace;color:var(--muted);border:1px solid var(--border);border-radius:4px;padding:0 5px;margin-left:6px;vertical-align:middle}
.lbl{display:block;font-size:13px;margin-bottom:4px}
.inp{height:36px;border:1px solid var(--border);border-radius:calc(var(--radius) - 2px);background:var(--bg)}
.inp.area{height:72px}
.btn{display:block;text-align:center;padding:9px 14px;border-radius:calc(var(--radius) - 2px);background:var(--primary);color:var(--on-primary);font-weight:600}
.lnk{color:var(--primary);text-decoration:underline;font-size:14px}
.chk,.rad,.sw{display:flex;gap:8px;align-items:center;font-size:14px}
.chk i{width:16px;height:16px;border:1px solid var(--border);border-radius:4px;background:var(--bg)}
.rad i{width:16px;height:16px;border:1px solid var(--border);border-radius:50%;background:var(--bg)}
.sw i{width:30px;height:16px;border:1px solid var(--border);border-radius:99px;background:var(--bg)}
.box{border:1px solid var(--border);border-radius:var(--radius);padding:10px;background:var(--bg);font-size:14px}
.alert{border:1px solid var(--primary);border-radius:var(--radius);padding:10px;font-size:14px}
.pill{display:inline-block;border:1px solid var(--border);border-radius:99px;padding:1px 10px;font-size:12px}
.hr{border-top:1px solid var(--border)}
.ph{height:80px;border:1px dashed var(--border);border-radius:var(--radius);display:flex;align-items:center;justify-content:center;color:var(--muted);font-size:12px}
.notes{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:var(--gap);margin-top:12px;font-size:13px}
.notes h4{margin:0 0 4px;font-size:12px;text-transform:uppercase;letter-spacing:.06em;color:var(--muted)}
.notes ul{margin:0;padding-left:18px}
.plan{max-width:1100px;margin:0 auto 24px;font-size:13px}
.plan h2{font-size:16px}
code{font-family:ui-monospace,Consolas,monospace;font-size:12px}
.disclaimer{max-width:1100px;margin:24px auto 0;color:var(--muted);font-size:12px}
@media (max-width:700px){.wire{grid-template-columns:1fr!important}body{padding:12px}}
"#;

fn mapping(c: &Component) -> String {
    match &c.library_component {
        Some(l) => format!("<span class=\"map\">{}</span>", esc(l)),
        None => String::new(),
    }
}

fn label(c: &Component, fallback: &str) -> String {
    esc(c.label.as_deref().unwrap_or(fallback))
}

fn component_html(c: &Component) -> String {
    let m = mapping(c);
    let l = |f: &str| label(c, f);
    let body = match c.kind.as_str() {
        "heading" => format!("<h3>{}{m}</h3>", l("Heading")),
        "text" => format!("<p>{}{m}</p>", l("Text")),
        "button" => format!("<div class=\"btn\">{}{m}</div>", l("Button")),
        "link" => format!("<span class=\"lnk\">{}</span>{m}", l("Link")),
        "input" | "email_input" | "password_input" | "select" => {
            let hint = match c.kind.as_str() {
                "password_input" => " (hidden)",
                "select" => " (select)",
                _ => "",
            };
            format!(
                "<span class=\"lbl\">{}{hint}{m}</span><div class=\"inp\"></div>",
                l("Field")
            )
        }
        "textarea" => format!(
            "<span class=\"lbl\">{}{m}</span><div class=\"inp area\"></div>",
            l("Message")
        ),
        "checkbox" => format!(
            "<div class=\"chk\"><i></i><span>{}{m}</span></div>",
            l("Option")
        ),
        "radio" => format!(
            "<div class=\"rad\"><i></i><span>{}{m}</span></div>",
            l("Option")
        ),
        "switch" => format!(
            "<div class=\"sw\"><i></i><span>{}{m}</span></div>",
            l("Toggle")
        ),
        "alert" => format!("<div class=\"alert\">{}{m}</div>", l("Alert")),
        "badge" => format!("<span class=\"pill\">{}</span>{m}", l("Badge")),
        "divider" => "<div class=\"hr\"></div>".to_string(),
        "image" => format!("<div class=\"ph\">image: {}{m}</div>", l("Image")),
        "icon" | "avatar" => format!("<div class=\"pill\">{}: {}{m}</div>", esc(&c.kind), l("")),
        _ => format!(
            "<div class=\"box\"><b>{}</b>: {}{m}</div>",
            esc(&c.kind),
            l("")
        ),
    };
    format!("<div class=\"c k-{}\">{body}</div>", esc(&c.kind))
}

fn list(title: &str, items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let li: String = items
        .iter()
        .map(|i| format!("<li>{}</li>", esc(i)))
        .collect();
    format!("<div><h4>{}</h4><ul>{li}</ul></div>", esc(title))
}

fn screen_html(s: &Screen) -> String {
    let secs: String = s
        .sections
        .iter()
        .map(|sec| {
            let comps: String = sec.components.iter().map(component_html).collect();
            format!(
                "<div class=\"sec\"><div class=\"sec-name\">{}</div>{comps}</div>",
                esc(&sec.name)
            )
        })
        .collect();
    let mut mapped: Vec<String> = s
        .sections
        .iter()
        .flat_map(|sec| &sec.components)
        .filter_map(|c| {
            c.library_component.as_ref().map(|l| {
                format!(
                    "{} -> {}{}",
                    c.kind,
                    l,
                    c.variant
                        .as_ref()
                        .map(|v| format!(" ({v})"))
                        .unwrap_or_default()
                )
            })
        })
        .collect();
    mapped.sort();
    mapped.dedup();
    let resp: Vec<String> = s
        .responsive
        .iter()
        .map(|r| format!("{}: {}", r.breakpoint, r.behavior))
        .collect();
    let states = vec![
        format!("empty: {}", s.states.empty),
        format!("loading: {}", s.states.loading),
        format!("error: {}", s.states.error),
    ];
    format!(
        "<section class=\"screen\"><h2>{}</h2><p class=\"purpose\">{}</p><div class=\"frame\"><div class=\"wire l-{}\">{secs}</div></div><div class=\"notes\">{}{}{}{}{}</div></section>",
        esc(&s.name),
        esc(&s.purpose),
        esc(&s.layout.kind),
        list("Layout", std::slice::from_ref(&s.layout.description)),
        list("States", &states),
        list("Responsive", &resp),
        list("Accessibility", &s.accessibility),
        list("Component mapping", &mapped),
    )
}

/// Render one candidate as a complete HTML document.
pub fn render_candidate(c: &Candidate, ctx: &ProjectContext, brief: &str) -> String {
    let t = theme(ctx);
    let screens: String = c.screens.iter().map(screen_html).collect();
    let plan: Vec<String> = c
        .implementation_plan
        .iter()
        .map(|p| format!("{} {}: {}", p.action, p.path, p.description))
        .collect();
    let used: Vec<String> = c
        .tokens_used
        .iter()
        .map(|u| {
            let flag = if c.unknown_tokens.contains(&u.token) {
                " (not found in project tokens)"
            } else {
                ""
            };
            format!("{}: {}{flag}", u.role, u.token)
        })
        .collect();
    let stack: Vec<String> = ctx
        .frameworks
        .iter()
        .chain(&ctx.styling)
        .chain(&ctx.ui_libraries)
        .cloned()
        .collect();
    let badges: String = stack
        .iter()
        .take(6)
        .map(|s| format!("<span class=\"badge\">{}</span>", esc(s)))
        .collect();
    let applied = if t.from_project.is_empty() {
        "No project tokens were found: neutral defaults are used.".to_string()
    } else {
        format!("Project tokens applied: {}", t.from_project.join("; "))
    };
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"generator\" content=\"kepler-genesis\">\n<title>{title} - Genesis preview</title>\n<style>:root{{--bg:{bg};--surface:{surface};--fg:{fg};--muted:{muted};--primary:{primary};--on-primary:{on_primary};--border:{border};--font:{font};--radius:{radius};--gap:{gap}}}{CSS}</style></head>\n<body>\n<header class=\"top\"><h1>{title}</h1><p>{rationale}</p><p><b>Goal:</b> {goal}</p><p><b>Brief:</b> {brief}</p>{badges}</header>\n{screens}\n<section class=\"plan\"><h2>Implementation plan</h2><div class=\"notes\">{plan_list}{used_list}</div></section>\n<p class=\"disclaimer\">Wireframe generated by Kepler Genesis from a model-written specification. It is a layout and token preview, not final visual design or code. {applied}</p>\n</body></html>\n",
        title = esc(&c.name),
        bg = t.bg,
        surface = t.surface,
        fg = t.fg,
        muted = t.muted,
        primary = t.primary,
        on_primary = t.on_primary,
        border = t.border,
        font = t.font,
        radius = t.radius,
        gap = t.gap,
        rationale = esc(&c.rationale),
        goal = esc(&c.goal),
        brief = esc(brief),
        plan_list = list("Files", &plan),
        used_list = list("Tokens used", &used),
        applied = esc(&applied),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::Tokens;
    use crate::spec::{parse_answer, tests::good_candidate};
    use serde_json::json;
    use std::collections::BTreeSet;

    fn candidate_from(v: serde_json::Value) -> Candidate {
        let a = json!({"schema_version":1,"candidates":[v]}).to_string();
        parse_answer(&a, &BTreeSet::new(), 1)
            .unwrap()
            .candidates
            .remove(0)
    }

    fn ctx_with_tokens() -> ProjectContext {
        let mut tokens = Tokens::default();
        tokens.colors.push(Token {
            name: "brand".into(),
            value: "#4f46e5".into(),
            source: "t".into(),
        });
        tokens.colors.push(Token {
            name: "background".into(),
            value: "0 0% 100%".into(),
            source: "t".into(),
        });
        tokens.fonts.push(Token {
            name: "font-sans".into(),
            value: "'Inter', sans-serif".into(),
            source: "t".into(),
        });
        tokens.radii.push(Token {
            name: "radius".into(),
            value: "12px".into(),
            source: "t".into(),
        });
        ProjectContext {
            frameworks: vec!["React 18".into()],
            tokens,
            ..Default::default()
        }
    }

    #[test]
    fn page_is_static_self_contained_and_applies_project_tokens() {
        let html = render_candidate(
            &candidate_from(good_candidate()),
            &ctx_with_tokens(),
            "login screen",
        );
        let low = html.to_lowercase();
        for forbidden in [
            "<script",
            "onerror",
            "onclick",
            "onload",
            "<iframe",
            "<link",
            "<form",
            "<img",
            "http://",
            "https://",
            "src=",
            "javascript:",
            "@import",
            "url(",
        ] {
            assert!(!low.contains(forbidden), "page contains {forbidden}");
        }
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("--primary:#4f46e5"));
        assert!(html.contains("--bg:hsl(0 0% 100%)"));
        assert!(html.contains("'Inter', sans-serif, system-ui"));
        assert!(html.contains("--radius:12px"));
        assert!(html.contains("Welcome back") && html.contains("Forgot your password?"));
        assert!(html.contains("Project tokens applied"));
    }

    #[test]
    fn hostile_text_is_escaped_everywhere() {
        let mut c = good_candidate();
        // `<>` are removed by the validator; the renderer must still escape what remains.
        c["name"] = json!("\"onmouseover=\"alert(1)\" & 'x'");
        c["goal"] = json!("a & b \" ' `");
        c["screens"][0]["sections"][0]["components"][0]["label"] = json!("&lt;b&gt; &amp;");
        let html = render_candidate(
            &candidate_from(c),
            &ctx_with_tokens(),
            "brief <script>alert(1)</script> & \"q\"",
        );
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;q&quot;"));
        assert!(
            html.contains("&amp;lt;b&amp;gt; &amp;amp;"),
            "already-escaped input must be escaped again"
        );
        assert!(!html.contains("\"onmouseover=\"alert"));
        assert!(esc("<a href='x'>\u{0}</a>")
            .chars()
            .all(|c| !matches!(c, '<' | '>' | '\0')));
    }

    #[test]
    fn hostile_token_values_never_reach_the_stylesheet() {
        let mut ctx = ctx_with_tokens();
        ctx.tokens.colors.push(Token {
            name: "primary".into(),
            value: "red;} body{display:none} /*".into(),
            source: "t".into(),
        });
        ctx.tokens.fonts[0].value = "x;}</style><script>alert(1)</script>".into();
        let html = render_candidate(&candidate_from(good_candidate()), &ctx, "b");
        assert!(!html.contains("display:none") && !html.to_lowercase().contains("<script"));
        assert!(
            html.contains("--primary:#3b5bdb"),
            "falls back to the safe default"
        );
        assert_eq!(html.matches("</style>").count(), 1);
    }

    #[test]
    fn empty_project_gets_neutral_defaults() {
        let html = render_candidate(
            &candidate_from(good_candidate()),
            &ProjectContext::default(),
            "b",
        );
        assert!(html.contains("No project tokens were found"));
        assert!(html.contains("--primary:#3b5bdb"));
    }

    #[test]
    fn dark_detection_picks_readable_text() {
        assert!(is_dark("#111111") && !is_dark("#ffffff"));
        assert!(is_dark("hsl(222 47% 11%)") && !is_dark("hsl(0 0% 100%)"));
    }
}
