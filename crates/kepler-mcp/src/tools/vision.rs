//! Vision tools: configuration status (read-only) and an on-demand page check.

use crate::tool::{obj, string, Hints, Tool};
use serde_json::json;

pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "kepler_vision_status",
            title: "Vision worker status",
            description: "Show whether Kepler's Vision worker is installed/configured and which hosts are allowed. Starts nothing. Use before kepler_vision_check to know whether it can work (if not installed the user must run `kepler vision setup`).",
            schema: || obj(json!({}), &[]),
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_vision_check",
            title: "Check a local page in a headless browser",
            description: "Open a LOCAL page (localhost, 127.0.0.1, [::1], *.localhost; other hosts are refused) in a headless browser for a few seconds and report horizontal overflow, console/page errors, failed requests and basic accessibility issues at phone/tablet/desktop sizes. Use after changing a UI served by a dev server. Starts the worker only for this call; nothing is stored or saved.",
            schema: || obj(json!({"url": string("http(s) URL on an allowed local host, e.g. http://localhost:5173/", 2048)}), &["url"]),
            hints: Hints::WRITE.open_world().non_destructive(),
            handler: None,
        },
    ]
}
