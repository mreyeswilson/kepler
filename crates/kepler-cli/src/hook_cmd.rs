//! `kepler hook claude-code <event>`: Claude Code command-hook adapter (ADR-0020).
//!
//! Contract: JSON from Claude Code on stdin, optional JSON on stdout, **always exit 0**.
//! Anything that goes wrong on Kepler's side (bad JSON, core down, timeout, unknown event)
//! prints nothing, which Claude Code treats as "no opinion". The only outputs that change
//! behavior are explicit Guardian verdicts (`deny` / `require_approval`) on PreToolUse.

use crate::hook_claude::{build_envelope, render_output, Ctx, HookEvent};
use clap::{Args, Subcommand};
use kepler_core::emit::emit_in_process;
use kepler_core::{emit_one_shot, EmitOptions, EmitOutcome};
use kepler_ipc::Endpoint;
use serde_json::{json, Value};
use std::io::Read;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Largest stdin accepted (tool responses can be big; only a bounded excerpt is forwarded).
const MAX_STDIN_BYTES: u64 = 8 * 1024 * 1024;
/// Total budget for one hook invocation talking to the core (`KEPLER_HOOK_TIMEOUT_MS`).
const DEFAULT_TIMEOUT_MS: u64 = 2000;

#[derive(Subcommand)]
pub enum HookCmd {
    /// Claude Code hooks (reads the hook JSON on stdin; prints hook output JSON, if any).
    ClaudeCode(ClaudeCodeArgs),
}

#[derive(Args)]
pub struct ClaudeCodeArgs {
    /// UserPromptSubmit | PreToolUse | PostToolUse | Stop | SessionStart | SessionEnd.
    /// Unknown names are ignored (exit 0) so a newer settings file never breaks Claude Code.
    pub event: Option<String>,
    /// Print the `hooks` settings block to paste into Claude Code settings.json, then exit.
    #[arg(long)]
    pub print_settings: bool,
    /// With --print-settings: emit single-string shell commands (bash-style quoting) instead of
    /// the exec form (`command` + `args`). Use it if your Claude Code predates `args`.
    #[arg(long, requires = "print_settings")]
    pub shell_form: bool,
    /// Do not start a daemon when none is running.
    #[arg(long)]
    pub no_autostart: bool,
}

pub fn run(cmd: HookCmd) -> ExitCode {
    let HookCmd::ClaudeCode(args) = cmd;
    if args.print_settings {
        return print_settings(args.shell_form);
    }
    // Panics must not turn into a non-zero exit that Claude Code reports as a hook error.
    let r = std::panic::catch_unwind(|| claude_code(&args));
    if let Ok(Some(out)) = r {
        println!("{out}");
    }
    ExitCode::SUCCESS
}

fn claude_code(args: &ClaudeCodeArgs) -> Option<String> {
    let ev = HookEvent::parse(args.event.as_deref()?)?;
    let mut raw = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_STDIN_BYTES + 1)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() as u64 > MAX_STDIN_BYTES {
        return None;
    }
    let input = match parse_stdin(&raw) {
        Ok(v) => v,
        Err(why) => {
            // Never the payload itself. Claude Code shows hook stderr only in verbose mode.
            eprintln!("kepler: hook {}: stdin is not valid JSON ({}); ignored (set KEPLER_HOOK_DEBUG=1 for details)", ev.name(), why.short);
            if std::env::var_os("KEPLER_HOOK_DEBUG").is_some_and(|v| v == "1") {
                eprintln!("kepler: debug: {}", why.detail);
            }
            return None;
        }
    };
    let outcome = process(ev, &input, args.no_autostart, &Endpoint::from_env())?;
    render_output(ev, &outcome)
}

pub struct StdinError {
    pub short: &'static str,
    pub detail: String,
}

/// Decode the hook's stdin leniently: UTF-8 (with or without BOM), UTF-16 LE/BE (with BOM, or
/// detected from NUL bytes, as Windows PowerShell 5.1 pipes can produce), surrounding whitespace
/// ignored. Returns the parsed JSON.
pub fn parse_stdin(raw: &[u8]) -> Result<Value, StdinError> {
    let (text, enc) = decode_text(raw).map_err(|d| StdinError {
        short: "undecodable text",
        detail: d,
    })?;
    let t = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if t.is_empty() {
        return Err(StdinError {
            short: "empty",
            detail: format!("empty stdin ({} bytes, {enc})", raw.len()),
        });
    }
    serde_json::from_str(t).map_err(|e| StdinError {
        short: "parse error",
        detail: format!(
            "{e}; {} bytes, encoding {enc}, first bytes {:02x?}",
            raw.len(),
            &raw[..raw.len().min(4)]
        ),
    })
}

fn decode_text(raw: &[u8]) -> Result<(String, &'static str), String> {
    let utf16 = |b: &[u8], le: bool| -> Result<String, String> {
        let units = b.as_chunks::<2>().0.iter().map(|c| {
            if le {
                u16::from_le_bytes(*c)
            } else {
                u16::from_be_bytes(*c)
            }
        });
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|e| format!("invalid UTF-16: {e}"))
    };
    if let Some(b) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(b.to_vec())
            .map(|s| (s, "utf-8+bom"))
            .map_err(|e| e.to_string());
    }
    if let Some(b) = raw.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(b, true).map(|s| (s, "utf-16le+bom"));
    }
    if let Some(b) = raw.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(b, false).map(|s| (s, "utf-16be+bom"));
    }
    // No BOM: JSON starts with an ASCII char, so a NUL in the first two bytes betrays UTF-16.
    if raw.len() >= 2 && raw[0] != 0 && raw[1] == 0 {
        return utf16(raw, true).map(|s| (s, "utf-16le"));
    }
    if raw.len() >= 2 && raw[0] == 0 && raw[1] != 0 {
        return utf16(raw, false).map(|s| (s, "utf-16be"));
    }
    String::from_utf8(raw.to_vec())
        .map(|s| (s, "utf-8"))
        .map_err(|e| e.to_string())
}

fn timeout_from_env() -> Duration {
    let ms = std::env::var("KEPLER_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(ms.clamp(1, 10_000))
}

/// Parse, map and send. `None` = nothing to do (malformed input, unmappable payload, no runtime).
fn process(
    ev: HookEvent,
    input: &Value,
    no_autostart: bool,
    endpoint: &Endpoint,
) -> Option<EmitOutcome> {
    let env = build_envelope(ev, input, &new_ctx())?;
    let total = timeout_from_env();
    // Guardian preflight is a security boundary: keep a copy to evaluate locally if the daemon
    // cannot answer in time (cold start, load, crash).
    let fallback = (ev == HookEvent::PreToolUse).then(|| env.clone());
    let rt = crate::runtime().ok()?;
    let outcome = rt.block_on(async {
        let spawn = crate::spawn_daemon;
        let opts = EmitOptions {
            spawn_daemon: if no_autostart { None } else { Some(&spawn) },
            timeout: total,
            autostart_wait: total.min(Duration::from_millis(1500)),
        };
        deadline(total, emit_one_shot(endpoint, env, &opts)).await
    });
    if let (Some(e), EmitOutcome::Passthrough { .. }) = (fallback, &outcome) {
        // Same engine and Guardian rules as the daemon, minus storage-backed modules.
        return Some(emit_in_process(e));
    }
    Some(outcome)
}

/// Hard wall-clock bound over the whole attempt (connect, autostart, request).
pub async fn deadline(
    total: Duration,
    fut: impl std::future::Future<Output = EmitOutcome>,
) -> EmitOutcome {
    match tokio::time::timeout(total, fut).await {
        Ok(o) => o,
        Err(_) => EmitOutcome::Passthrough {
            reason: "hook_timeout".into(),
        },
    }
}

fn new_ctx() -> Ctx {
    static N: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ctx {
        timestamp: rfc3339_utc(now.as_secs(), now.subsec_millis()),
        nonce: format!(
            "{:x}-{:x}-{:x}",
            now.as_nanos(),
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ),
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` from Unix time (civil-from-days, no date dependency).
pub fn rfc3339_utc(secs: u64, millis: u32) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// (event, matcher, timeout seconds). Short timeouts: Kepler's own budget is 2 s.
pub const HOOK_TABLE: [(&str, Option<&str>, u32); 6] = [
    ("UserPromptSubmit", None, 5),
    (
        "PreToolUse",
        Some("Bash|Write|Edit|MultiEdit|NotebookEdit|Read"),
        5,
    ),
    ("PostToolUse", Some("Bash|Read|Grep"), 5),
    ("Stop", None, 3),
    ("SessionStart", None, 3),
    ("SessionEnd", None, 3),
];

/// The `hooks` block, ready to paste. `exe` is absolute; forward slashes are used on Windows
/// because they work in both Git Bash and PowerShell and need no JSON-escaping.
pub fn settings_block(exe: &str, shell_form: bool) -> Value {
    let mut hooks = serde_json::Map::new();
    for (event, matcher, timeout) in HOOK_TABLE {
        let handler = if shell_form {
            json!({
                "type": "command",
                "command": format!("'{}' hook claude-code {event}", exe.replace('\'', "'\\''")),
                "timeout": timeout,
            })
        } else {
            json!({
                "type": "command",
                "command": exe,
                "args": ["hook", "claude-code", event],
                "timeout": timeout,
            })
        };
        let mut group = serde_json::Map::new();
        if let Some(m) = matcher {
            group.insert("matcher".into(), json!(m));
        }
        group.insert("hooks".into(), json!([handler]));
        hooks.insert(event.into(), json!([Value::Object(group)]));
    }
    json!({ "hooks": Value::Object(hooks) })
}

fn print_settings(shell_form: bool) -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(e) => {
            eprintln!("kepler: cannot determine the executable path: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&settings_block(&exe, shell_form)).unwrap_or_default()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_values() {
        assert_eq!(rfc3339_utc(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_utc(1_709_164_800, 7), "2024-02-29T00:00:00.007Z");
        assert_eq!(rfc3339_utc(1_767_323_045, 0), "2026-01-02T03:04:05.000Z");
        assert!(kepler_events::is_valid_rfc3339(&new_ctx().timestamp));
    }

    #[test]
    fn settings_block_shape_exec_and_shell() {
        let v = settings_block("C:/Tools/kepler.exe", false);
        for (event, matcher, _) in HOOK_TABLE {
            let g = &v["hooks"][event][0];
            assert_eq!(g.get("matcher").and_then(Value::as_str), matcher);
            let h = &g["hooks"][0];
            assert_eq!(h["type"], "command");
            assert_eq!(h["command"], "C:/Tools/kepler.exe");
            assert_eq!(h["args"], json!(["hook", "claude-code", event]));
            assert!(h["timeout"].as_u64().unwrap() <= 5);
        }
        let s = settings_block("/opt/it's/kepler", true);
        let c = s["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(c, "'/opt/it'\\''s/kepler' hook claude-code Stop");
        assert!(s["hooks"]["Stop"][0]["hooks"][0].get("args").is_none());
    }

    #[test]
    fn deadline_turns_a_hung_core_into_passthrough() {
        let rt = crate::runtime().unwrap();
        let o = rt.block_on(deadline(Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            EmitOutcome::Passthrough {
                reason: "late".into(),
            }
        }));
        assert_eq!(
            o,
            EmitOutcome::Passthrough {
                reason: "hook_timeout".into()
            }
        );
    }
}

#[cfg(test)]
mod stdin_tests {
    use super::*;

    const J: &str = "{\"a\":1,\"t\":\"caf\u{e9} \u{1f600}\"}";

    fn utf16(s: &str, le: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend(if le { [0xFF, 0xFE] } else { [0xFE, 0xFF] });
        }
        for u in s.encode_utf16() {
            out.extend(if le { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        out
    }

    #[test]
    fn accepts_bom_utf16_crlf_and_padding() {
        let want: Value = serde_json::from_str(J).unwrap();
        let mut cases: Vec<Vec<u8>> = vec![
            J.as_bytes().to_vec(),
            [&[0xEF, 0xBB, 0xBF][..], J.as_bytes()].concat(),
            [J.as_bytes(), b"\r\n"].concat(),
            [&[0xEF, 0xBB, 0xBF][..], J.as_bytes(), b"\r\n"].concat(),
            [b"  \r\n", J.as_bytes(), b"\n\n "].concat(),
        ];
        for (le, bom) in [(true, true), (false, true), (true, false), (false, false)] {
            cases.push(utf16(J, le, bom));
            cases.push(utf16(&format!("{J}\r\n"), le, bom));
        }
        for (i, c) in cases.iter().enumerate() {
            assert_eq!(
                parse_stdin(c).unwrap_or_else(|e| panic!("case {i}: {}", e.detail)),
                want,
                "case {i}"
            );
        }
    }

    #[test]
    fn rejects_garbage_without_leaking_payload() {
        for raw in [
            &b""[..],
            b"   \r\n",
            b"not json",
            b"{",
            &[0xff, 0xfe, 0xfd],
            &[0xC3, 0x28],
        ] {
            assert!(parse_stdin(raw).is_err());
        }
        let e = parse_stdin(b"{\"secret\": nope}").unwrap_err();
        assert!(!e.short.contains("secret"));
        assert!(!e.detail.contains("secret"), "{}", e.detail);
    }
}
