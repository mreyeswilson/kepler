//! End-to-end tests for `kepler genesis` (ADR-0027): the real `kepler` binary and a fake `claude`
//! (cargo example `fake_genesis_claude`, chosen with `KEPLER_CLAUDE_BIN`). Nothing here talks to
//! Anthropic and no browser is started (`--no-capture`, or an unreachable `KEPLER_VISION_BROWSER`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static N: AtomicU64 = AtomicU64::new(0);

fn example(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_BIN_EXE_kepler"));
    p.pop();
    p.push("examples");
    p.push(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(p.exists(), "build the example first: {}", p.display());
    p
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/genesis-react")
}

struct Env {
    home: PathBuf,
    log: PathBuf,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let home =
            std::env::temp_dir().join(format!("kepler-genesis-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Self {
            log: home.join("fake.log"),
            home,
        }
    }

    fn cmd(&self, args: &[&str], mode: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
        c.args(args)
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_CLAUDE_BIN", example("fake_genesis_claude"))
            .env("FAKE_CLAUDE_MODE", mode)
            .env("FAKE_CLAUDE_LOG", &self.log)
            .env(
                "KEPLER_VISION_BROWSER",
                self.home.join("no-such-browser.exe"),
            )
            .env_remove("KEPLER_VISION_WORKER")
            .env_remove("KEPLER_FORGE_CHILD")
            .env_remove("KEPLER_GENESIS_CHILD")
            .env_remove("KEPLER_GENESIS_MODEL")
            .env_remove("KEPLER_GENESIS_TIMEOUT_MS")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn run(&self, args: &[&str], mode: &str) -> Output {
        self.cmd(args, mode).output().unwrap()
    }

    fn log_lines(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn genesis_dir_entries(&self) -> usize {
        std::fs::read_dir(self.home.join("genesis"))
            .map(|r| r.count())
            .unwrap_or(0)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&o.stdout)))
}

fn design_args<'a>(project: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut a = vec![
        "genesis",
        "design",
        "pantalla de login",
        "--project",
        project,
        "--no-capture",
        "--json",
    ];
    a.extend_from_slice(extra);
    a
}

#[test]
fn inspect_is_deterministic_and_never_calls_the_model() {
    let env = Env::new();
    let p = fixture();
    let o = env.run(
        &[
            "genesis",
            "inspect",
            "--project",
            p.to_str().unwrap(),
            "--json",
        ],
        "fail",
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = stdout_json(&o);
    assert_eq!(v["project_name"], "acme-portal");
    assert!(v["frameworks"].to_string().contains("React 18"));
    assert!(v["styling"].to_string().contains("Tailwind CSS 3"));
    assert!(v["ui_libraries"].to_string().contains("shadcn/ui"));
    assert!(v["tokens"]["colors"].to_string().contains("#4f46e5"));
    assert!(v["tokens"]["fonts"].to_string().contains("Inter"));
    assert!(v["components"].to_string().contains("button"));
    assert!(env.log_lines().is_empty(), "inspect must not run claude");
    assert!(!env.home.join("genesis").exists(), "inspect writes nothing");
    let bad = env.run(&["genesis", "inspect", "--project", "/no/such/dir"], "ok");
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn design_writes_spec_and_safe_previews_and_passes_a_locked_down_child() {
    let env = Env::new();
    let p = fixture();
    let secret_brief = "pantalla de login con sk-ant-api03-QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ";
    let o = env.run(
        &[
            "genesis",
            "design",
            secret_brief,
            "--project",
            p.to_str().unwrap(),
            "--no-capture",
            "--json",
            "--candidates",
            "2",
        ],
        "ok",
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = stdout_json(&o);
    assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
    let dir = PathBuf::from(v["directory"].as_str().unwrap());
    assert!(
        dir.starts_with(env.home.join("genesis")),
        "designs live under KEPLER_HOME, not the repo"
    );
    assert!(dir.join("spec.json").is_file());
    for i in 1..=2 {
        let html = std::fs::read_to_string(dir.join(format!("preview-{i}.html"))).unwrap();
        assert!(
            html.contains("--primary:#4f46e5")
                && html.contains("Welcome back")
                && !html.contains("<script")
        );
        assert!(
            v["candidates"][i - 1]["preview_png"].is_null(),
            "no capture requested"
        );
    }
    assert!(
        !p.join("genesis").exists() && !p.join("spec.json").exists(),
        "the project is never written"
    );

    // what was launched, and what was sent
    let log = env.log_lines();
    assert_eq!(log.len(), 1);
    let args = log[0]["args"].to_string();
    for must in [
        "\"-p\"",
        "--tools",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "disableAllHooks",
        "--setting-sources",
    ] {
        assert!(args.contains(must), "{must} in {args}");
    }
    assert!(!args.contains("--bare") && !args.contains("dangerously"));
    assert_eq!(log[0]["guard"], "1");
    assert_eq!(log[0]["genesis_guard"], "1");
    assert_eq!(log[0]["forge_env"], "off");
    assert!(log[0]["cwd"].as_str().unwrap().ends_with("genesis-sandbox"));
    let stdin = log[0]["stdin"].as_str().unwrap();
    assert!(
        stdin.contains("<brief>") && stdin.contains("acme-portal") && stdin.contains("#4f46e5")
    );
    assert!(
        !stdin.contains("QQQQQQQQQQ"),
        "secrets are redacted before leaving the machine"
    );
    assert!(!std::fs::read_to_string(dir.join("spec.json"))
        .unwrap()
        .contains("QQQQQQQQQQ"));

    // second identical request is served from disk: no second model call
    let again = stdout_json(&env.run(
        &[
            "genesis",
            "design",
            secret_brief,
            "--project",
            p.to_str().unwrap(),
            "--no-capture",
            "--json",
            "--candidates",
            "2",
        ],
        "ok",
    ));
    assert_eq!(again["cached"], true);
    assert_eq!(again["id"], v["id"]);
    assert_eq!(env.log_lines().len(), 1);
}

#[test]
fn hostile_model_output_is_neutralized_or_rejected() {
    let env = Env::new();
    let p = fixture();
    let o = env.run(
        &design_args(p.to_str().unwrap(), &["--candidates", "2"]),
        "hostile",
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = stdout_json(&o);
    assert_eq!(
        v["candidates"].as_array().unwrap().len(),
        1,
        "the candidate with an unknown field is dropped"
    );
    assert_eq!(v["rejected_candidates"].as_array().unwrap().len(), 1);
    assert!(!v["rejected_candidates"].to_string().contains("rm -rf"));
    let dir = PathBuf::from(v["directory"].as_str().unwrap());
    let html = std::fs::read_to_string(dir.join("preview-1.html"))
        .unwrap()
        .to_lowercase();
    assert!(
        !html.contains("<script") && !html.contains("<img") && !html.contains("onerror=alert(1)>")
    );
    let spec = std::fs::read_to_string(dir.join("spec.json")).unwrap();
    assert!(!spec.contains("<script>") && !spec.contains("<img"));
}

#[test]
fn model_failures_are_clean_errors_and_leave_no_files() {
    let env = Env::new();
    let p = fixture();
    for (mode, needle) in [
        ("garbage", "rejected"),
        ("fail", "model call failed"),
        ("huge", "rejected"),
    ] {
        let o = env.run(&design_args(p.to_str().unwrap(), &[]), mode);
        assert_eq!(o.status.code(), Some(2), "{mode}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(needle), "{mode}: {err}");
        assert!(!err.contains("panicked"), "{err}");
    }
    assert_eq!(env.genesis_dir_entries(), 0);
    // missing binary
    let mut c = env.cmd(&design_args(p.to_str().unwrap(), &[]), "ok");
    c.env("KEPLER_CLAUDE_BIN", env.home.join("no-such-claude.exe"));
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unavailable"));
}

#[test]
fn a_slow_model_is_cut_off_at_the_deadline() {
    let env = Env::new();
    let p = fixture();
    let mut c = env.cmd(&design_args(p.to_str().unwrap(), &[]), "slow");
    c.env("KEPLER_GENESIS_TIMEOUT_MS", "1500");
    let t = Instant::now();
    let o = c.output().unwrap();
    assert!(t.elapsed().as_secs() < 45, "took {:?}", t.elapsed());
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("did not answer within 1500 ms"));
    assert_eq!(env.genesis_dir_entries(), 0);
}

#[test]
fn the_child_cannot_reenter_kepler() {
    let env = Env::new();
    let p = fixture();
    let mut c = env.cmd(&design_args(p.to_str().unwrap(), &[]), "recurse");
    c.env("FAKE_CLAUDE_HOOK_EXE", env!("CARGO_BIN_EXE_kepler"));
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let log = env.log_lines();
    let runs = log.iter().filter(|l| l.get("args").is_some()).count();
    assert_eq!(runs, 1, "exactly one claude run: {log:?}");
    let nested = log
        .iter()
        .find(|l| l.get("nested_hook_exit").is_some())
        .expect("nested log");
    assert_eq!(nested["nested_hook_exit"], 0);
    assert_eq!(
        nested["nested_hook_stdout_bytes"], 0,
        "the hook must stay silent inside the child"
    );
    assert_ne!(
        nested["nested_genesis_exit"], 0,
        "genesis refuses to run inside a child"
    );
    assert_eq!(nested["nested_genesis_exit"], 2);
}

#[test]
fn list_show_and_purge_require_confirmation() {
    let env = Env::new();
    let p = fixture();
    let v = stdout_json(&env.run(&design_args(p.to_str().unwrap(), &[]), "ok"));
    let id = v["id"].as_str().unwrap().to_string();
    let l = stdout_json(&env.run(&["genesis", "list", "--json"], "ok"));
    assert_eq!(l["designs"][0]["id"], id.as_str());
    let s = stdout_json(&env.run(&["genesis", "show", &id, "--candidate", "1"], "ok"));
    assert_eq!(s["spec"]["screens"][0]["layout"]["kind"], "centered_card");
    assert_eq!(s["schema"], "kepler.genesis.spec/1");
    assert_eq!(
        env.run(&["genesis", "show", "../../etc", "--json"], "ok")
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        env.run(&["genesis", "show", &id, "--candidate", "3"], "ok")
            .status
            .code(),
        Some(2)
    );
    // purge without --yes refuses and keeps everything
    let o = env.run(&["genesis", "purge"], "ok");
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--yes"));
    assert_eq!(env.genesis_dir_entries(), 1);
    let o = env.run(&["genesis", "purge", &id, "--yes"], "ok");
    assert!(o.status.success());
    assert_eq!(env.genesis_dir_entries(), 0);
    assert_eq!(
        env.run(&["genesis", "purge", "--yes"], "ok").status.code(),
        Some(0)
    );
}

#[test]
fn other_commands_never_touch_genesis() {
    let env = Env::new();
    for args in [vec!["version"], vec!["inspect", "--limit", "1"]] {
        let _ = env.run(&args, "fail");
    }
    assert!(!env.home.join("genesis").exists());
    assert!(env.log_lines().is_empty());
}

#[test]
fn mcp_exposes_the_genesis_tools_and_runs_design() {
    let env = Env::new();
    let p = fixture();
    let mut c = env.cmd(&["mcp", "serve"], "ok");
    c.stdin(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |v: Value| -> Value {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let r = ask(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    let names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "kepler_genesis_inspect",
        "kepler_genesis_design",
        "kepler_genesis_list",
        "kepler_genesis_show",
    ] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    let r = ask(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"kepler_genesis_inspect","arguments":{"project": p.to_str().unwrap()}}}),
    );
    assert_eq!(r["result"]["isError"], false);
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("acme-portal"));
    let r = ask(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kepler_genesis_design","arguments":{"brief":"login","project": p.to_str().unwrap(),"candidates":1}}}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    let summary: Value = serde_json::from_str(text).unwrap();
    assert_eq!(summary["candidates"].as_array().unwrap().len(), 1);
    assert!(summary["candidates"][0]["preview_html"]
        .as_str()
        .unwrap()
        .ends_with("preview-1.html"));
    assert!(
        !text.contains("<html"),
        "the response carries paths, not the HTML"
    );
    let id = summary["id"].as_str().unwrap();
    let r = ask(
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"kepler_genesis_show","arguments":{"id": id}}}),
    );
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("implementation_plan"));
    let r = ask(
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"kepler_genesis_show","arguments":{"id": "nope"}}}),
    );
    assert_eq!(r["result"]["isError"], true);
    drop(stdin);
    let _ = child.wait();
}
