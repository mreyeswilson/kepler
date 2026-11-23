//! Wire types of the Vision worker protocol v1 (mirrors `workers/vision/src/protocol.ts`).

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_VIEWPORTS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub fn new(name: &str, width: u32, height: u32) -> Self {
        Self {
            name: name.into(),
            width,
            height,
        }
    }

    /// Common smoke-test set: a phone, a tablet and a laptop.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new("mobile", 375, 812),
            Self::new("tablet", 768, 1024),
            Self::new("desktop", 1280, 800),
        ]
    }

    /// Parse `WIDTHxHEIGHT` (200..=4000 each).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (w, h) = spec.split_once(['x', 'X']).ok_or("expected WIDTHxHEIGHT")?;
        let num = |s: &str| {
            s.parse::<u32>()
                .ok()
                .filter(|n| (200..=4000).contains(n))
                .ok_or("width and height must be integers between 200 and 4000")
        };
        let (w, h) = (num(w)?, num(h)?);
        Ok(Self::new(&format!("{w}x{h}"), w, h))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Issue {
    pub rule: String,
    pub count: u32,
    #[serde(default)]
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ViewportResult {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub scroll_width: u32,
    #[serde(default)]
    pub client_width: u32,
    #[serde(default)]
    pub horizontal_overflow: bool,
    #[serde(default)]
    pub console_errors: Vec<String>,
    #[serde(default)]
    pub page_errors: Vec<String>,
    #[serde(default)]
    pub failed_requests: Vec<String>,
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub screenshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckResult {
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub viewports: Vec<ViewportResult>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorBody {
    pub code: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<ErrorBody>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_specs_are_bounded() {
        assert_eq!(
            Viewport::parse("375x812").unwrap(),
            Viewport::new("375x812", 375, 812)
        );
        for bad in ["375", "10x10", "5000x800", "axb", "375x"] {
            assert!(Viewport::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(Viewport::defaults().len(), 3);
    }

    #[test]
    fn results_tolerate_unknown_and_missing_optional_fields() {
        let r: CheckResult = serde_json::from_str(
            r#"{"url":"http://localhost/","viewports":[{"name":"m","width":375,"height":812,"future":1}],"extra":true}"#,
        )
        .unwrap();
        assert_eq!(r.viewports[0].issues.len(), 0);
        assert!(r.viewports[0].screenshot.is_none());
    }
}
