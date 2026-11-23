#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end tests of the functional-core modules through the real `kepler` binary.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Env {
    home: PathBuf,
    ipc: String,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("kepler-func-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            home,
            ipc: format!("func{}-{n}", std::process::id()),
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
        c.args(args)
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_IPC_NAME", &self.ipc)
            .env("KEPLER_IDLE_TIMEOUT_SECS", "5")
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
}

#[test]
fn sentinel_check_runs_the_project_build_records_findings_and_gates_with_guardian() {
    let env = Env::new();
    let proj = env.home.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("main.rs"),
        "fn main() {\n    let x: u32 = \"not a number\";\n}\n",
    )
    .unwrap();
    let repo = proj.to_str().unwrap();

    // A failing build: exit code 1, a finding tied to src/main.rs, stored in the database.
    let o = env.run(&[
        "sentinel",
        "check",
        "--repo",
        repo,
        "--cmd",
        "build=cargo build --offline",
        "--store",
        "--json",
    ]);
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(v["checks"][0]["status"]
        .as_str()
        .unwrap()
        .starts_with("failed"));
    let f = v["findings"].as_array().unwrap();
    assert!(
        f.iter()
            .any(|h| h["rule"] == "check.error" && h["path"] == "src/main.rs" && h["line"] == 2),
        "{f:?}"
    );
    assert!(v["stored_new"].as_u64().unwrap() >= 1);
    let o = env.run(&["sentinel", "list", "--json"]);
    let rows: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["rule"] == "check.error"));

    // A passing command.
    let o = env.run(&[
        "sentinel",
        "check",
        "--repo",
        repo,
        "--cmd",
        "v=cargo --version",
        "--json",
    ]);
    assert_eq!(o.status.code(), Some(0));

    // Guardian: a command reading a secret never runs, even with --yes.
    let o = env.run(&[
        "sentinel",
        "check",
        "--repo",
        repo,
        "--cmd",
        "x=cat .env",
        "--yes",
        "--json",
    ]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(
        v["checks"][0]["status"]
            .as_str()
            .unwrap()
            .contains("denied"),
        "{v}"
    );
    // approval-class commands need --yes
    let o = env.run(&[
        "sentinel",
        "check",
        "--repo",
        repo,
        "--cmd",
        "x=git reset --hard",
        "--json",
    ]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(
        v["checks"][0]["status"]
            .as_str()
            .unwrap()
            .contains("approval"),
        "{v}"
    );

    // Nothing requested -> clear error, not a silent success.
    let o = env.run(&["sentinel", "check", "--repo", repo]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("nothing to run"));
}

#[test]
fn sentinel_check_timeout_kills_the_command() {
    let env = Env::new();
    let dir = env.home.join("t");
    std::fs::create_dir_all(&dir).unwrap();
    let repo = dir.to_str().unwrap();
    let started = std::time::Instant::now();
    // `ping -n 30 127.0.0.1` ~ 29 s on Windows; `sleep 30` elsewhere.
    let cmd = if cfg!(windows) {
        "slow=ping -n 30 127.0.0.1"
    } else {
        "slow=sleep 30"
    };
    let o = env.run(&[
        "sentinel",
        "check",
        "--repo",
        repo,
        "--cmd",
        cmd,
        "--timeout-secs",
        "1",
        "--json",
    ]);
    assert!(started.elapsed().as_secs() < 15, "timeout was not enforced");
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["checks"][0]["status"], "timed_out", "{v}");
    assert_eq!(o.status.code(), Some(1));
}

fn http(port: u16, req: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let status = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, buf)
}

#[test]
fn ui_serves_real_data_from_the_binary_with_token_and_host_checks() {
    use std::io::BufRead;
    let env = Env::new();
    // Real data: a memory recorded through the CLI.
    let o = env.run(&["memory", "add", "--project", "p", "the ui test fact"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let mut child = env
        .cmd(&["ui", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    let port = v["port"].as_u64().unwrap() as u16;
    let token = v["token"].as_str().unwrap().to_string();
    assert!(v["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));
    assert_eq!(v["actions"], false);

    let host = format!("127.0.0.1:{port}");
    let (s, _) = http(
        port,
        &format!("GET /api/memories HTTP/1.1\r\nHost: {host}\r\n\r\n"),
    );
    assert_eq!(s, 401, "no token");
    let (s, _) = http(
        port,
        &format!("GET /api/memories HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer {token}\r\n\r\n"),
    );
    assert_eq!(s, 421, "rebinding host");
    let (s, body) = http(
        port,
        &format!(
            "GET /api/memories HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\r\n"
        ),
    );
    assert_eq!(s, 200);
    assert!(body.contains("the ui test fact"), "{body}");
    let (s, _) = http(
        port,
        &format!("POST /api/orbit/resolve HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"),
    );
    assert_eq!(s, 403, "actions are off unless --allow-actions");
    let _ = child.kill();
    let _ = child.wait();
}
