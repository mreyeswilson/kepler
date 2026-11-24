//! A strict validator for the small JSON Schema subset the tool schemas use, so the schema the
//! model sees and the validation the server applies can never drift apart.
//!
//! Supported keywords: `type` (string, integer, boolean, array, object), `enum`, `minLength`,
//! `maxLength`, `minimum`, `maximum`, `items`, `minItems`, `maxItems`, `properties`, `required`
//! and `additionalProperties: false`. Strings may not contain NUL. Unknown keywords are ignored
//! (they are documentation such as `description`). Errors are short and carry the JSON path.

use serde_json::Value;

/// Deepest nesting accepted in arguments (the plan schema needs 4).
const MAX_DEPTH: usize = 8;

pub fn validate(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    check(schema, value, path, 0)
}

fn check(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: nested too deeply"));
    }
    let Some(s) = schema.as_object() else {
        return Ok(());
    };
    if let Some(t) = s.get("type").and_then(Value::as_str) {
        let ok = match t {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !ok {
            return Err(format!("{path}: expected {t}"));
        }
    }
    if let Some(options) = s.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            let names: Vec<String> = options
                .iter()
                .map(|o| o.as_str().map_or_else(|| o.to_string(), str::to_string))
                .collect();
            return Err(format!("{path}: must be one of {}", names.join(", ")));
        }
    }
    match value {
        Value::String(text) => {
            if text.contains('\0') {
                return Err(format!("{path}: contains NUL"));
            }
            let n = text.chars().count();
            if let Some(min) = s.get("minLength").and_then(Value::as_u64) {
                if (n as u64) < min {
                    return Err(format!("{path}: shorter than {min}"));
                }
            }
            if let Some(max) = s.get("maxLength").and_then(Value::as_u64) {
                if n as u64 > max {
                    return Err(format!("{path}: longer than {max}"));
                }
            }
        }
        Value::Number(_) => {
            if let Some(v) = value.as_i64() {
                if let Some(min) = s.get("minimum").and_then(Value::as_i64) {
                    if v < min {
                        return Err(format!("{path}: below {min}"));
                    }
                }
                if let Some(max) = s.get("maximum").and_then(Value::as_i64) {
                    if v > max {
                        return Err(format!("{path}: above {max}"));
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(min) = s.get("minItems").and_then(Value::as_u64) {
                if (items.len() as u64) < min {
                    return Err(format!("{path}: needs at least {min} item(s)"));
                }
            }
            if let Some(max) = s.get("maxItems").and_then(Value::as_u64) {
                if items.len() as u64 > max {
                    return Err(format!("{path}: more than {max} item(s)"));
                }
            }
            if let Some(item_schema) = s.get("items") {
                for (i, it) in items.iter().enumerate() {
                    check(item_schema, it, &format!("{path}[{i}]"), depth + 1)?;
                }
            }
        }
        Value::Object(map) => {
            if let Some(required) = s.get("required").and_then(Value::as_array) {
                for r in required.iter().filter_map(Value::as_str) {
                    if !map.contains_key(r) {
                        return Err(format!("{path}: `{r}` is required"));
                    }
                }
            }
            let props = s.get("properties").and_then(Value::as_object);
            for (k, v) in map {
                match props.and_then(|p| p.get(k)) {
                    Some(sub) => check(sub, v, &format!("{path}.{k}"), depth + 1)?,
                    None if s.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        return Err(format!("{path}: unknown field `{k}`"));
                    }
                    None => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({"type": "object", "required": ["a"], "additionalProperties": false, "properties": {
            "a": {"type": "string", "minLength": 2, "maxLength": 4},
            "n": {"type": "integer", "minimum": 1, "maximum": 3},
            "b": {"type": "boolean"},
            "e": {"type": "string", "enum": ["x", "y"]},
            "l": {"type": "array", "minItems": 1, "maxItems": 2, "items": {"type": "object",
                "additionalProperties": false, "required": ["k"], "properties": {"k": {"type": "string"}}}}
        }})
    }

    fn ok(v: Value) -> Result<(), String> {
        validate(&schema(), &v, "arguments")
    }

    #[test]
    fn accepts_valid_and_rejects_each_violation_with_a_path() {
        assert!(ok(json!({"a": "abc", "n": 2, "b": true, "e": "x", "l": [{"k": "v"}]})).is_ok());
        let e = |v: Value| ok(v).unwrap_err();
        assert!(e(json!({})).contains("`a` is required"));
        assert!(e(json!({"a": "z"})).contains("shorter"));
        assert!(e(json!({"a": "zzzzz"})).contains("longer"));
        assert!(e(json!({"a": 1})).contains("expected string"));
        assert!(e(json!({"a": "ab", "n": 0})).contains("below"));
        assert!(e(json!({"a": "ab", "n": 4})).contains("above"));
        assert!(e(json!({"a": "ab", "n": 1.5})).contains("expected integer"));
        assert!(e(json!({"a": "ab", "n": "1"})).contains("expected integer"));
        assert!(e(json!({"a": "ab", "b": "yes"})).contains("expected boolean"));
        assert!(e(json!({"a": "ab", "e": "z"})).contains("must be one of"));
        assert!(e(json!({"a": "ab", "extra": 1})).contains("unknown field `extra`"));
        assert!(e(json!({"a": "ab", "l": []})).contains("at least 1"));
        assert!(
            e(json!({"a": "ab", "l": [{"k": "1"}, {"k": "2"}, {"k": "3"}]}))
                .contains("more than 2")
        );
        assert!(e(json!({"a": "ab", "l": [{"k": "1", "z": 1}]})).contains("arguments.l[0]"));
        assert!(e(json!({"a": "a\u{0}b"})).contains("NUL"));
        assert!(validate(&schema(), &json!([1]), "arguments")
            .unwrap_err()
            .contains("expected object"));
    }

    #[test]
    fn depth_is_bounded() {
        let mut v = json!({"a": "ab"});
        for _ in 0..12 {
            v = json!({"x": v});
        }
        let open = json!({"type": "object"});
        assert!(
            validate(&open, &v, "p").is_ok(),
            "unconstrained objects are not walked"
        );
        let mut deep = json!({"type": "object"});
        for _ in 0..12 {
            deep = json!({"type": "object", "properties": {"x": deep}});
        }
        assert!(validate(&deep, &v, "p").unwrap_err().contains("deeply"));
    }
}
