//! One-shot event delivery with lazy daemon start and guaranteed passthrough fallback.

use kepler_events::Envelope;
use kepler_ipc::{Client, ClientError, Endpoint, HelloOptions};
use kepler_policy::{Decision, Verdict};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum EmitOutcome {
    /// The daemon (or in-process engine) produced a verdict.
    Verdict(Verdict),
    /// Core unavailable/slow/erroring: the original input proceeds untouched.
    Passthrough { reason: String },
    /// The event itself is invalid or was refused (code is stable, see ADR-0007).
    Rejected { code: String, message: String },
}

pub struct EmitOptions<'a> {
    /// Called once when no daemon answers; should start one detached. `None` = never autostart.
    pub spawn_daemon: Option<&'a dyn Fn() -> std::io::Result<()>>,
    /// Upper bound for the whole attempt (connect + handshake + request).
    pub timeout: Duration,
    /// How long to wait for a freshly spawned daemon to start listening.
    pub autostart_wait: Duration,
}

impl Default for EmitOptions<'_> {
    fn default() -> Self {
        Self {
            spawn_daemon: None,
            timeout: Duration::from_secs(2),
            autostart_wait: Duration::from_secs(3),
        }
    }
}

fn passthrough(reason: &str) -> EmitOutcome {
    EmitOutcome::Passthrough {
        reason: reason.to_string(),
    }
}

/// Send one event. Never returns an error that should stop the harness: anything
/// other than a validation/refusal result is a passthrough.
pub async fn emit_one_shot(
    endpoint: &Endpoint,
    event: serde_json::Value,
    opts: &EmitOptions<'_>,
) -> EmitOutcome {
    // Reject junk locally: no point starting a daemon for an invalid event.
    if let Err(e) = Envelope::from_value(event.clone()) {
        return EmitOutcome::Rejected {
            code: e.code().to_string(),
            message: e.to_string(),
        };
    }
    let mut hello = HelloOptions::new("kepler-cli");
    hello.timeout = opts.timeout;

    let mut client = match Client::connect(endpoint, hello.clone()).await {
        Ok(c) => c,
        Err(ClientError::Unavailable(_)) => {
            let Some(spawn) = opts.spawn_daemon else {
                return passthrough("core_unavailable");
            };
            if spawn().is_err() {
                return passthrough("core_spawn_failed");
            }
            match connect_with_backoff(endpoint, hello, opts.autostart_wait).await {
                Some(c) => c,
                None => return passthrough("core_unavailable"),
            }
        }
        Err(_) => return passthrough("core_handshake_failed"),
    };
    match client.send_event_value(event).await {
        Ok(v) => EmitOutcome::Verdict(v),
        Err(ClientError::Rejected { code, message, .. }) => match code.as_str() {
            // Core-side hiccups are not the event's fault: keep the harness moving.
            "handler_timeout" | "internal" => passthrough(&format!("core_error:{code}")),
            _ => EmitOutcome::Rejected { code, message },
        },
        Err(_) => passthrough("core_unavailable"),
    }
}

/// Bounded retry (short sleeps) while a just-spawned daemon binds its endpoint.
async fn connect_with_backoff(
    endpoint: &Endpoint,
    hello: HelloOptions,
    total: Duration,
) -> Option<Client> {
    let deadline = tokio::time::Instant::now() + total;
    let mut delay = Duration::from_millis(20);
    loop {
        tokio::time::sleep(delay).await;
        if let Ok(c) = Client::connect(endpoint, hello.clone()).await {
            return Some(c);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        delay = (delay * 2).min(Duration::from_millis(250));
    }
}

/// Evaluate without any daemon (same engine, no persistence, no sessions).
pub fn emit_in_process(event: serde_json::Value) -> EmitOutcome {
    match Envelope::from_value(event) {
        Err(e) => EmitOutcome::Rejected {
            code: e.code().to_string(),
            message: e.to_string(),
        },
        Ok(env) => EmitOutcome::Verdict(kepler_policy::PolicyEngine::default().evaluate(&env)),
    }
}

impl EmitOutcome {
    /// JSON printed by `kepler emit`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Verdict(v) => serde_json::to_value(v).unwrap_or_else(
                |_| serde_json::json!({"action": "passthrough", "reason": "serialize_failed"}),
            ),
            Self::Passthrough { reason } => {
                serde_json::to_value(Verdict::new(Decision::passthrough(reason.clone())))
                    .unwrap_or_else(|_| serde_json::json!({"action": "passthrough"}))
            }
            Self::Rejected { code, message } => {
                serde_json::json!({"action": "rejected", "code": code, "message": message})
            }
        }
    }
}
