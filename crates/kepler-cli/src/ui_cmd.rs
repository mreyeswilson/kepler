//! `kepler ui`: the local Genesis dashboard (ADR-0024). Loopback only, token in the URL.

use clap::Args;
use kepler_ui::{start, UiConfig};
use std::process::ExitCode;

#[derive(Args)]
pub struct UiArgs {
    /// Port to listen on (127.0.0.1 only). 0 = a random free port.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Allow approving/denying waiting Orbit tasks from the page (each needs typed confirmation).
    #[arg(long)]
    allow_actions: bool,
    /// Open the URL in the default browser (the URL carries the session token).
    #[arg(long)]
    open: bool,
    /// Print one JSON line with the URL, port and token instead of text.
    #[arg(long)]
    json: bool,
}

pub fn run(args: UiArgs) -> ExitCode {
    let db_path = match kepler_storage::dirs::db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kepler: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("kepler: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async move {
        let server = match start(UiConfig {
            db_path,
            port: args.port,
            allow_actions: args.allow_actions,
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("kepler: {e}");
                return ExitCode::FAILURE;
            }
        };
        let url = server.url();
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "url": url, "port": server.addr.port(), "token": server.token,
                    "actions": server.allow_actions,
                })
            );
        } else {
            println!("Kepler UI: {url}");
            println!(
                "Local only; the link carries a one-time session token (do not share it). Actions: {}. Ctrl+C stops.",
                if server.allow_actions { "enabled" } else { "read-only" }
            );
        }
        if args.open {
            open_browser(&url);
        }
        let _ = tokio::signal::ctrl_c().await;
        server.shutdown();
        ExitCode::SUCCESS
    })
}

fn open_browser(url: &str) {
    use std::process::{Command, Stdio};
    #[cfg(windows)]
    let r = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    #[cfg(target_os = "macos")]
    let r = Command::new("open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = Command::new("xdg-open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if r.is_err() {
        eprintln!("kepler: could not open a browser; open the URL manually");
    }
}
