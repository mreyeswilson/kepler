use kepler_events::capabilities::{Advertisement, Negotiated};
use kepler_policy::Verdict;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
/// Max bytes of one frame, excluding the terminating newline.
pub const MAX_FRAME_BYTES: usize = 128 * 1024;
pub const MAX_REQUEST_ID_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol: u32,
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Advertisement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Hello(Hello),
    Event {
        request_id: String,
        event: Value,
    },
    Ping {
        #[serde(default)]
        request_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    HelloAck {
        protocol: u32,
        server_version: String,
        max_frame_bytes: usize,
        negotiated: Option<Negotiated>,
    },
    Decision {
        request_id: String,
        event_id: String,
        decision: Verdict,
    },
    Pong {
        request_id: Option<String>,
    },
    Error {
        request_id: Option<String>,
        code: String,
        message: String,
        fatal: bool,
    },
}

impl ServerFrame {
    pub fn error(
        request_id: Option<String>,
        code: &str,
        message: impl Into<String>,
        fatal: bool,
    ) -> Self {
        Self::Error {
            request_id,
            code: code.to_string(),
            message: message.into(),
            fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_frames_are_strict() {
        let ok: ClientFrame =
            serde_json::from_str(r#"{"type":"hello","protocol":1,"client":"x"}"#).unwrap();
        assert!(matches!(ok, ClientFrame::Hello(_)));
        for bad in [
            r#"{"type":"hello","protocol":1,"client":"x","extra":1}"#,
            r#"{"type":"nope"}"#,
            r#"{"type":"event","request_id":"a"}"#,
            r#"{"type":"ping","junk":true}"#,
            r#"[]"#,
        ] {
            assert!(serde_json::from_str::<ClientFrame>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn server_frame_shapes() {
        let f = ServerFrame::error(None, "bad_frame", "x", false);
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"type":"error","request_id":null,"code":"bad_frame","message":"x","fatal":false}"#
        );
        let d = ServerFrame::Decision {
            request_id: "r".into(),
            event_id: "e".into(),
            decision: Verdict::new(kepler_policy::Decision::passthrough("t")),
        };
        assert_eq!(
            serde_json::to_string(&d).unwrap(),
            r#"{"type":"decision","request_id":"r","event_id":"e","decision":{"action":"passthrough","reason":"t"}}"#
        );
    }
}
