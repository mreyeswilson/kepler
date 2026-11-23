//! Minimal, bounded parser for `git diff --unified=0` output (added and removed lines only).

pub const MAX_FILES: usize = 500;
pub const MAX_DIFF_LINES: usize = 50_000;
pub const MAX_LINE_CHARS: usize = 400;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// (line number in the new file, text clipped to [`MAX_LINE_CHARS`]).
    pub added: Vec<(u32, String)>,
    pub removed: Vec<String>,
}

fn unquote(p: &str) -> &str {
    p.trim().trim_matches('"')
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_LINE_CHARS).collect()
}

/// Parse the new-file start line out of `@@ -a,b +c,d @@`.
fn hunk_start(line: &str) -> Option<u32> {
    let plus = line.split_whitespace().find(|t| t.starts_with('+'))?;
    plus.trim_start_matches('+')
        .split(',')
        .next()?
        .parse::<u32>()
        .ok()
}

/// Files appear in diff order; deleted files and binary files yield no lines. Input beyond
/// [`MAX_DIFF_LINES`] lines or [`MAX_FILES`] files is ignored.
pub fn parse_unified(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut new_line: u32 = 0;
    let mut skipping = false;
    for (n, line) in text.lines().enumerate() {
        if n >= MAX_DIFF_LINES {
            break;
        }
        if line.starts_with("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            if files.len() >= MAX_FILES {
                return files;
            }
            skipping = true; // until we see the `+++` line with a real path
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if p.trim() == "/dev/null" {
                skipping = true;
            } else {
                let path = unquote(p.strip_prefix("b/").unwrap_or(p)).to_string();
                cur = Some(FileDiff {
                    path,
                    ..FileDiff::default()
                });
                skipping = false;
            }
        } else if line.starts_with("--- ") || skipping {
            // old-file header or content of a skipped file
        } else if line.starts_with("@@") {
            new_line = hunk_start(line).unwrap_or(0);
        } else if let Some(f) = cur.as_mut() {
            if let Some(t) = line.strip_prefix('+') {
                f.added.push((new_line, clip(t)));
                new_line = new_line.saturating_add(1);
            } else if let Some(t) = line.strip_prefix('-') {
                f.removed.push(clip(t));
            } else if line.starts_with(' ') {
                new_line = new_line.saturating_add(1);
            }
        }
    }
    if let Some(f) = cur.take() {
        files.push(f);
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3,0 +4,2 @@ fn x\n+let a = 1;\n+unsafe { b() }\n@@ -10 +12 @@\n-old\n+new\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\ndiff --git a/docs/b.md b/docs/b.md\n--- a/docs/b.md\n+++ b/docs/b.md\n@@ -0,0 +1 @@\n+hello\n";

    #[test]
    fn parses_added_lines_with_numbers_and_skips_deleted_and_binary() {
        let d = parse_unified(DIFF);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].path, "src/a.rs");
        assert_eq!(
            d[0].added,
            vec![
                (4, "let a = 1;".into()),
                (5, "unsafe { b() }".into()),
                (12, "new".into())
            ]
        );
        assert_eq!(d[0].removed, vec!["old".to_string()]);
        assert_eq!(d[1].path, "docs/b.md");
        assert_eq!(d[1].added, vec![(1, "hello".into())]);
    }

    #[test]
    fn long_lines_and_huge_inputs_are_bounded() {
        let long = format!(
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1 @@\n+{}\n",
            "z".repeat(5000)
        );
        assert_eq!(
            parse_unified(&long)[0].added[0].1.chars().count(),
            MAX_LINE_CHARS
        );
        let mut many = String::new();
        for i in 0..(MAX_FILES + 50) {
            many.push_str(&format!(
                "diff --git a/f{i} b/f{i}\n--- a/f{i}\n+++ b/f{i}\n@@ -0,0 +1 @@\n+x\n"
            ));
        }
        assert!(parse_unified(&many).len() <= MAX_FILES);
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(parse_unified("").is_empty());
        assert!(parse_unified("+++\n@@@@\n+++ b/\n+x").len() <= 1);
    }
}
