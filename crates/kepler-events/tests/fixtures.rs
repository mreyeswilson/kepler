#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kepler_events::{Envelope, EventError, EventKind, MAX_ENVELOPE_BYTES};
use serde_json::Value;
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/events")
}

fn manifest() -> Value {
    serde_json::from_slice(&std::fs::read(dir().join("manifest.json")).unwrap()).unwrap()
}

fn valid_json() -> String {
    std::fs::read_to_string(dir().join("valid/input.received.json")).unwrap()
}

#[test]
fn every_valid_fixture_parses_and_roundtrips() {
    let m = manifest();
    let files = m["valid"].as_array().unwrap();
    assert_eq!(
        files.len(),
        EventKind::ALL.len(),
        "one valid fixture per event kind"
    );
    let mut kinds = Vec::new();
    for f in files {
        let bytes = std::fs::read(dir().join(f.as_str().unwrap())).unwrap();
        let env = Envelope::parse(&bytes).unwrap_or_else(|e| panic!("{f}: {e}"));
        kinds.push(env.kind);
        // serialize -> parse is stable and equal to the original JSON value
        let again = Envelope::parse(env.to_json().unwrap().as_bytes()).unwrap();
        assert_eq!(env, again);
        let original: Value = serde_json::from_slice(&bytes).unwrap();
        let reserialized: Value = serde_json::from_str(&env.to_json().unwrap()).unwrap();
        assert_eq!(original, reserialized, "{f}");
    }
    for k in EventKind::ALL {
        assert!(kinds.contains(k), "missing fixture for {}", k.as_str());
    }
}

#[test]
fn every_invalid_fixture_is_rejected_with_expected_code() {
    let m = manifest();
    for entry in m["invalid"].as_array().unwrap() {
        let file = entry["file"].as_str().unwrap();
        let bytes = std::fs::read(dir().join(file)).unwrap();
        let err = Envelope::parse(&bytes).expect_err(file);
        assert_eq!(
            err.code(),
            entry["error"].as_str().unwrap(),
            "{file}: {err}"
        );
    }
}

#[test]
fn oversized_input_is_rejected_before_parsing() {
    let big = vec![b' '; MAX_ENVELOPE_BYTES + 1];
    assert!(matches!(
        Envelope::parse(&big),
        Err(EventError::Oversized { .. })
    ));
}

#[test]
fn oversized_text_field_is_rejected() {
    let text = "x".repeat(16 * 1024 + 1);
    let json = valid_json().replace("Fix the failing test in parser.rs", &text);
    let err = Envelope::parse(json.as_bytes()).unwrap_err();
    assert_eq!(err.code(), "field_too_large");
}

#[test]
fn too_many_paths_rejected() {
    let paths: Vec<String> = (0..65).map(|i| format!("\"f{i}.rs\"")).collect();
    let json = format!(
        r#"{{"version":1,"id":"e","type":"files.changed","timestamp":"2026-01-02T03:04:05Z","harness":"pi","project_id":"p","session_id":"s","correlation_id":"c","payload":{{"paths":[{}]}}}}"#,
        paths.join(",")
    );
    assert_eq!(
        Envelope::parse(json.as_bytes()).unwrap_err().code(),
        "field_too_large"
    );
}

#[test]
fn deeply_nested_garbage_is_malformed_not_a_crash() {
    let json = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    assert_eq!(
        Envelope::parse(json.as_bytes()).unwrap_err().code(),
        "malformed"
    );
}

#[test]
fn invalid_utf8_and_empty_input_are_malformed() {
    assert_eq!(
        Envelope::parse(&[0xff, 0xfe, 0x00]).unwrap_err().code(),
        "malformed"
    );
    assert_eq!(Envelope::parse(b"").unwrap_err().code(), "malformed");
}

#[test]
fn nul_bytes_in_text_are_rejected() {
    let json = valid_json().replace("Fix the", "Fix\\u0000the");
    assert_eq!(
        Envelope::parse(json.as_bytes()).unwrap_err().code(),
        "invalid_field"
    );
}

#[test]
fn identifier_length_limit() {
    let json = valid_json().replace("sess-001", &"s".repeat(129));
    assert_eq!(
        Envelope::parse(json.as_bytes()).unwrap_err().code(),
        "field_too_large"
    );
}

#[test]
fn payload_must_match_type() {
    // A session.closed payload under an input.received type is rejected.
    let json = valid_json().replace(
        r#""text": "Fix the failing test in parser.rs""#,
        r#""reason": "x""#,
    );
    assert!(Envelope::parse(json.as_bytes()).is_err());
}

#[test]
fn original_text_is_preserved_verbatim() {
    let env = Envelope::parse(valid_json().as_bytes()).unwrap();
    match env.payload {
        kepler_events::Payload::InputReceived(p) => {
            assert_eq!(p.text, "Fix the failing test in parser.rs");
        }
        other => panic!("unexpected {other:?}"),
    }
}
