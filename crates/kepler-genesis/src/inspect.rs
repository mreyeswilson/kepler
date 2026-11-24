//! Deterministic project inspection (no model, no network, no process).
//!
//! Reads a small, allowlisted set of files inside the project root (`package.json`, Tailwind and
//! theme files, CSS files, token JSON, `components.json`, directory listings) under hard limits
//! on entries, files and bytes. It never reads `.env*`, key material or `node_modules`, and every
//! value it keeps is checked against a conservative character set, so the result is safe to show
//! to a model and to put into a stylesheet.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const CONTEXT_SCHEMA: u32 = 1;
const MAX_ENTRIES: usize = 4000;
const MAX_DEPTH: usize = 5;
const MAX_FILE_BYTES: u64 = 96 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024;
const MAX_CSS_FILES: usize = 12;
const MAX_TOKENS_PER_KIND: usize = 24;
const MAX_COMPONENTS: usize = 60;

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".output",
    "coverage",
    ".turbo",
    ".cache",
    "out",
    "vendor",
    ".venv",
    "__pycache__",
    ".idea",
    ".vscode",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Token {
    pub name: String,
    pub value: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub colors: Vec<Token>,
    pub fonts: Vec<Token>,
    pub spacing: Vec<Token>,
    pub radii: Vec<Token>,
    pub shadows: Vec<Token>,
}

impl Tokens {
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
            && self.fonts.is_empty()
            && self.spacing.is_empty()
            && self.radii.is_empty()
            && self.shadows.is_empty()
    }

    fn push(&mut self, kind: Kind, t: Token) {
        let list = match kind {
            Kind::Color => &mut self.colors,
            Kind::Font => &mut self.fonts,
            Kind::Spacing => &mut self.spacing,
            Kind::Radius => &mut self.radii,
            Kind::Shadow => &mut self.shadows,
        };
        if list.len() < MAX_TOKENS_PER_KIND && !list.iter().any(|x| x.name == t.name) {
            list.push(t);
        }
    }

    /// Names of every known token (used to flag invented token references).
    pub fn names(&self) -> BTreeSet<String> {
        [
            &self.colors,
            &self.fonts,
            &self.spacing,
            &self.radii,
            &self.shadows,
        ]
        .iter()
        .flat_map(|l| l.iter().map(|t| t.name.clone()))
        .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProjectContext {
    pub schema_version: u32,
    pub project_name: Option<String>,
    pub frameworks: Vec<String>,
    pub styling: Vec<String>,
    pub ui_libraries: Vec<String>,
    pub icon_libraries: Vec<String>,
    pub other_libraries: Vec<String>,
    pub typescript: bool,
    pub dark_mode: bool,
    pub tokens: Tokens,
    pub component_dirs: Vec<String>,
    pub components: Vec<String>,
    pub conventions: Vec<String>,
    pub files_read: u32,
    pub bytes_read: u64,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum InspectError {
    NotADirectory(String),
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotADirectory(p) => write!(f, "not a directory: {p}"),
        }
    }
}

impl std::error::Error for InspectError {}

#[derive(Clone, Copy)]
enum Kind {
    Color,
    Font,
    Spacing,
    Radius,
    Shadow,
}

// ------------------------------------------------------------------ value hygiene

/// Token/identifier names: short, no markup, no path separators beyond `.`/`-`/`/`.
pub fn clean_name(s: &str) -> Option<String> {
    let s = s.trim().trim_start_matches("--");
    if s.is_empty()
        || s.len() > 48
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        return None;
    }
    Some(s.to_string())
}

/// Values that may end up in CSS custom properties: no `;{}<>\\` or `url(`/`expr`/`@import`.
pub fn clean_value(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches("!important").trim();
    if s.is_empty() || s.len() > 96 {
        return None;
    }
    let ok = s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '#' | '%' | '.' | ',' | '(' | ')' | '/' | ' ' | '-' | '_' | '\'' | '"' | '+' | ':'
            )
    });
    let low = s.to_ascii_lowercase();
    if !ok || low.contains("url(") || low.contains("expression") || low.contains("var(") {
        return None;
    }
    Some(s.to_string())
}

fn is_color_value(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    if let Some(h) = l.strip_prefix('#') {
        return matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit());
    }
    for p in [
        "rgb(", "rgba(", "hsl(", "hsla(", "oklch(", "oklab(", "lch(", "lab(", "color(",
    ] {
        if l.starts_with(p) && l.ends_with(')') {
            return true;
        }
    }
    // shadcn style bare HSL triplet: `222.2 47.4% 11.2%`
    let parts: Vec<&str> = l.split_whitespace().collect();
    parts.len() == 3
        && parts[1].ends_with('%')
        && parts[2].ends_with('%')
        && parts
            .iter()
            .all(|p| p.trim_end_matches('%').parse::<f64>().is_ok())
}

fn classify(name: &str, value: &str) -> Option<Kind> {
    let n = name.to_ascii_lowercase();
    if n.contains("font") && !n.contains("size") && !n.contains("weight") {
        return value
            .chars()
            .any(|c| c.is_alphabetic())
            .then_some(Kind::Font);
    }
    if n.contains("radius") || n.contains("rounded") {
        return Some(Kind::Radius);
    }
    if n.contains("shadow") {
        return Some(Kind::Shadow);
    }
    if n.contains("space") || n.contains("spacing") || n.contains("gap") {
        return Some(Kind::Spacing);
    }
    is_color_value(value).then_some(Kind::Color)
}

// ------------------------------------------------------------------ CSS

/// `--name: value` declarations from `:root`/`html`/`@theme` blocks; `dark` is set when a dark
/// theme block exists.
fn parse_css(text: &str) -> (Vec<(String, String)>, bool) {
    let mut cleaned = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        cleaned.push_str(&rest[..i]);
        rest = match rest[i + 2..].find("*/") {
            Some(j) => &rest[i + 2 + j + 2..],
            None => "",
        };
    }
    cleaned.push_str(rest);
    let mut stack: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut out = Vec::new();
    let mut dark = false;
    let flush = |buf: &mut String, stack: &Vec<String>, out: &mut Vec<(String, String)>| {
        let decl = buf.trim().to_string();
        buf.clear();
        if let Some(d) = decl.strip_prefix("--") {
            if let Some((k, v)) = d.split_once(':') {
                let sel = stack.last().map(String::as_str).unwrap_or("");
                if sel.contains(":root") || sel.starts_with("@theme") || sel == "html" {
                    out.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        }
    };
    for ch in cleaned.chars() {
        match ch {
            '{' => {
                let sel = buf.trim().to_ascii_lowercase();
                if sel.contains(".dark") || sel.contains("data-theme") && sel.contains("dark") {
                    dark = true;
                }
                if sel.contains("prefers-color-scheme") && sel.contains("dark") {
                    dark = true;
                }
                stack.push(sel);
                buf.clear();
            }
            '}' => {
                flush(&mut buf, &stack, &mut out);
                stack.pop();
            }
            ';' => flush(&mut buf, &stack, &mut out),
            c => {
                if buf.len() < 512 {
                    buf.push(c);
                }
            }
        }
    }
    (out, dark)
}

// ------------------------------------------------------------------ JS/TS heuristics

fn ident_before(text: &str, end: usize) -> Option<String> {
    let b = text.as_bytes();
    let mut i = end;
    while i > 0 && (b[i - 1] as char).is_whitespace() {
        i -= 1;
    }
    let mut stop = i;
    if stop > 0 && matches!(b[stop - 1], b'\'' | b'"' | b'`') {
        stop -= 1;
        i = stop;
    }
    let mut start = i;
    while start > 0
        && ((b[start - 1] as char).is_ascii_alphanumeric()
            || matches!(b[start - 1], b'_' | b'-' | b'.'))
    {
        start -= 1;
    }
    (start < i).then(|| text[start..i].to_string())
}

const CONTAINER_KEYS: &[&str] = &[
    "theme",
    "extend",
    "colors",
    "color",
    "module",
    "exports",
    "default",
    "config",
    "palette",
    "tailwind",
    "export",
    "const",
    "export default",
];

/// `key: '#hex'` pairs with the object nesting folded into the name. Heuristic by design.
fn scan_color_pairs(text: &str) -> Vec<(String, String)> {
    let b = text.as_bytes();
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() && out.len() < 200 {
        match b[i] {
            b'{' => {
                let mut j = i;
                while j > 0 && (b[j - 1] as char).is_whitespace() {
                    j -= 1;
                }
                let key = if j > 0 && b[j - 1] == b':' {
                    ident_before(text, j - 1)
                } else {
                    None
                };
                stack.push(key);
            }
            b'}' => {
                stack.pop();
            }
            b':' => {
                let mut j = i + 1;
                while j < b.len() && (b[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < b.len() && matches!(b[j], b'\'' | b'"') {
                    let q = b[j];
                    if let Some(len) = b[j + 1..].iter().take(80).position(|c| *c == q) {
                        let value = &text[j + 1..j + 1 + len];
                        if is_color_value(value) {
                            if let Some(key) = ident_before(text, i) {
                                let mut parts: Vec<String> = stack
                                    .iter()
                                    .flatten()
                                    .filter(|k| !CONTAINER_KEYS.contains(&k.as_str()))
                                    .cloned()
                                    .collect();
                                if !(key == "DEFAULT" && !parts.is_empty()) {
                                    parts.push(key);
                                }
                                let n = parts.join("-");
                                out.push((n, value.to_string()));
                            }
                        }
                        i = j + 1 + len;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Inner text of the object that follows `key:` (bounded), e.g. `fontFamily: { ... }`.
fn object_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let at = text.find(key)?;
    let rest = &text[at + key.len()..];
    let open = rest.find('{')?;
    if open > 12 {
        return None;
    }
    let body = &rest[open + 1..];
    let mut depth = 1usize;
    for (i, c) in body.char_indices().take(2000) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&body[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn quoted_strings(s: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '\'' | '"') {
            let rest = &s[i + 1..];
            if let Some(end) = rest.find(c) {
                out.push(rest[..end].to_string());
                for _ in 0..rest[..end].chars().count() + 1 {
                    chars.next();
                }
                if out.len() >= max {
                    break;
                }
            }
        }
    }
    out
}

fn scan_js_tokens(text: &str, source: &str, tokens: &mut Tokens) {
    for (n, v) in scan_color_pairs(text) {
        if let (Some(n), Some(v)) = (clean_name(&n), clean_value(&v)) {
            tokens.push(
                Kind::Color,
                Token {
                    name: n,
                    value: v,
                    source: source.into(),
                },
            );
        }
    }
    if let Some(block) = object_after(text, "fontFamily") {
        for line in block.split(['\n', ']']) {
            if let Some((k, rest)) = line.split_once(':') {
                let k = k
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"' || c == ',' || c == '[');
                let fams = quoted_strings(rest, 4);
                if let (Some(n), false) = (clean_name(k), fams.is_empty()) {
                    if let Some(v) = clean_value(&fams.join(", ")) {
                        tokens.push(
                            Kind::Font,
                            Token {
                                name: format!("font-{n}"),
                                value: v,
                                source: source.into(),
                            },
                        );
                    }
                }
            }
        }
    }
    for (key, kind, prefix) in [
        ("borderRadius", Kind::Radius, "radius"),
        ("spacing", Kind::Spacing, "spacing"),
        ("boxShadow", Kind::Shadow, "shadow"),
    ] {
        if let Some(block) = object_after(text, key) {
            for line in block.split([',', '\n']) {
                if let Some((k, rest)) = line.split_once(':') {
                    let k = k.trim().trim_matches(|c| c == '\'' || c == '"');
                    let vals = quoted_strings(rest, 1);
                    if let (Some(n), Some(v)) =
                        (clean_name(k), vals.first().and_then(|v| clean_value(v)))
                    {
                        tokens.push(
                            kind,
                            Token {
                                name: format!("{prefix}-{n}"),
                                value: v,
                                source: source.into(),
                            },
                        );
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ token JSON

fn walk_token_json(
    v: &Value,
    path: &mut Vec<String>,
    depth: usize,
    budget: &mut usize,
    tokens: &mut Tokens,
    source: &str,
) {
    if depth > 8 || *budget == 0 {
        return;
    }
    let Value::Object(m) = v else { return };
    let leaf = m
        .get("$value")
        .or_else(|| m.get("value"))
        .and_then(Value::as_str);
    if let Some(val) = leaf {
        *budget -= 1;
        let name = path.join("-");
        let ty = m
            .get("$type")
            .or_else(|| m.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if let (Some(n), Some(val)) = (clean_name(&name), clean_value(val)) {
            let kind = match ty.as_str() {
                "color" => Some(Kind::Color),
                "fontfamily" | "fontfamilies" => Some(Kind::Font),
                "spacing" | "dimension" if name.to_ascii_lowercase().contains("radius") => {
                    Some(Kind::Radius)
                }
                "spacing" | "dimension" | "sizing" => Some(Kind::Spacing),
                "borderradius" => Some(Kind::Radius),
                "shadow" | "boxshadow" => Some(Kind::Shadow),
                _ => classify(&n, &val),
            };
            if let Some(k) = kind {
                tokens.push(
                    k,
                    Token {
                        name: n,
                        value: val,
                        source: source.into(),
                    },
                );
            }
        }
        return;
    }
    for (k, child) in m.iter().take(200) {
        if k.starts_with('$') {
            continue;
        }
        path.push(k.clone());
        walk_token_json(child, path, depth + 1, budget, tokens, source);
        path.pop();
    }
}

// ------------------------------------------------------------------ the walk

#[derive(Default)]
struct Found {
    package_json: Option<PathBuf>,
    tailwind_config: Option<PathBuf>,
    components_json: Option<PathBuf>,
    tsconfig: bool,
    css: Vec<(u32, PathBuf)>,
    token_json: Vec<PathBuf>,
    theme_js: Vec<PathBuf>,
    component_dirs: Vec<PathBuf>,
    css_modules: bool,
    app_router: bool,
    pages_router: bool,
    src_dir: bool,
    truncated: bool,
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn css_priority(name: &str) -> u32 {
    match name {
        "globals.css" | "global.css" => 0,
        "index.css" | "app.css" | "main.css" => 1,
        "theme.css" | "variables.css" | "tokens.css" | "styles.css" | "style.css" => 2,
        _ => 5,
    }
}

fn is_secretish(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.starts_with(".env")
        || l.contains("secret")
        || l.contains("credential")
        || l.ends_with(".pem")
        || l.ends_with(".key")
        || l.ends_with(".pfx")
}

fn walk(root: &Path) -> Found {
    let mut f = Found::default();
    let mut visited = 0usize;
    let mut queue: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            visited += 1;
            if visited > MAX_ENTRIES {
                f.truncated = true;
                return f;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue; // never follow links out of the project
            }
            if ft.is_dir() {
                if SKIP_DIRS.contains(&name.as_str())
                    || name.starts_with('.') && name != ".storybook"
                {
                    continue;
                }
                if depth == 0 && name == "src" {
                    f.src_dir = true;
                }
                if depth == 0 && name == "app" {
                    f.app_router = true;
                }
                if (depth == 0 || (depth == 1 && f.src_dir)) && name == "pages" {
                    f.pages_router = true;
                }
                if name == "components"
                    || (name == "ui" && path.parent().is_some_and(|p| p.ends_with("components")))
                {
                    f.component_dirs.push(path.clone());
                }
                if depth < MAX_DEPTH {
                    queue.push((path, depth + 1));
                }
                continue;
            }
            if !ft.is_file() || is_secretish(&name) {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            match lower.as_str() {
                "package.json" if depth == 0 => f.package_json = Some(path),
                "components.json" if depth == 0 => f.components_json = Some(path),
                "tsconfig.json" if depth == 0 => f.tsconfig = true,
                n if n.starts_with("tailwind.config.") && depth == 0 => {
                    f.tailwind_config = Some(path)
                }
                n if n.ends_with(".module.css") || n.ends_with(".module.scss") => {
                    f.css_modules = true
                }
                n if n.ends_with(".css") || n.ends_with(".scss") => {
                    f.css.push((css_priority(n) * 10 + depth as u32, path));
                }
                "tokens.json" | "design-tokens.json" | "theme.json" => f.token_json.push(path),
                n if n.ends_with(".tokens.json") => f.token_json.push(path),
                n => {
                    let stem = n.rsplit_once('.').map(|(s, _)| s).unwrap_or(n);
                    let ext = n.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
                    if matches!(
                        stem,
                        "theme" | "tokens" | "colors" | "palette" | "design-tokens"
                    ) && matches!(ext, "ts" | "js" | "mjs" | "cjs" | "tsx")
                    {
                        f.theme_js.push(path);
                    }
                }
            }
        }
    }
    f
}

struct Reader {
    files: u32,
    bytes: u64,
    truncated: bool,
}

impl Reader {
    fn read(&mut self, p: &Path) -> Option<String> {
        if self.bytes >= MAX_TOTAL_BYTES {
            self.truncated = true;
            return None;
        }
        let meta = std::fs::metadata(p).ok()?;
        if meta.len() > MAX_FILE_BYTES {
            self.truncated = true;
            return None;
        }
        let bytes = std::fs::read(p).ok()?;
        self.files += 1;
        self.bytes += bytes.len() as u64;
        String::from_utf8(bytes).ok()
    }
}

fn dep_version(deps: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    let v = deps.get(name)?.as_str()?;
    let v = v.trim_start_matches(['^', '~', '>', '=', ' ']);
    let major = v.split('.').next().unwrap_or("");
    (!major.is_empty() && major.chars().all(|c| c.is_ascii_digit())).then(|| major.to_string())
}

fn detect_packages(pkg: &Value, ctx: &mut ProjectContext) {
    let mut deps = serde_json::Map::new();
    for k in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(Value::Object(m)) = pkg.get(k) {
            for (n, v) in m {
                deps.insert(n.clone(), v.clone());
            }
        }
    }
    ctx.project_name = pkg
        .get("name")
        .and_then(Value::as_str)
        .and_then(|n| clean_name(n.trim_start_matches('@')));
    let has = |n: &str| deps.contains_key(n);
    let with_ver = |n: &str, label: &str| match dep_version(&deps, n) {
        Some(v) => format!("{label} {v}"),
        None => label.to_string(),
    };
    let push = |list: &mut Vec<String>, s: String| {
        if !list.contains(&s) {
            list.push(s)
        }
    };
    for (dep, label) in [
        ("next", "Next.js"),
        ("react", "React"),
        ("nuxt", "Nuxt"),
        ("vue", "Vue"),
        ("@angular/core", "Angular"),
        ("@sveltejs/kit", "SvelteKit"),
        ("svelte", "Svelte"),
        ("solid-js", "SolidJS"),
        ("astro", "Astro"),
        ("preact", "Preact"),
        ("@remix-run/react", "Remix"),
        ("vite", "Vite"),
        ("react-native", "React Native"),
    ] {
        if has(dep) {
            push(&mut ctx.frameworks, with_ver(dep, label));
        }
    }
    for (dep, label) in [
        ("tailwindcss", "Tailwind CSS"),
        ("@tailwindcss/vite", "Tailwind CSS 4 (vite plugin)"),
        ("styled-components", "styled-components"),
        ("@emotion/react", "Emotion"),
        ("sass", "Sass"),
        ("unocss", "UnoCSS"),
        ("@vanilla-extract/css", "vanilla-extract"),
        ("class-variance-authority", "class-variance-authority"),
    ] {
        if has(dep) {
            push(&mut ctx.styling, with_ver(dep, label));
        }
    }
    for (dep, label) in [
        ("@mui/material", "MUI"),
        ("@chakra-ui/react", "Chakra UI"),
        ("antd", "Ant Design"),
        ("@mantine/core", "Mantine"),
        ("vuetify", "Vuetify"),
        ("primevue", "PrimeVue"),
        ("element-plus", "Element Plus"),
        ("daisyui", "daisyUI"),
        ("@headlessui/react", "Headless UI"),
        ("@headlessui/vue", "Headless UI"),
        ("react-bootstrap", "React Bootstrap"),
        ("bootstrap", "Bootstrap"),
        ("@angular/material", "Angular Material"),
        ("@ionic/react", "Ionic"),
        ("@nextui-org/react", "NextUI"),
        ("@heroui/react", "HeroUI"),
    ] {
        if has(dep) {
            push(&mut ctx.ui_libraries, label.to_string());
        }
    }
    if deps.keys().any(|k| k.starts_with("@radix-ui/")) || has("radix-ui") {
        push(&mut ctx.ui_libraries, "Radix UI".into());
    }
    for (dep, label) in [
        ("lucide-react", "lucide-react"),
        ("lucide-vue-next", "lucide-vue-next"),
        ("@heroicons/react", "Heroicons"),
        ("react-icons", "react-icons"),
        ("@fortawesome/fontawesome-svg-core", "Font Awesome"),
        ("@tabler/icons-react", "Tabler Icons"),
    ] {
        if has(dep) {
            push(&mut ctx.icon_libraries, label.to_string());
        }
    }
    for dep in [
        "react-hook-form",
        "zod",
        "formik",
        "framer-motion",
        "react-router-dom",
        "vue-router",
        "@tanstack/react-query",
        "pinia",
        "zustand",
        "redux",
        "@reduxjs/toolkit",
        "storybook",
    ] {
        if has(dep) {
            push(&mut ctx.other_libraries, dep.to_string());
        }
    }
    if has("typescript") {
        ctx.typescript = true;
    }
}

fn listing(dir: &Path, limit: usize) -> Vec<String> {
    let mut names = BTreeSet::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten().take(400) {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = e.path();
            if ft.is_dir() {
                if depth < 1 && !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()) {
                    stack.push((path, depth + 1));
                }
            } else if let Some(stem) = component_stem(&e.file_name().to_string_lossy()) {
                names.insert(stem);
            }
        }
    }
    names.into_iter().take(limit).collect()
}

fn component_stem(file: &str) -> Option<String> {
    let (stem, ext) = file.rsplit_once('.')?;
    if !matches!(ext, "tsx" | "jsx" | "vue" | "svelte" | "astro") {
        return None;
    }
    let stem = stem.trim_end_matches(".stories").trim_end_matches(".test");
    clean_name(stem)
}

/// Inspect `project`. Always returns a context (possibly empty, with warnings) for an existing
/// directory; only a missing/non-directory path is an error.
pub fn inspect(project: &Path) -> Result<ProjectContext, InspectError> {
    if !project.is_dir() {
        return Err(InspectError::NotADirectory(project.display().to_string()));
    }
    let root = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let found = walk(&root);
    let mut rd = Reader {
        files: 0,
        bytes: 0,
        truncated: found.truncated,
    };
    let mut ctx = ProjectContext {
        schema_version: CONTEXT_SCHEMA,
        ..Default::default()
    };

    match found.package_json.as_ref().and_then(|p| rd.read(p)) {
        Some(text) => match serde_json::from_str::<Value>(&text) {
            Ok(pkg) => detect_packages(&pkg, &mut ctx),
            Err(_) => ctx.warnings.push("package.json is not valid JSON".into()),
        },
        None => ctx
            .warnings
            .push("no readable package.json at the project root".into()),
    }
    if found.tsconfig {
        ctx.typescript = true;
    }

    // Tokens, most authoritative first: token JSON, Tailwind config, theme files, CSS.
    for p in found.token_json.iter().take(3) {
        if let Some(text) = rd.read(p) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let mut budget = 400usize;
                walk_token_json(
                    &v,
                    &mut Vec::new(),
                    0,
                    &mut budget,
                    &mut ctx.tokens,
                    &rel(&root, p),
                );
            }
        }
    }
    if let Some(p) = &found.tailwind_config {
        if let Some(text) = rd.read(p) {
            let src = format!("{} (heuristic)", rel(&root, p));
            scan_js_tokens(&text, &src, &mut ctx.tokens);
            if text.contains("darkMode") {
                ctx.dark_mode = true;
            }
        }
    }
    for p in found.theme_js.iter().take(3) {
        if let Some(text) = rd.read(p) {
            scan_js_tokens(
                &text,
                &format!("{} (heuristic)", rel(&root, p)),
                &mut ctx.tokens,
            );
        }
    }
    let mut css = found.css.clone();
    css.sort();
    for (_, p) in css.iter().take(MAX_CSS_FILES) {
        let Some(text) = rd.read(p) else { continue };
        let (vars, dark) = parse_css(&text);
        ctx.dark_mode |= dark;
        let src = rel(&root, p);
        for (k, v) in vars {
            if let (Some(n), Some(v)) = (clean_name(&k), clean_value(&v)) {
                if let Some(kind) = classify(&n, &v) {
                    // Tailwind v4 `@theme` namespaces: `--color-primary` is the token `primary`.
                    let name = n.strip_prefix("color-").map(str::to_string).unwrap_or(n);
                    ctx.tokens.push(
                        kind,
                        Token {
                            name,
                            value: v,
                            source: src.clone(),
                        },
                    );
                }
            }
        }
    }
    if ctx.tokens.is_empty() {
        ctx.warnings.push(
            "no design tokens found (no CSS variables in :root, Tailwind colors, or token JSON)"
                .into(),
        );
    }

    // Components.
    let mut dirs: Vec<&PathBuf> = found.component_dirs.iter().collect();
    dirs.sort();
    for d in dirs.iter().take(8) {
        ctx.component_dirs.push(rel(&root, d));
    }
    let mut comps = BTreeSet::new();
    for d in dirs.iter().filter(|d| d.ends_with("ui")).take(2) {
        comps.extend(listing(d, MAX_COMPONENTS));
    }
    for d in dirs.iter().filter(|d| !d.ends_with("ui")).take(4) {
        if comps.len() >= MAX_COMPONENTS {
            break;
        }
        comps.extend(listing(d, 30));
    }
    ctx.components = comps.into_iter().take(MAX_COMPONENTS).collect();

    // shadcn/ui.
    let shadcn_cfg = found.components_json.is_some();
    let shadcn_dir = found.component_dirs.iter().any(|d| d.ends_with("ui"))
        && ctx.styling.iter().any(|s| s.starts_with("class-variance"));
    if shadcn_cfg || shadcn_dir {
        ctx.ui_libraries.push("shadcn/ui".into());
    }
    if found.css_modules {
        ctx.styling.push("CSS Modules".into());
    }
    if ctx.styling.is_empty() && ctx.ui_libraries.is_empty() && !found.css.is_empty() {
        ctx.styling.push("plain CSS".into());
    }

    // Conventions.
    if found.app_router && ctx.frameworks.iter().any(|f| f.starts_with("Next.js")) {
        ctx.conventions.push("Next.js App Router (app/)".into());
    } else if found.pages_router && ctx.frameworks.iter().any(|f| f.starts_with("Next.js")) {
        ctx.conventions.push("Next.js Pages Router (pages/)".into());
    }
    if found.src_dir {
        ctx.conventions.push("sources under src/".into());
    }
    if !ctx.components.is_empty() {
        let kebab = ctx.components.iter().filter(|c| c.contains('-')).count();
        ctx.conventions.push(
            if kebab * 2 > ctx.components.len() {
                "component files are kebab-case"
            } else {
                "component files are PascalCase or lowercase single words"
            }
            .into(),
        );
    }
    if ctx.typescript {
        ctx.conventions.push("TypeScript".into());
    }
    if let Some(p) = &root
        .join("tsconfig.json")
        .exists()
        .then(|| root.join("tsconfig.json"))
    {
        if rd.read(p).is_some_and(|t| t.contains("\"@/*\"")) {
            ctx.conventions
                .push("import alias @/* is configured".into());
        }
    }
    if ctx.frameworks.is_empty() && ctx.styling.is_empty() && ctx.ui_libraries.is_empty() {
        ctx.warnings
            .push("no frontend stack detected; designs will be framework-neutral".into());
    }

    ctx.files_read = rd.files;
    ctx.bytes_read = rd.bytes;
    ctx.truncated = rd.truncated;
    Ok(ctx)
}

impl ProjectContext {
    /// Compact JSON for the model prompt (bounded: tokens are capped per kind).
    pub fn prompt_json(&self) -> String {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(m) = v.as_object_mut() {
            for k in [
                "files_read",
                "bytes_read",
                "truncated",
                "warnings",
                "schema_version",
            ] {
                m.remove(k);
            }
        }
        v.to_string()
    }

    /// One short line for human output.
    pub fn headline(&self) -> String {
        let stack: Vec<String> = self
            .frameworks
            .iter()
            .chain(&self.styling)
            .chain(&self.ui_libraries)
            .cloned()
            .collect();
        format!(
            "{} | tokens: {} colors, {} fonts, {} spacing, {} radii | {} components",
            if stack.is_empty() {
                "no stack detected".to_string()
            } else {
                stack.join(", ")
            },
            self.tokens.colors.len(),
            self.tokens.fonts.len(),
            self.tokens.spacing.len(),
            self.tokens.radii.len(),
            self.components.len()
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);

    pub fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "kepler-genesis-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    pub fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    pub fn react_tailwind() -> PathBuf {
        let d = temp_dir("react");
        write(
            &d,
            "package.json",
            r#"{"name":"acme-app","dependencies":{"react":"^18.3.1","react-dom":"^18","lucide-react":"^0.4","class-variance-authority":"^0.7","@radix-ui/react-slot":"^1"},"devDependencies":{"vite":"^5","tailwindcss":"^3.4.1","typescript":"^5"}}"#,
        );
        write(
            &d,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        );
        write(&d, "components.json", "{}");
        write(&d, "tailwind.config.ts", "export default {\n darkMode: ['class'],\n theme: { extend: {\n  colors: { brand: { DEFAULT: '#4f46e5', dark: '#3730a3' }, surface: '#f8fafc', ink: \"#0f172a\" },\n  fontFamily: { sans: ['Inter', 'system-ui', 'sans-serif'], display: ['Sora', 'serif'] },\n  borderRadius: { lg: '12px', md: '8px' },\n } } }");
        write(&d, "src/index.css", "@tailwind base;\n:root { --background: 0 0% 100%; --radius: 0.5rem; --space-4: 16px; --shadow-card: 0 1px 2px rgba(0,0,0,.1); --bg-url: url(x.png); }\n.dark { --background: 222 47% 11%; }\n");
        write(
            &d,
            "src/components/ui/button.tsx",
            "export const Button = () => null;",
        );
        write(
            &d,
            "src/components/ui/input.tsx",
            "export const Input = () => null;",
        );
        write(&d, "src/components/ui/dialog-content.tsx", "x");
        write(&d, "src/components/Header.tsx", "x");
        write(&d, "node_modules/evil/package.json", r#"{"name":"evil"}"#);
        write(
            &d,
            "node_modules/evil/theme.ts",
            "colors: { evil: '#ff0000' }",
        );
        write(&d, ".env", "SECRET=hunter2");
        write(&d, ".env.local", "API_KEY=abc");
        d
    }

    pub fn vue_css_vars() -> PathBuf {
        let d = temp_dir("vue");
        write(
            &d,
            "package.json",
            r#"{"name":"vue-shop","dependencies":{"vue":"^3.4.0","vue-router":"^4","pinia":"^2"},"devDependencies":{"vite":"^5","sass":"^1"}}"#,
        );
        write(&d, "src/assets/main.css", ":root {\n  --color-primary: #0ea5e9;\n  --color-bg: #ffffff;\n  --font-body: 'Inter', sans-serif;\n  --radius-card: 6px;\n  --spacing-md: 12px;\n}\n@media (prefers-color-scheme: dark) { :root { --color-bg: #000; } }\n");
        write(
            &d,
            "src/components/ProductCard.vue",
            "<template></template>",
        );
        write(&d, "src/components/BaseButton.vue", "<template></template>");
        d
    }

    #[test]
    fn react_tailwind_is_detected_with_tokens_and_components() {
        let d = react_tailwind();
        let c = inspect(&d).unwrap();
        assert_eq!(c.project_name.as_deref(), Some("acme-app"));
        assert!(
            c.frameworks.contains(&"React 18".to_string()),
            "{:?}",
            c.frameworks
        );
        assert!(c.frameworks.iter().any(|f| f.starts_with("Vite")));
        assert!(c.styling.iter().any(|s| s.starts_with("Tailwind CSS 3")));
        assert!(c.ui_libraries.contains(&"shadcn/ui".to_string()));
        assert!(c.ui_libraries.contains(&"Radix UI".to_string()));
        assert!(c.icon_libraries.contains(&"lucide-react".to_string()));
        assert!(c.typescript && c.dark_mode);
        let color = |n: &str| {
            c.tokens
                .colors
                .iter()
                .find(|t| t.name == n)
                .map(|t| t.value.clone())
        };
        assert_eq!(
            color("brand").as_deref(),
            Some("#4f46e5"),
            "{:?}",
            c.tokens.colors
        );
        assert_eq!(color("brand-dark").as_deref(), Some("#3730a3"));
        assert_eq!(color("surface").as_deref(), Some("#f8fafc"));
        assert_eq!(color("ink").as_deref(), Some("#0f172a"));
        assert_eq!(color("background").as_deref(), Some("0 0% 100%"));
        assert!(c
            .tokens
            .fonts
            .iter()
            .any(|t| t.name == "font-sans" && t.value.starts_with("Inter")));
        assert!(c.tokens.radii.iter().any(|t| t.value == "12px"));
        assert!(c
            .tokens
            .radii
            .iter()
            .any(|t| t.name == "radius" && t.value == "0.5rem"));
        assert!(c.tokens.spacing.iter().any(|t| t.value == "16px"));
        assert!(c.tokens.shadows.iter().any(|t| t.name == "shadow-card"));
        // url(...) values never become tokens
        assert!(!c.tokens.colors.iter().any(|t| t.value.contains("url")));
        assert!(c.components.contains(&"button".to_string()));
        assert!(c.components.contains(&"dialog-content".to_string()));
        assert!(c.component_dirs.iter().any(|d| d == "src/components/ui"));
        assert!(c.conventions.iter().any(|x| x.contains("alias")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn secrets_and_node_modules_are_never_read() {
        let d = react_tailwind();
        let c = inspect(&d).unwrap();
        let all = serde_json::to_string(&c).unwrap();
        assert!(
            !all.contains("hunter2")
                && !all.contains("API_KEY")
                && !all.contains("evil")
                && !all.contains("#ff0000")
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn vue_with_css_variables() {
        let d = vue_css_vars();
        let c = inspect(&d).unwrap();
        assert!(c.frameworks.contains(&"Vue 3".to_string()));
        assert!(c.styling.iter().any(|s| s.starts_with("Sass")));
        assert!(c.dark_mode);
        assert!(c
            .tokens
            .colors
            .iter()
            .any(|t| t.name == "primary" && t.value == "#0ea5e9"));
        assert!(c.tokens.fonts.iter().any(|t| t.value.contains("Inter")));
        assert!(c.tokens.radii.iter().any(|t| t.value == "6px"));
        assert!(c.tokens.spacing.iter().any(|t| t.value == "12px"));
        assert_eq!(
            c.components,
            vec!["BaseButton".to_string(), "ProductCard".to_string()]
        );
        assert!(c.conventions.iter().any(|x| x.contains("PascalCase")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn empty_project_is_honest() {
        let d = temp_dir("empty");
        let c = inspect(&d).unwrap();
        assert!(c.frameworks.is_empty() && c.tokens.is_empty() && c.components.is_empty());
        assert!(c.warnings.iter().any(|w| w.contains("package.json")));
        assert!(c.warnings.iter().any(|w| w.contains("no design tokens")));
        assert!(c.warnings.iter().any(|w| w.contains("framework-neutral")));
        assert_eq!(c.files_read, 0);
        let _ = std::fs::remove_dir_all(d);
        assert!(inspect(Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn limits_hold_on_huge_and_hostile_inputs() {
        let d = temp_dir("hostile");
        write(&d, "package.json", "{not json");
        let big: String = (0..5000)
            .map(|i| format!("--c{i}: #{:06x};\n", i))
            .collect();
        write(&d, "src/big.css", &format!(":root {{ {big} }}"));
        write(
            &d,
            "src/index.css",
            &format!(
                ":root {{ --ok: #123456; --bad: red;}} /* {} */",
                "x".repeat(200_000)
            ),
        );
        let c = inspect(&d).unwrap();
        assert!(c.tokens.colors.len() <= MAX_TOKENS_PER_KIND);
        assert!(c.bytes_read <= MAX_TOTAL_BYTES);
        assert!(
            c.truncated,
            "files over the per-file limit are skipped and reported"
        );
        assert!(c.warnings.iter().any(|w| w.contains("not valid JSON")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn many_files_are_capped() {
        let d = temp_dir("many");
        for i in 0..(MAX_ENTRIES + 50) {
            write(&d, &format!("src/f{i}.txt"), "x");
        }
        let c = inspect(&d).unwrap();
        assert!(c.truncated);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn token_json_and_value_hygiene() {
        let d = temp_dir("tokjson");
        write(&d, "package.json", "{}");
        write(
            &d,
            "tokens.json",
            r##"{"color":{"primary":{"$value":"#112233","$type":"color"},"evil":{"$value":"red;} body{display:none","$type":"color"}},"radius":{"sm":{"value":"4px"}}}"##,
        );
        let c = inspect(&d).unwrap();
        assert!(c.tokens.colors.iter().any(|t| t.name == "color-primary"));
        assert!(!c.tokens.colors.iter().any(|t| t.value.contains('}')));
        assert!(c.tokens.radii.iter().any(|t| t.value == "4px"));
        assert!(clean_value("url(http://x)").is_none());
        assert!(clean_value("a;b").is_none());
        assert!(clean_name("a b").is_none());
        let _ = std::fs::remove_dir_all(d);
    }
}
