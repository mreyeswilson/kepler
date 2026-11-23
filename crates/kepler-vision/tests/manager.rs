#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Worker manager against a fake worker. The fake is this very test binary re-executed with
//! `KEPLER_FAKE_VISION=<mode>` and one libtest filter, so no extra binary is built and no browser,
//! Node or network is involved.

use kepler_vision::{CheckOptions, Viewport, VisionError, WorkerCommand, WorkerManager};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

const CANNED: &str = r#"{"url":"http://localhost:3000/","title":"Demo","viewports":[{"name":"mobile","width":375,"height":812,"scroll_width":420,"client_width":375,"horizontal_overflow":true,"console_errors":["boom"],"page_errors":[],"failed_requests":[],"issues":[{"rule":"a11y.img_alt","count":2,"samples":["img.hero"]}]}]}"#;

/// Entry point of the fake worker; a no-op when the test binary runs normally.
#[test]
fn fake_worker_entry() {
    let Ok(mode) = std::env::var("KEPLER_FAKE_VISION") else {
        return;
    };
    if let Ok(path) = std::env::var("KEPLER_VISION_PROBE_FILE") {
        let leaked = std::env::var("KEPLER_TEST_SECRET_TOKEN").is_ok();
        std::fs::write(path, format!("leaked={leaked}")).unwrap();
    }
    let mut out = std::io::stdout();
    // libtest prints "test name ... " without a newline: end that line so frames start clean.
    writeln!(out).unwrap();
    out.flush().unwrap();
    if mode == "spam" {
        for i in 0..300 {
            writeln!(out, "log line {i}").unwrap();
        }
        out.flush().unwrap();
    }
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        let id = req["id"].as_str().unwrap().to_string();
        let ok = |result: &str| format!(r#"{{"v":1,"id":"{id}","ok":true,"result":{result}}}"#);
        match req["op"].as_str().unwrap() {
            "hello" => {
                let pw = mode != "noplaywright";
                writeln!(out, "{}", ok(&format!(r#"{{"worker":"fake","playwright":{pw}}}"#))).unwrap();
            }
            "shutdown" => {
                writeln!(out, "{}", ok(r#"{"bye":true}"#)).unwrap();
                out.flush().unwrap();
                std::process::exit(0);
            }
            "check" => match mode.as_str() {
                "hang" => loop {
                    std::thread::sleep(Duration::from_secs(60));
                },
                "crash" => std::process::exit(3),
                "huge" => {
                    writeln!(out, "{{\"id\":\"{id}\",\"pad\":\"{}\"}}", "x".repeat(400 * 1024)).unwrap();
                }
                "error" => writeln!(
                    out,
                    r#"{{"v":1,"id":"{id}","ok":false,"error":{{"code":"browser_unavailable","message":"no chromium"}}}}"#
                )
                .unwrap(),
                "junk" => {
                    writeln!(out, "some stray log line").unwrap();
                    writeln!(out, r#"{{"v":1,"id":"other","ok":true,"result":{{}}}}"#).unwrap();
                    writeln!(out, "{}", ok(CANNED)).unwrap();
                }
                "echo_request" => {
                    // Report what the manager sent, so tests can assert on the request itself.
                    let seen = serde_json::json!({
                        "url": req["url"], "timeout_ms": req["timeout_ms"],
                        "viewports": req["viewports"].as_array().map_or(0, Vec::len),
                        "screenshot_dir": req["screenshot_dir"],
                    });
                    writeln!(out, "{}", ok(&format!(
                        r#"{{"url":"http://localhost/","title":{},"viewports":[]}}"#,
                        serde_json::Value::String(seen.to_string())
                    ))).unwrap();
                }
                _ => writeln!(out, "{}", ok(CANNED)).unwrap(),
            },
            _ => {}
        }
        out.flush().unwrap();
    }
    std::process::exit(0);
}

fn fake(mode: &str) -> WorkerCommand {
    let mut c = WorkerCommand::new(
        std::env::current_exe().unwrap(),
        [
            "--exact",
            "fake_worker_entry",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
    );
    c.env.push(("KEPLER_FAKE_VISION".into(), mode.into()));
    c
}

fn mgr(mode: &str) -> WorkerManager {
    WorkerManager::new(fake(mode), Vec::new()).with_grace(Duration::from_millis(300))
}

fn vp() -> Vec<Viewport> {
    vec![Viewport::new("mobile", 375, 812)]
}

fn opts(secs: u64) -> CheckOptions {
    CheckOptions {
        timeout: Duration::from_secs(secs),
        screenshot_dir: None,
    }
}

#[test]
fn nothing_starts_until_a_valid_check_is_requested() {
    let mut m = WorkerManager::new(
        WorkerCommand::new("definitely-not-a-real-program-xyz", vec![]),
        Vec::new(),
    );
    assert!(!m.is_running());
    assert_eq!(m.spawn_count(), 0);
    // Refused URLs and bad requests are rejected before any spawn attempt.
    assert!(matches!(
        m.check("https://example.com/", &vp(), &opts(2)),
        Err(VisionError::UrlNotAllowed(_))
    ));
    assert!(matches!(
        m.check("http://localhost/", &[], &opts(2)),
        Err(VisionError::BadRequest(_))
    ));
    assert!(matches!(
        m.check("http://localhost/", &vp(), &opts(500)),
        Err(VisionError::BadRequest(_))
    ));
    assert_eq!(m.spawn_count(), 0);
    // A valid request reaches the spawn and fails cleanly when the program does not exist.
    assert!(matches!(
        m.check("http://localhost/", &vp(), &opts(2)),
        Err(VisionError::Spawn(_))
    ));
    assert!(!m.is_running());
}

#[test]
fn happy_path_returns_results_and_reuses_one_worker() {
    let mut m = mgr("ok");
    let r = m.check("http://localhost:3000/", &vp(), &opts(5)).unwrap();
    assert_eq!(r.title, "Demo");
    assert!(r.viewports[0].horizontal_overflow);
    assert_eq!(r.viewports[0].issues[0].rule, "a11y.img_alt");
    assert!(m.is_running());
    m.check("http://localhost:3000/", &vp(), &opts(5)).unwrap();
    assert_eq!(m.spawn_count(), 1, "the worker is reused between checks");
    m.shutdown();
    assert!(!m.is_running());
}

#[test]
fn request_carries_url_timeout_viewports_and_screenshot_dir() {
    let mut m = mgr("echo_request");
    let o = CheckOptions {
        timeout: Duration::from_secs(7),
        screenshot_dir: Some(std::env::temp_dir().join("kepler-shots")),
    };
    let r = m
        .check("http://localhost:3000/x", &Viewport::defaults(), &o)
        .unwrap();
    let seen: serde_json::Value = serde_json::from_str(&r.title).unwrap();
    assert_eq!(seen["url"], "http://localhost:3000/x");
    assert_eq!(seen["timeout_ms"], 7000);
    assert_eq!(seen["viewports"], 3);
    assert!(seen["screenshot_dir"]
        .as_str()
        .unwrap()
        .contains("kepler-shots"));
}

#[test]
fn a_hung_worker_is_killed_on_timeout_and_restarted_lazily() {
    let mut m = mgr("hang");
    let t = Instant::now();
    let e = m.check("http://localhost/", &vp(), &opts(1)).unwrap_err();
    assert!(matches!(e, VisionError::Timeout), "{e}");
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    assert!(!m.is_running(), "the hung worker must be gone");
    let _ = m.check("http://localhost/", &vp(), &opts(1));
    assert_eq!(m.spawn_count(), 2, "a new worker is started on demand");
}

#[test]
fn crash_oversized_output_junk_and_error_frames_are_handled() {
    let mut m = mgr("crash");
    assert!(matches!(
        m.check("http://localhost/", &vp(), &opts(5)),
        Err(VisionError::WorkerExited)
    ));
    assert!(!m.is_running());

    let mut m = mgr("huge");
    assert!(matches!(
        m.check("http://localhost/", &vp(), &opts(5)),
        Err(VisionError::TooLarge)
    ));
    assert!(!m.is_running());

    let mut m = mgr("error");
    match m.check("http://localhost/", &vp(), &opts(5)) {
        Err(VisionError::Worker { code, .. }) => assert_eq!(code, "browser_unavailable"),
        other => panic!("{other:?}"),
    }
    // A plain error frame is not fatal: the worker keeps running.
    assert!(m.is_running());

    // Stray log lines and frames for other ids are skipped.
    let mut m = mgr("junk");
    assert_eq!(
        m.check("http://localhost/", &vp(), &opts(5)).unwrap().title,
        "Demo"
    );

    // Endless log spam is cut off instead of being buffered forever.
    let mut m = mgr("spam");
    assert!(m.check("http://localhost/", &vp(), &opts(5)).is_err());
    assert!(!m.is_running());
}

#[test]
fn missing_playwright_is_reported_before_any_check() {
    let mut m = mgr("noplaywright");
    match m.check("http://localhost/", &vp(), &opts(5)) {
        Err(VisionError::Worker { code, .. }) => assert_eq!(code, "playwright_unavailable"),
        other => panic!("{other:?}"),
    }
    assert!(!m.is_running());
}

#[test]
fn the_worker_environment_is_minimal() {
    std::env::set_var("KEPLER_TEST_SECRET_TOKEN", "hunter2");
    let probe = std::env::temp_dir().join(format!("kepler-vision-probe-{}", std::process::id()));
    let _ = std::fs::remove_file(&probe);
    let mut cmd = fake("ok");
    cmd.env.push((
        "KEPLER_VISION_PROBE_FILE".into(),
        probe.display().to_string(),
    ));
    let mut m = WorkerManager::new(cmd, Vec::new());
    m.check("http://localhost/", &vp(), &opts(5)).unwrap();
    m.shutdown();
    assert_eq!(std::fs::read_to_string(&probe).unwrap(), "leaked=false");
}

#[test]
fn from_env_requires_explicit_configuration() {
    // Not set in the test environment: Vision stays off until the user configures a worker.
    if std::env::var_os("KEPLER_VISION_WORKER").is_none() {
        assert!(matches!(
            WorkerCommand::from_env(),
            Err(VisionError::NotConfigured)
        ));
    }
}
